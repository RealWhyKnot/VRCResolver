using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Runtime.Versioning;
using Microsoft.Win32;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class RelayWedgeTests
{
    [Theory]
    [InlineData("/play/abc123/manifest.m3u8", "abc123")]
    [InlineData("/play/abc123/proxy/9f/seg.ts", "abc123")]
    [InlineData("/play/abc123", "abc123")]
    public void TryGetPlayId_ReadsSecondSegment(string path, string expected)
    {
        Assert.True(RelayActivity.TryGetPlayId(path, out string playId));
        Assert.Equal(expected, playId);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("/play/")]
    [InlineData("/not-play/abc/manifest.m3u8")]
    public void TryGetPlayId_RejectsNonPlayPaths(string? path)
    {
        Assert.False(RelayActivity.TryGetPlayId(path, out _));
    }

    [Fact]
    public void LooksWedged_IgnoresUrlsThatAreNotTheLocalRelay()
    {
        Assert.False(RelayActivity.LooksWedged("https://vrcresolver.com/play/x/manifest.m3u8", out _));
        Assert.False(RelayActivity.LooksWedged("https://localhost.youtube.com:5000/other/x", out _));
        Assert.False(RelayActivity.LooksWedged("not a url", out _));
    }

    [Fact]
    public void LooksWedged_TrueWhenRelayNeverSawThePlay()
    {
        string playId = Guid.NewGuid().ToString("N");
        Assert.True(RelayActivity.LooksWedged(
            "https://localhost.youtube.com:54321/play/" + playId + "/manifest.m3u8?target=abc", out int port));
        Assert.Equal(54321, port);
    }

    [Fact]
    public async Task LooksWedged_FalseOnceRelayReceivedAnyRequestForThePlay()
    {
        int port = FreePort();
        string playId = Guid.NewGuid().ToString("N");
        var relay = new LocalRelayServer(port);
        relay.Start();
        try
        {
            using var http = new HttpClient();
            using var req = new HttpRequestMessage(HttpMethod.Get,
                "http://127.0.0.1:" + port + "/play/" + playId + "/manifest.m3u8?target=x");
            req.Headers.TryAddWithoutValidation("Origin", "http://example.com");
            using var resp = await http.SendAsync(req);
            Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);

            Assert.False(RelayActivity.LooksWedged(
                "https://localhost.youtube.com:" + port + "/play/" + playId + "/manifest.m3u8?target=x", out _));
        }
        finally
        {
            await relay.StopAsync();
        }
    }

    [Theory]
    [InlineData("full", true)]
    [InlineData("short", false)]
    [InlineData("throw", false)]
    public async Task Relay_FinishesOrResetsClientPromptlyWhateverTheUpstreamBodyDoes(string mode, bool expectBody)
    {
        int port = FreePort();
        var relay = new LocalRelayServer(port, "http", new StubUpstream(mode));
        relay.Start();
        try
        {
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(15) };
            string url = "http://127.0.0.1:" + port + "/play/" + Guid.NewGuid().ToString("N") + "/seg.ts?target="
                + LocalRelayTargetResolver.EncodeTargetParam("https://vrcresolver.com/api/proxy/seg.ts?url=abc");
            var sw = System.Diagnostics.Stopwatch.StartNew();
            if (expectBody)
            {
                byte[] body = await http.GetByteArrayAsync(url);
                Assert.Equal(StubUpstream.DeclaredLength, body.Length);
            }
            else
            {
                await Assert.ThrowsAnyAsync<Exception>(() => http.GetByteArrayAsync(url));
            }
            Assert.True(sw.Elapsed < TimeSpan.FromSeconds(5), "client waited " + sw.Elapsed.TotalSeconds + "s");
        }
        finally
        {
            await relay.StopAsync();
        }
    }

    [Fact]
    public async Task Relay_ResetsClientWhenUpstreamStallsMidBody()
    {
        int port = FreePort();
        var relay = new LocalRelayServer(port, "http", new StubUpstream("stall"), TimeSpan.FromMilliseconds(500));
        relay.Start();
        try
        {
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(15) };
            var sw = System.Diagnostics.Stopwatch.StartNew();
            await Assert.ThrowsAnyAsync<Exception>(() => http.GetByteArrayAsync(SegmentUrl(port, "seg.ts")));
            Assert.True(sw.Elapsed < TimeSpan.FromSeconds(5), "client waited " + sw.Elapsed.TotalSeconds + "s");
        }
        finally
        {
            await relay.StopAsync();
        }
    }

    [Fact]
    public async Task Relay_AbortsStalledClientAndKeepsServingNextRequest()
    {
        int port = FreePort();
        var relay = new LocalRelayServer(port, "http", new StubUpstream("bypath"), TimeSpan.FromMilliseconds(500));
        relay.Start();
        try
        {
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
            using (var resp = await http.GetAsync(SegmentUrl(port, "big.ts"), HttpCompletionOption.ResponseHeadersRead))
            {
                using var body = await resp.Content.ReadAsStreamAsync();
                await Task.Delay(TimeSpan.FromSeconds(3));
                await Assert.ThrowsAnyAsync<Exception>(async () =>
                {
                    byte[] sink = new byte[256 * 1024];
                    while (await body.ReadAsync(sink) > 0) { }
                });
            }

            byte[] next = await http.GetByteArrayAsync(SegmentUrl(port, "seg.ts"));
            Assert.Equal(StubUpstream.DeclaredLength, next.Length);
        }
        finally
        {
            await relay.StopAsync();
        }
    }

    private static string SegmentUrl(int port, string file)
    {
        return "http://127.0.0.1:" + port + "/play/" + Guid.NewGuid().ToString("N") + "/" + file + "?target="
            + LocalRelayTargetResolver.EncodeTargetParam("https://vrcresolver.com/api/proxy/" + file + "?url=abc");
    }

    [Fact]
    public void TryHandleWedgedRelay_PassesPortAndHonoursRecoveryResult()
    {
        string url = "https://localhost.youtube.com:54999/play/" + Guid.NewGuid().ToString("N") + "/manifest.m3u8";
        int seenPort = 0;

        using (var recovering = new VrcLogMonitor(new MeshClient(), onRelayWedged: p => { seenPort = p; return true; }))
            Assert.True(recovering.TryHandleWedgedRelay(url, 12000));
        Assert.Equal(54999, seenPort);

        using (var cooling = new VrcLogMonitor(new MeshClient(), onRelayWedged: _ => false))
            Assert.False(cooling.TryHandleWedgedRelay(url, 12000));

        using (var unwired = new VrcLogMonitor(new MeshClient()))
            Assert.False(unwired.TryHandleWedgedRelay(url, 12000));
    }

    [Fact]
    public void RelayBypass_HidesPortFileWhileWedgedAndRestoresOnNewSession()
    {
        string stateRoot = CreateTempStateRoot();
        try
        {
            var manager = new RelayPortManager(stateRoot);
            Assert.True(manager.Initialize());
            manager.WriteSchemeFile("https");
            string portFile = Path.Combine(stateRoot, "relay_port.txt");
            string schemeFile = Path.Combine(stateRoot, "relay_scheme.txt");
            var bypass = new RelayBypass(manager, "https");

            Assert.False(bypass.Engage(manager.CurrentPort + 1));
            Assert.True(File.Exists(portFile));

            Assert.True(bypass.Engage(manager.CurrentPort));
            Assert.True(bypass.Active);
            Assert.False(File.Exists(portFile));
            Assert.True(bypass.Engage(manager.CurrentPort));

            bypass.Release();
            Assert.False(bypass.Active);
            Assert.Equal(
                manager.CurrentPort.ToString(CultureInfo.InvariantCulture),
                File.ReadAllText(portFile).Trim());
            Assert.Equal("https", File.ReadAllText(schemeFile).Trim());
        }
        finally
        {
            DeleteTempStateRoot(stateRoot);
        }
    }

    [Theory]
    [InlineData(null, 16)]
    [InlineData(2, 16)]
    [InlineData(15, 16)]
    [InlineData(16, null)]
    [InlineData(32, null)]
    public void DecideRaise_OnlyEverRaises(int? current, int? expected)
    {
        Assert.Equal(expected, WinInetConnectionLimit.DecideRaise(current));
    }

    [Fact]
    public void EnsureThenRestore_RemovesValueThatWasAbsent()
    {
        WithTempKey((key, marker) =>
        {
            WinInetConnectionLimit.Ensure(key, marker, _ => { });
            Assert.Equal(16, key.GetValue(WinInetConnectionLimit.ValueName));
            Assert.Equal("absent", File.ReadAllText(marker));

            WinInetConnectionLimit.Restore(key, marker, _ => { });
            Assert.Null(key.GetValue(WinInetConnectionLimit.ValueName));
            Assert.False(File.Exists(marker));
        });
    }

    [Fact]
    public void EnsureThenRestore_PutsBackLowerPreviousValue()
    {
        WithTempKey((key, marker) =>
        {
            key.SetValue(WinInetConnectionLimit.ValueName, 4, RegistryValueKind.DWord);
            WinInetConnectionLimit.Ensure(key, marker, _ => { });
            WinInetConnectionLimit.Ensure(key, marker, _ => { });
            Assert.Equal("4", File.ReadAllText(marker));

            WinInetConnectionLimit.Restore(key, marker, _ => { });
            Assert.Equal(4, key.GetValue(WinInetConnectionLimit.ValueName));
        });
    }

    [Fact]
    public void Ensure_LeavesHigherValueAloneAndRestoreKeepsLaterUserChange()
    {
        WithTempKey((key, marker) =>
        {
            key.SetValue(WinInetConnectionLimit.ValueName, 32, RegistryValueKind.DWord);
            WinInetConnectionLimit.Ensure(key, marker, _ => { });
            Assert.Equal(32, key.GetValue(WinInetConnectionLimit.ValueName));
            Assert.False(File.Exists(marker));

            key.DeleteValue(WinInetConnectionLimit.ValueName);
            WinInetConnectionLimit.Ensure(key, marker, _ => { });
            key.SetValue(WinInetConnectionLimit.ValueName, 8, RegistryValueKind.DWord);
            WinInetConnectionLimit.Restore(key, marker, _ => { });
            Assert.Equal(8, key.GetValue(WinInetConnectionLimit.ValueName));
            Assert.False(File.Exists(marker));
        });
    }

    private sealed class StubUpstream : HttpMessageHandler
    {
        public const int DeclaredLength = 100_000;
        public const long BigLength = 1L << 30;
        private readonly string _mode;

        public StubUpstream(string mode) => _mode = mode;

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            bool big = _mode == "bypath" && request.RequestUri!.AbsolutePath.EndsWith("/big.ts", StringComparison.Ordinal);
            Stream body = _mode switch
            {
                "full" => new MemoryStream(new byte[DeclaredLength]),
                "short" => new MemoryStream(new byte[1000]),
                "stall" => new StallingStream(),
                "bypath" when big => new ZeroStream(BigLength),
                "bypath" => new MemoryStream(new byte[DeclaredLength]),
                _ => new FailingStream(),
            };
            var content = new StreamContent(body);
            content.Headers.ContentLength = big ? BigLength : DeclaredLength;
            content.Headers.ContentType = new System.Net.Http.Headers.MediaTypeHeaderValue("video/mp2t");
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK) { Content = content });
        }
    }

    private sealed class FailingStream : Stream
    {
        private bool _sent;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => 0; set => throw new NotSupportedException(); }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

        public override int Read(byte[] buffer, int offset, int count)
        {
            if (_sent) throw new IOException("upstream reset");
            _sent = true;
            int n = Math.Min(count, 1000);
            Array.Clear(buffer, offset, n);
            return n;
        }
    }

    private sealed class StallingStream : Stream
    {
        private bool _sent;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => 0; set => throw new NotSupportedException(); }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        public override int Read(byte[] buffer, int offset, int count) => throw new NotSupportedException();

        public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
        {
            if (!_sent)
            {
                _sent = true;
                int n = Math.Min(buffer.Length, 1000);
                buffer.Span[..n].Clear();
                return n;
            }
            await Task.Delay(Timeout.Infinite, cancellationToken);
            return 0;
        }
    }

    private sealed class ZeroStream : Stream
    {
        private long _left;
        public ZeroStream(long length) => _left = length;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => 0; set => throw new NotSupportedException(); }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        public override int Read(byte[] buffer, int offset, int count) => Read(buffer.AsSpan(offset, count));

        public override int Read(Span<byte> buffer)
        {
            int n = (int)Math.Min(buffer.Length, _left);
            buffer[..n].Clear();
            _left -= n;
            return n;
        }

        public override ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
            => ValueTask.FromResult(Read(buffer.Span));
    }

    private static void WithTempKey(Action<RegistryKey, string> body)
    {
        string subKey = @"Software\VrcResolver.Tests\" + Guid.NewGuid().ToString("N");
        string stateRoot = CreateTempStateRoot();
        try
        {
            using RegistryKey key = Registry.CurrentUser.CreateSubKey(subKey);
            body(key, Path.Combine(stateRoot, "wininet_limit_previous.txt"));
        }
        finally
        {
            Registry.CurrentUser.DeleteSubKeyTree(subKey, throwOnMissingSubKey: false);
            DeleteTempStateRoot(stateRoot);
        }
    }

    private static int FreePort()
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        int port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        return port;
    }

    private static string CreateTempStateRoot()
    {
        string path = Path.Combine(Path.GetTempPath(), "VrcResolver.Tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }

    private static void DeleteTempStateRoot(string path)
    {
        try { Directory.Delete(path, recursive: true); }
        catch { }
    }
}
