using System;
using System.Collections.Concurrent;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class RateLimitDispatchTests
{
    private static async Task DispatchJsonAsync(MeshClient client, string json)
    {
        var method = typeof(MeshClient).GetMethod("DispatchFrameAsync",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);
        var task = (Task)method!.Invoke(client, new object[] { Encoding.UTF8.GetBytes(json), CancellationToken.None })!;
        await task;
    }

    private static ConcurrentDictionary<string, TaskCompletionSource<MeshResolveResult>> Pending(MeshClient client)
    {
        var field = typeof(MeshClient).GetField("_pending", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        return (ConcurrentDictionary<string, TaskCompletionSource<MeshResolveResult>>)field!.GetValue(client)!;
    }

    private static long CooldownTicks(MeshClient client)
    {
        var field = typeof(MeshClient).GetField("_resolveRateLimitedUntilTicks",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        return (long)field!.GetValue(client)!;
    }

    private static void SetCooldownTicks(MeshClient client, long ticks)
    {
        var field = typeof(MeshClient).GetField("_resolveRateLimitedUntilTicks",
            BindingFlags.Instance | BindingFlags.NonPublic);
        field!.SetValue(client, ticks);
    }

    [Fact]
    public async Task RateLimited_resolve_with_id_fails_that_pending_only()
    {
        var client = new MeshClient();
        var pending = Pending(client);
        var limited = new TaskCompletionSource<MeshResolveResult>();
        var other = new TaskCompletionSource<MeshResolveResult>();
        pending["req1"] = limited;
        pending["req2"] = other;

        await DispatchJsonAsync(client,
            "{\"action\":\"rate_limited\",\"meshAction\":\"resolve\",\"limit\":10,\"windowSeconds\":60,\"retryAfterSeconds\":6,\"id\":\"req1\"}");

        Assert.True(limited.Task.IsCompleted);
        var result = await limited.Task;
        Assert.Equal(WireConstants.FallbackRateLimited, result.Reason);
        Assert.Equal(WireConstants.ActionFallbackNative, result.Action);
        Assert.False(other.Task.IsCompleted);
        Assert.True(CooldownTicks(client) > DateTime.UtcNow.Ticks);
    }

    [Fact]
    public async Task RateLimited_resolve_without_id_fails_all_pending()
    {
        var client = new MeshClient();
        var pending = Pending(client);
        var a = new TaskCompletionSource<MeshResolveResult>();
        var b = new TaskCompletionSource<MeshResolveResult>();
        pending["a"] = a;
        pending["b"] = b;

        await DispatchJsonAsync(client,
            "{\"action\":\"rate_limited\",\"meshAction\":\"resolve\",\"limit\":10,\"windowSeconds\":60,\"retryAfterSeconds\":6}");

        Assert.True(a.Task.IsCompleted);
        Assert.True(b.Task.IsCompleted);
        Assert.Equal(WireConstants.FallbackRateLimited, (await a.Task).Reason);
    }

    [Fact]
    public async Task RateLimited_cooldown_is_clamped_to_ceiling()
    {
        var client = new MeshClient();
        await DispatchJsonAsync(client,
            "{\"action\":\"rate_limited\",\"meshAction\":\"resolve\",\"limit\":10,\"windowSeconds\":60,\"retryAfterSeconds\":86400}");

        long ticks = CooldownTicks(client);
        Assert.True(ticks <= DateTime.UtcNow.AddSeconds(61).Ticks,
            "cooldown must clamp to 60s even when the server asks for a day");
        Assert.True(ticks > DateTime.UtcNow.Ticks);
    }

    [Fact]
    public async Task RateLimited_nonresolve_action_sets_no_cooldown()
    {
        var client = new MeshClient();
        await DispatchJsonAsync(client,
            "{\"action\":\"rate_limited\",\"meshAction\":\"playback_feedback\",\"limit\":30,\"windowSeconds\":60,\"retryAfterSeconds\":2}");
        Assert.Equal(0, CooldownTicks(client));
    }

    [Fact]
    public async Task ResolveAsync_short_circuits_during_cooldown_without_wire_traffic()
    {
        var client = new MeshClient();
        SetCooldownTicks(client, DateTime.UtcNow.AddSeconds(30).Ticks);

        var result = await client.ResolveAsync(new ResolveRequest { Url = "https://example.com/v" },
            CancellationToken.None);

        Assert.Equal(WireConstants.ActionFallbackNative, result.Action);
        Assert.Equal(WireConstants.FallbackRateLimited, result.Reason);
        var frame = JsonSerializer.Deserialize<ResolveResponse>(result.Frame);
        Assert.Equal(WireConstants.FallbackRateLimited, frame!.Reason);
    }

    [Fact]
    public async Task ProtocolError_with_id_fails_that_pending()
    {
        var client = new MeshClient();
        var pending = Pending(client);
        var tcs = new TaskCompletionSource<MeshResolveResult>();
        pending["bad1"] = tcs;

        await DispatchJsonAsync(client,
            "{\"action\":\"protocol_error\",\"reason\":\"oversize_field\",\"field\":\"url\",\"id\":\"bad1\"}");

        Assert.True(tcs.Task.IsCompleted);
        Assert.Equal(WireConstants.FallbackProtocolError, (await tcs.Task).Reason);
    }

    [Fact]
    public async Task ProtocolError_without_id_never_touches_pending()
    {
        var client = new MeshClient();
        var pending = Pending(client);
        var tcs = new TaskCompletionSource<MeshResolveResult>();
        pending["r1"] = tcs;

        await DispatchJsonAsync(client,
            "{\"action\":\"protocol_error\",\"reason\":\"invalid_field\",\"field\":\"kind\"}");

        Assert.False(tcs.Task.IsCompleted);
    }
}
