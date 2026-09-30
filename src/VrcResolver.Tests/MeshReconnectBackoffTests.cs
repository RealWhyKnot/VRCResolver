using System;
using System.IO;
using System.Reflection;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class MeshReconnectBackoffTests : IDisposable
{
    private const string Node = "vrcresolver.com";
    private readonly string _dir = Path.Combine(Path.GetTempPath(), "vrcresolver-welcome-" + Guid.NewGuid().ToString("N"));

    public MeshReconnectBackoffTests() => Directory.CreateDirectory(_dir);

    public void Dispose()
    {
        try { Directory.Delete(_dir, true); } catch { }
    }

    private static async Task DispatchJsonAsync(MeshClient client, string json)
    {
        var method = typeof(MeshClient).GetMethod("DispatchFrameAsync",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);
        await (Task)method!.Invoke(client, new object[] { Encoding.UTF8.GetBytes(json), CancellationToken.None })!;
    }

    private static void Set(MeshClient client, string name, object value)
    {
        var field = typeof(MeshClient).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        field!.SetValue(client, value);
    }

    private static int ReconnectAttempt(MeshClient client)
    {
        var field = typeof(MeshClient).GetField("_reconnectAttempt", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        return (int)field!.GetValue(client)!;
    }

    private MeshClient ReconnectedClient(bool cacheHasNode)
    {
        var cache = new WelcomeCache(Path.Combine(_dir, "v3_welcome_cache.json"));
        if (cacheHasNode)
            cache.Store(Node, new WelcomeFrame { ProtocolVersion = 3, Node = "node1" }, "hash1");
        var client = new MeshClient();
        Set(client, "_welcomeCache", cache);
        Set(client, "_isV3Connection", true);
        Set(client, "_currentNodeHost", Node);
        Set(client, "_reconnectAttempt", 4);
        return client;
    }

    [Fact]
    public async Task Cached_welcome_resets_the_reconnect_backoff()
    {
        var client = ReconnectedClient(cacheHasNode: true);

        await DispatchJsonAsync(client, "{\"action\":\"welcome_cached\",\"protocol_version\":3,\"node\":\"node1\"}");

        Assert.Equal(0, ReconnectAttempt(client));
    }

    [Fact]
    public async Task Full_welcome_resets_the_reconnect_backoff()
    {
        var client = ReconnectedClient(cacheHasNode: false);

        await DispatchJsonAsync(client, "{\"action\":\"welcome\",\"protocol_version\":3,\"node\":\"node1\"}");

        Assert.Equal(0, ReconnectAttempt(client));
    }

    [Fact]
    public async Task Cached_welcome_without_a_local_entry_keeps_the_backoff()
    {
        var client = ReconnectedClient(cacheHasNode: false);

        await DispatchJsonAsync(client, "{\"action\":\"welcome_cached\",\"protocol_version\":3,\"node\":\"node1\"}");

        Assert.Equal(4, ReconnectAttempt(client));
    }
}
