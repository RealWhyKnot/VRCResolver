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

internal enum MeshAttempt
{
    Accept,
    Refuse,
    Hang,
    SlowAccept,
}

internal sealed class ScriptedMeshServer : IAsyncDisposable
{
    private readonly HttpListener _listener = new();
    private readonly Func<int, MeshAttempt> _policy;
    private readonly Func<int, WebSocket, string, Task> _onResolve;
    private readonly CancellationTokenSource _cts = new();
    private int _attempts;
    private int _connections;

    public int Port { get; }
    public int Attempts => Volatile.Read(ref _attempts);
    public int Connections => Volatile.Read(ref _connections);
    public ConcurrentQueue<string> Paths { get; } = new();
    public ConcurrentQueue<string> UserAgents { get; } = new();
    public ConcurrentQueue<(int Connection, string Id)> Resolves { get; } = new();
    public TimeSpan SlowAcceptDelay { get; set; } = TimeSpan.FromSeconds(2);

    public ScriptedMeshServer(Func<int, MeshAttempt> policy, Func<int, WebSocket, string, Task>? onResolve = null)
    {
        _policy = policy;
        _onResolve = onResolve ?? ((_, ws, id) => ResolvedAsync(ws, id));
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        Port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        _listener.Prefixes.Add("http://localhost:" + Port + "/");
        _listener.Start();
        _ = AcceptLoopAsync();
    }

    public Uri MeshUri => new("ws://localhost:" + Port + "/mesh");

    private async Task AcceptLoopAsync()
    {
        while (!_cts.IsCancellationRequested)
        {
            HttpListenerContext ctx;
            try { ctx = await _listener.GetContextAsync(); }
            catch { return; }

            int attempt = Interlocked.Increment(ref _attempts);
            Paths.Enqueue(ctx.Request.Url?.AbsolutePath ?? "");
            UserAgents.Enqueue(ctx.Request.UserAgent ?? "");

            switch (_policy(attempt))
            {
                case MeshAttempt.Refuse:
                    ctx.Response.StatusCode = 503;
                    ctx.Response.Close();
                    continue;
                case MeshAttempt.Hang:
                    _ = HoldUntilDisposedAsync(ctx);
                    continue;
                case MeshAttempt.SlowAccept:
                    _ = AcceptAfterDelayAsync(ctx);
                    continue;
            }

            if (!ctx.Request.IsWebSocketRequest)
            {
                ctx.Response.StatusCode = 400;
                ctx.Response.Close();
                continue;
            }

            WebSocket ws;
            try { ws = (await ctx.AcceptWebSocketAsync(null)).WebSocket; }
            catch { continue; }
            int index = Interlocked.Increment(ref _connections);
            _ = ServeAsync(index, ws);
        }
    }

    private async Task HoldUntilDisposedAsync(HttpListenerContext ctx)
    {
        try { await Task.Delay(Timeout.Infinite, _cts.Token); }
        catch (OperationCanceledException) { }
        try { ctx.Response.Abort(); } catch { }
    }

    private async Task AcceptAfterDelayAsync(HttpListenerContext ctx)
    {
        try { await Task.Delay(SlowAcceptDelay, _cts.Token); }
        catch (OperationCanceledException)
        {
            try { ctx.Response.Abort(); } catch { }
            return;
        }

        WebSocket ws;
        try { ws = (await ctx.AcceptWebSocketAsync(null)).WebSocket; }
        catch { return; }
        int index = Interlocked.Increment(ref _connections);
        await ServeAsync(index, ws);
    }

    private async Task ServeAsync(int index, WebSocket ws)
    {
        try
        {
            await SendAsync(ws, "{\"action\":\"welcome\",\"protocol_version\":1,\"node\":\"fake\"}");
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

    public ValueTask DisposeAsync()
    {
        _cts.Cancel();
        try { _listener.Stop(); } catch { }
        return ValueTask.CompletedTask;
    }
}

internal static class MeshTestClient
{
    public static void Set(MeshClient client, string name, object value)
    {
        var field = typeof(MeshClient).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        field!.SetValue(client, value);
    }

    public static MeshClient For(
        ScriptedMeshServer server,
        ConcurrentQueue<string>? requestedHosts = null,
        TimeSpan? connectDeadline = null,
        TimeSpan? reconnectGrace = null)
    {
        var client = new MeshClient();
        Set(client, "_meshUriForHost", (Func<string, Uri>)(host =>
        {
            requestedHosts?.Enqueue(host);
            return server.MeshUri;
        }));
        if (connectDeadline is { } deadline) Set(client, "_connectDeadline", deadline);
        if (reconnectGrace is { } grace) Set(client, "_reconnectGrace", grace);
        return client;
    }

    public static async Task WaitUntilAsync(Func<bool> condition, int timeoutMs, string what)
    {
        var sw = Stopwatch.StartNew();
        while (!condition())
        {
            if (sw.ElapsedMilliseconds > timeoutMs)
                throw new TimeoutException(what + " not reached within " + timeoutMs + " ms");
            await Task.Delay(20);
        }
    }
}
