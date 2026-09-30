using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Net.WebSockets;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class MeshResendAfterDropTests
{
    private sealed class FakeMeshServer : IAsyncDisposable
    {
        private readonly HttpListener _listener = new();
        private readonly Func<int, WebSocket, string, Task> _onResolve;
        private readonly CancellationTokenSource _cts = new();
        private int _connections;

        public ConcurrentQueue<(int Connection, string Id)> Resolves { get; } = new();
        public int Port { get; }
        public int Connections => Volatile.Read(ref _connections);

        public FakeMeshServer(Func<int, WebSocket, string, Task> onResolve, Func<int, WebSocket, Task>? onConnect = null)
        {
            _onResolve = onResolve;
            var probe = new TcpListener(IPAddress.Loopback, 0);
            probe.Start();
            Port = ((IPEndPoint)probe.LocalEndpoint).Port;
            probe.Stop();
            _listener.Prefixes.Add("http://localhost:" + Port + "/");
            _listener.Start();
            _ = AcceptLoopAsync(onConnect);
        }

        public void StopAccepting()
        {
            try { _listener.Stop(); } catch { }
        }

        private async Task AcceptLoopAsync(Func<int, WebSocket, Task>? onConnect)
        {
            while (!_cts.IsCancellationRequested)
            {
                HttpListenerContext ctx;
                try { ctx = await _listener.GetContextAsync(); }
                catch { return; }
                if (!ctx.Request.IsWebSocketRequest)
                {
                    ctx.Response.StatusCode = 400;
                    ctx.Response.Close();
                    continue;
                }
                var wsCtx = await ctx.AcceptWebSocketAsync(null);
                int index = Interlocked.Increment(ref _connections);
                _ = ServeAsync(index, wsCtx.WebSocket, onConnect);
            }
        }

        private async Task ServeAsync(int index, WebSocket ws, Func<int, WebSocket, Task>? onConnect)
        {
            try
            {
                await SendAsync(ws, "{\"action\":\"welcome\",\"protocol_version\":1,\"node\":\"fake\"}");
                if (onConnect != null) await onConnect(index, ws);
                var buf = new byte[64 * 1024];
                while (ws.State == WebSocketState.Open && !_cts.IsCancellationRequested)
                {
                    var r = await ws.ReceiveAsync(buf, _cts.Token);
                    if (r.MessageType == WebSocketMessageType.Close) return;
                    string text = Encoding.UTF8.GetString(buf, 0, r.Count);
                    using var doc = JsonDocument.Parse(text);
                    string action = doc.RootElement.GetProperty("action").GetString() ?? "";
                    if (action == "ping")
                    {
                        await SendAsync(ws, "{\"action\":\"pong\"}");
                        continue;
                    }
                    if (action != WireConstants.ActionResolve) continue;
                    string id = doc.RootElement.GetProperty("id").GetString() ?? "";
                    Resolves.Enqueue((index, id));
                    await _onResolve(index, ws, id);
                }
            }
            catch { }
        }

        public static Task SendAsync(WebSocket ws, string json)
            => ws.SendAsync(Encoding.UTF8.GetBytes(json), WebSocketMessageType.Text, true, CancellationToken.None);

        public static Task ResolvedAsync(WebSocket ws, string id)
            => SendAsync(ws, "{\"action\":\"resolved\",\"id\":\"" + id + "\",\"url\":\"https://media.example/v.mp4\"}");

        public async ValueTask DisposeAsync()
        {
            _cts.Cancel();
            StopAccepting();
            await Task.CompletedTask;
        }
    }

    private static void Set(MeshClient client, string name, object value)
    {
        var field = typeof(MeshClient).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        field!.SetValue(client, value);
    }

    private static MeshClient ClientFor(int port, TimeSpan grace)
    {
        var client = new MeshClient();
        Set(client, "_meshUriForHost", (Func<string, Uri>)(_ => new Uri("ws://localhost:" + port + "/mesh")));
        Set(client, "_reconnectGrace", grace);
        return client;
    }

    private static async Task WaitUntilAsync(Func<bool> condition, int timeoutMs = 5000)
    {
        var sw = Stopwatch.StartNew();
        while (!condition())
        {
            if (sw.ElapsedMilliseconds > timeoutMs) throw new TimeoutException("condition not met");
            await Task.Delay(20);
        }
    }

    private static ResolveRequest Request() => new() { Url = "https://www.youtube.com/watch?v=abc", Player = "avpro" };

    [Fact]
    public async Task Resolve_in_flight_at_a_drop_is_resent_on_the_next_connection()
    {
        await using var server = new FakeMeshServer((conn, ws, id) =>
        {
            if (conn == 1)
            {
                ws.Abort();
                return Task.CompletedTask;
            }
            return FakeMeshServer.ResolvedAsync(ws, id);
        });
        await using var client = ClientFor(server.Port, TimeSpan.FromSeconds(5));
        await client.StartAsync();
        await WaitUntilAsync(() => client.IsConnected);

        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        var result = await client.ResolveAsync(Request(), cts.Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        var resolves = server.Resolves.ToArray();
        Assert.Equal(2, resolves.Length);
        Assert.Equal(1, resolves[0].Connection);
        Assert.Equal(2, resolves[1].Connection);
        Assert.Equal(resolves[0].Id, resolves[1].Id);
    }

    [Fact]
    public async Task Resolve_asked_while_reconnecting_waits_for_the_new_connection()
    {
        await using var server = new FakeMeshServer(
            (conn, ws, id) => FakeMeshServer.ResolvedAsync(ws, id),
            (conn, ws) =>
            {
                if (conn == 1) ws.Abort();
                return Task.CompletedTask;
            });
        await using var client = ClientFor(server.Port, TimeSpan.FromSeconds(5));
        await client.StartAsync();
        await WaitUntilAsync(() => server.Connections >= 1);
        await WaitUntilAsync(() => !client.IsConnected);

        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        var result = await client.ResolveAsync(Request(), cts.Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.Equal(2, server.Resolves.ToArray()[0].Connection);
    }

    [Fact]
    public async Task Resolve_fails_to_og_once_the_reconnect_grace_runs_out()
    {
        FakeMeshServer? self = null;
        await using var server = self = new FakeMeshServer((conn, ws, id) =>
        {
            self!.StopAccepting();
            ws.Abort();
            return Task.CompletedTask;
        });
        await using var client = ClientFor(server.Port, TimeSpan.FromSeconds(1.5));
        await client.StartAsync();
        await WaitUntilAsync(() => client.IsConnected);

        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        var sw = Stopwatch.StartNew();
        var result = await client.ResolveAsync(Request(), cts.Token);
        sw.Stop();

        Assert.Equal(WireConstants.ActionFallbackNative, result.Action);
        Assert.Equal(WireConstants.FallbackServerUnreachable, result.Reason);
        Assert.InRange(sw.ElapsedMilliseconds, 1000, 6000);
    }

    [Fact]
    public async Task Resolve_during_a_long_outage_fails_fast()
    {
        await using var client = ClientFor(1, TimeSpan.FromSeconds(5));
        Set(client, "_linkDownSinceTicks", DateTime.UtcNow.AddMinutes(-2).Ticks);

        var sw = Stopwatch.StartNew();
        var result = await client.ResolveAsync(Request(), CancellationToken.None);
        sw.Stop();

        Assert.Equal(WireConstants.FallbackServerUnreachable, result.Reason);
        Assert.True(sw.ElapsedMilliseconds < 500, "took " + sw.ElapsedMilliseconds + " ms");
    }
}
