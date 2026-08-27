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
}
