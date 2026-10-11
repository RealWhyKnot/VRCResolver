using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Reflection;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using VrcResolver.Shared;
using Xunit;
using WrapperProgram = VrcResolver.YtDlp.Program;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class WrapperPipeTests : IDisposable
{
    private const string Url = "https://example.com/watch";
    private const string ResolvedUrl = "https://media.example.com/stream/video.m3u8";

    private readonly string _scratchDir;
    private readonly string _originalPipeName;
    private readonly string? _originalLogPath;
    private readonly string _pipeName;

    public WrapperPipeTests()
    {
        _scratchDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-wrapperpipe-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_scratchDir);
        _originalPipeName = WrapperProgram.s_pipeName;
        _originalLogPath = WrapperProgram.s_logPathOverride;
        _pipeName = "vrcresolver.test." + Guid.NewGuid().ToString("N");
        WrapperProgram.s_pipeName = _pipeName;
        WrapperProgram.s_logPathOverride = Path.Combine(_scratchDir, "yt-dlp-wrapper.log");
        ResetWrapperState();
    }

    public void Dispose()
    {
        WrapperProgram.CloseLog();
        WrapperProgram.s_pipeName = _originalPipeName;
        WrapperProgram.s_logPathOverride = _originalLogPath;
        ResetWrapperState();
        try { Directory.Delete(_scratchDir, recursive: true); } catch { }
    }

    private static void ResetWrapperState()
    {
        SetStatic("s_pipeUnavailable", false);
        SetStatic("s_serverFallbackReason", null);
        SetStatic("s_serverRetryAtElapsedMs", null);
        SetStatic("s_lastRequestId", null);
    }

    private static void SetStatic(string name, object? value)
    {
        var field = typeof(WrapperProgram).GetField(name, BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(field);
        field!.SetValue(null, value);
    }

    private static string ResolvedLine(string url)
        => "{\"action\":\"resolved\",\"id\":\"srv1\",\"url\":\"" + url + "\"}";

    [Fact]
    public async Task Resolve_over_pipe_returns_the_resolved_url()
    {
        using var server = new StubPipeServer(_pipeName, ResolvedLine(ResolvedUrl));

        var result = await WrapperProgram.ResolveOverPipeAsync(Url, WireConstants.PlayerAvPro, null);

        Assert.Equal(ResolvedUrl, result.Url);
        Assert.Null(result.FallbackReason);
        Assert.Null(result.PublicMessage);
    }

    [Fact]
    public async Task Resolve_over_pipe_returns_the_server_fallback_reason()
    {
        using var server = new StubPipeServer(_pipeName,
            "{\"action\":\"fallback_native\",\"id\":\"srv1\",\"reason\":\"resolve_failed\",\"public_message\":\"Could not resolve this link\"}");

        var result = await WrapperProgram.ResolveOverPipeAsync(Url, WireConstants.PlayerAvPro, null);

        Assert.Null(result.Url);
        Assert.Equal(WireConstants.OgFallbackReasonServerFallbackNative, result.FallbackReason);
        Assert.Equal("Could not resolve this link", result.PublicMessage);
    }

    [Fact]
    public async Task Resolve_over_pipe_sends_a_wrapper_deadline_below_its_budget()
    {
        using var server = new StubPipeServer(_pipeName, ResolvedLine(ResolvedUrl));

        await WrapperProgram.ResolveOverPipeAsync(Url, WireConstants.PlayerAvPro, null, TimeSpan.FromSeconds(10));

        string request = await server.NextLineAsync();
        using var doc = JsonDocument.Parse(request);
        Assert.Equal(WireConstants.ActionResolve, doc.RootElement.GetProperty("action").GetString());
        int deadlineMs = doc.RootElement.GetProperty("wrapper_deadline_ms").GetInt32();
        Assert.InRange(deadlineMs, 8001, 9000);
    }

    [Fact]
    public async Task Notify_returns_at_once_after_the_pipe_connect_failed()
    {
        var result = await WrapperProgram.ResolveOverPipeAsync(Url, WireConstants.PlayerAvPro, null, TimeSpan.FromSeconds(2));
        Assert.Equal(WireConstants.OgFallbackReasonPipeConnectFailed, result.FallbackReason);

        var sw = Stopwatch.StartNew();
        await WrapperProgram.TrySendOgFallbackNotifyAsync(Url, WireConstants.OgFallbackReasonPipeConnectFailed, 2000);
        sw.Stop();

        Assert.True(sw.ElapsedMilliseconds < 100, "notify took " + sw.ElapsedMilliseconds + " ms");
    }

    [Fact]
    public async Task Og_failed_notify_returns_at_once_after_the_pipe_connect_failed()
    {
        var result = await WrapperProgram.ResolveOverPipeAsync(Url, WireConstants.PlayerAvPro, null, TimeSpan.FromSeconds(2));
        Assert.Equal(WireConstants.OgFallbackReasonPipeConnectFailed, result.FallbackReason);

        var sw = Stopwatch.StartNew();
        await WrapperProgram.TrySendOgFailedNotifyAsync(Url, "timeout", 1, "", 2000);
        sw.Stop();

        Assert.True(sw.ElapsedMilliseconds < 100, "notify took " + sw.ElapsedMilliseconds + " ms");
    }

    [Fact]
    public async Task Notify_reaches_the_watchdog_while_the_pipe_is_up()
    {
        using var server = new StubPipeServer(_pipeName, null);

        await WrapperProgram.TrySendOgFallbackNotifyAsync(Url, WireConstants.OgFallbackReasonPipeResolveFailed, 123, "Could not resolve this link");

        string line = await server.NextLineAsync();
        using var doc = JsonDocument.Parse(line);
        Assert.Equal(WireConstants.ActionOgFallbackNotify, doc.RootElement.GetProperty("action").GetString());
        Assert.Equal(WireConstants.OgFallbackReasonPipeResolveFailed, doc.RootElement.GetProperty("reason").GetString());
        Assert.Equal(123, doc.RootElement.GetProperty("elapsed_ms").GetInt64());
    }

    [Fact]
    public async Task Notify_still_goes_out_when_the_connect_retry_succeeds()
    {
        using var server = new StubPipeServer(_pipeName,
            "{\"action\":\"fallback_native\",\"id\":\"srv1\",\"reason\":\"resolve_failed\"}",
            startDelay: TimeSpan.FromMilliseconds(1400));

        var result = await WrapperProgram.ResolveOverPipeAsync(Url, WireConstants.PlayerAvPro, null);
        Assert.Equal(WireConstants.OgFallbackReasonServerFallbackNative, result.FallbackReason);
        await server.NextLineAsync();

        await WrapperProgram.TrySendOgFallbackNotifyAsync(Url, result.FallbackReason!, 2000);

        string line = await server.NextLineAsync();
        using var doc = JsonDocument.Parse(line);
        Assert.Equal(WireConstants.ActionOgFallbackNotify, doc.RootElement.GetProperty("action").GetString());
    }

    private sealed class StubPipeServer : IDisposable
    {
        private readonly string _name;
        private readonly string? _reply;
        private readonly CancellationTokenSource _cts = new();
        private readonly Channel<string> _lines = Channel.CreateUnbounded<string>();
        private readonly Task _loop;

        public StubPipeServer(string name, string? reply, TimeSpan startDelay = default)
        {
            _name = name;
            _reply = reply;
            var first = startDelay == TimeSpan.Zero ? NewInstance() : null;
            _loop = Task.Run(() => RunAsync(first, startDelay));
        }

        public async Task<string> NextLineAsync()
            => await _lines.Reader.ReadAsync().AsTask().WaitAsync(TimeSpan.FromSeconds(10));

        public void Dispose()
        {
            _cts.Cancel();
            try { _loop.Wait(TimeSpan.FromSeconds(5)); } catch { }
            _cts.Dispose();
        }

        private NamedPipeServerStream NewInstance()
            => new(_name, PipeDirection.InOut, NamedPipeServerStream.MaxAllowedServerInstances,
                PipeTransmissionMode.Byte, PipeOptions.Asynchronous);

        private async Task RunAsync(NamedPipeServerStream? pipe, TimeSpan startDelay)
        {
            try
            {
                if (pipe == null)
                {
                    await Task.Delay(startDelay, _cts.Token);
                    pipe = NewInstance();
                }

                while (!_cts.IsCancellationRequested)
                {
                    using (pipe)
                    {
                        await pipe.WaitForConnectionAsync(_cts.Token);
                        using var reader = new StreamReader(pipe, Encoding.UTF8, leaveOpen: true);
                        string? line = await reader.ReadLineAsync(_cts.Token);
                        if (line != null) _lines.Writer.TryWrite(line);
                        if (_reply != null)
                        {
                            await pipe.WriteAsync(Encoding.UTF8.GetBytes(_reply + "\n"), _cts.Token);
                            await pipe.FlushAsync(_cts.Token);
                        }
                        await reader.ReadToEndAsync(_cts.Token);
                    }
                    pipe = NewInstance();
                }
            }
            catch (OperationCanceledException) { }
            catch (IOException) { }
        }
    }
}
