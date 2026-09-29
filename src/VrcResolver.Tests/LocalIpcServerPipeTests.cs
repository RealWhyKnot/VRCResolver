using System;
using System.IO;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class LocalIpcServerPipeTests
{
    private static async Task<ResolveResponse> RoundTripAsync(
        string request,
        ResolveCache? cache = null,
        OgFallbackHint? ogHint = null,
        ResolverHealthGate? health = null)
    {
        string pipeName = "vrcresolver.test." + Guid.NewGuid().ToString("N");
        var server = new LocalIpcServer(new MeshClient(), cache, ogHint, health);
        server.StartForTests(pipeName);
        try
        {
            using var client = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut,
                PipeOptions.Asynchronous);
            await client.ConnectAsync(TimeSpan.FromSeconds(5), CancellationToken.None);
            byte[] payload = Encoding.UTF8.GetBytes(request + "\n");
            await client.WriteAsync(payload);

            using var reader = new StreamReader(client, Encoding.UTF8, leaveOpen: true);
            string? line = await reader.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(10));
            Assert.False(string.IsNullOrEmpty(line));
            var resp = JsonSerializer.Deserialize<ResolveResponse>(line!);
            Assert.NotNull(resp);
            return resp!;
        }
        finally
        {
            await server.StopAsync();
            server.Dispose();
        }
    }

    [Fact]
    public async Task MalformedJson_YieldsInternalErrorFallback()
    {
        var resp = await RoundTripAsync("{not json at all");
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.FallbackInternalError, resp.Reason);
    }

    [Fact]
    public async Task NonResolveAction_IsRejected()
    {
        var resp = await RoundTripAsync(
            "{\"action\":\"ping\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\"}");
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.FallbackInternalError, resp.Reason);
    }

    [Theory]
    [InlineData("AVPro")]
    [InlineData("Unity")]
    [InlineData("")]
    public async Task PlayerVocabulary_IsCaseSensitive(string player)
    {
        var resp = await RoundTripAsync(
            "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"" + player + "\"}");
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.FallbackInternalError, resp.Reason);
    }

    [Fact]
    public async Task OgFallbackHint_ShortCircuitsToPriorLoadFailure()
    {
        var hint = new OgFallbackHint();
        hint.RecordLoadFailure("https://example.com/broken");
        var resp = await RoundTripAsync(
            "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/broken\",\"player\":\"avpro\"}",
            ogHint: hint);
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.OgFallbackReasonPriorLoadFailure, resp.Reason);
    }

    [Fact]
    public async Task OpenHealthGate_ShortCircuitsToResolverUnhealthy()
    {
        var gate = new ResolverHealthGate();
        for (int i = 0; i < ResolverHealthGate.OpenThreshold; i++)
            gate.RecordResolveOutcome(healthy: false, resolved: false);

        var resp = await RoundTripAsync(
            "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\"}",
            health: gate);
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.OgFallbackReasonResolverUnhealthy, resp.Reason);
    }

    [Fact]
    public async Task OpenHealthGate_LetsTheWrapperReAskThrough()
    {
        var resp = await RoundTripAsync(
            "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\",\"skip_native_hint\":true}",
            health: OpenGate());
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.FallbackServerUnreachable, resp.Reason);
    }

    [Fact]
    public async Task OpenHealthGate_KeepsSitesWithBlockedOgOnTheServer()
    {
        var hint = new OgFallbackHint();
        hint.RecordOgBlocked("https://youtu.be/abc");
        var resp = await RoundTripAsync(
            "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://www.youtube.com/watch?v=def\",\"player\":\"avpro\"}",
            ogHint: hint,
            health: OpenGate());
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.FallbackServerUnreachable, resp.Reason);
    }

    [Fact]
    public async Task PriorLoadFailure_DoesNotSendABlockedSiteToOg()
    {
        var hint = new OgFallbackHint();
        hint.RecordLoadFailure("https://youtu.be/abc");
        hint.RecordOgBlocked("https://youtu.be/abc");
        var resp = await RoundTripAsync(
            "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://youtu.be/abc\",\"player\":\"avpro\"}",
            ogHint: hint);
        Assert.Equal(WireConstants.ActionFallbackNative, resp.Action);
        Assert.Equal(WireConstants.FallbackServerUnreachable, resp.Reason);
    }

    [Theory]
    [InlineData("sign_in_required", true)]
    [InlineData("cf_403", true)]
    [InlineData("rate_limited", true)]
    [InlineData("content_not_found", false)]
    [InlineData("timeout", false)]
    public async Task OgFailedNotify_BlocksOgForTheSiteOnlyForSiteWideReasons(string reason, bool expectBlocked)
    {
        var hint = new OgFallbackHint();
        hint.RecordLoadFailure("https://youtu.be/abc");
        string pipeName = "vrcresolver.test." + Guid.NewGuid().ToString("N");
        var server = new LocalIpcServer(new MeshClient(), null, hint, null);
        server.StartForTests(pipeName);
        try
        {
            using (var client = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous))
            {
                await client.ConnectAsync(TimeSpan.FromSeconds(5), CancellationToken.None);
                byte[] payload = Encoding.UTF8.GetBytes(
                    "{\"action\":\"wrapper_og_failed\",\"url\":\"https://youtu.be/abc\",\"reason\":\"" + reason
                    + "\",\"exit_code\":1,\"rid\":\"r1\"}\n");
                await client.WriteAsync(payload);
                await client.FlushAsync();
            }

            var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(5);
            while (hint.LiveEntryCountForTests() > 0 && DateTime.UtcNow < deadline)
                await Task.Delay(25);
            Assert.Equal(0, hint.LiveEntryCountForTests());
            Assert.Equal(expectBlocked, hint.IsOgBlocked("https://www.youtube.com/watch?v=zzz"));
        }
        finally
        {
            await server.StopAsync();
            server.Dispose();
        }
    }

    private static ResolverHealthGate OpenGate()
    {
        var gate = new ResolverHealthGate();
        for (int i = 0; i < ResolverHealthGate.OpenThreshold; i++)
            gate.RecordResolveOutcome(healthy: false, resolved: false);
        return gate;
    }
}
