using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;
using Xunit.Abstractions;

namespace VrcResolver.Tests;

public sealed class LiveFactAttribute : FactAttribute
{
    public LiveFactAttribute()
    {
        if (Environment.GetEnvironmentVariable("VRCRESOLVER_LIVE_E2E") != "1")
            Skip = "set VRCRESOLVER_LIVE_E2E=1 to run against the live servers";
    }
}

[SupportedOSPlatform("windows")]
public sealed class LiveVrchatPathTests
{
    private const string AvProFormat = "(mp4/best)[height<=?4096][height>=?64][width>=?64]";
    private const string UnityFormat = "(mp4/best)[height<=?720][height>=?64][width>=?64]";
    private const int PlaysPerVideo = 3;
    private static readonly TimeSpan FetchDeadline = TimeSpan.FromSeconds(30);
    private static readonly JsonSerializerOptions s_json = new() { DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull };

    private readonly ITestOutputHelper _out;

    public LiveVrchatPathTests(ITestOutputHelper output) => _out = output;

    private static IEnumerable<(string Url, string Format)> Cases()
    {
        string spec = Environment.GetEnvironmentVariable("VRCRESOLVER_LIVE_URLS")
            ?? "https://www.youtube.com/watch?v=jNQXAC9IVRw;https://youtu.be/aqz-KE-bpKQ;https://soundcloud.com/forss/flickermood|avpro";
        foreach (string entry in spec.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            string[] parts = entry.Split('|', 2);
            string players = parts.Length > 1 ? parts[1] : "avpro,unity";
            if (players.Contains("avpro", StringComparison.Ordinal)) yield return (parts[0], AvProFormat);
            if (players.Contains("unity", StringComparison.Ordinal)) yield return (parts[0], UnityFormat);
        }
    }

    [LiveFact]
    public async Task EveryVideoResolvesThroughTheWatchdogAndStreamsFromTheServer()
    {
        WinInetClient.SetMaxConnectionsPerServer(2);
        await using var mesh = new MeshClient();
        await mesh.StartAsync();
        var sw = Stopwatch.StartNew();
        while (!mesh.IsConnected)
        {
            Assert.True(sw.Elapsed < TimeSpan.FromSeconds(30), "mesh never connected to " + ServerEndpoints.ProxyHost);
            await Task.Delay(100);
        }

        string pipeName = "vrcresolver.live." + Guid.NewGuid().ToString("N");
        using var ipc = new LocalIpcServer(mesh);
        ipc.StartForTests(pipeName);
        int relayPort = WinInetRelayTests.FreePort();
        var relay = new LocalRelayServer(relayPort, "http");
        relay.Start();
        using var avpro = new WinInetClient();
        var failures = new List<string>();

        try
        {
            foreach (var (video, format) in Cases())
            {
                string player = ResolveRequestProfile.InferPlayer(format);
                for (int play = 0; play < PlaysPerVideo; play++)
                {
                    string label = player + " " + video + " #" + play;
                    try
                    {
                        string url = await ResolveLikeTheWrapper(pipeName, video, player, format);
                        bool relayed = TrustGatewayUrlBuilder.TryBuild(relayPort, url, null, "http", out string vrchatUrl);
                        string fetchUrl = relayed ? vrchatUrl.Replace("//localhost.youtube.com:", "//127.0.0.1:", StringComparison.Ordinal) : url;
                        string detail = await Stream(avpro, fetchUrl);
                        if (play == 0 && player == WireConstants.PlayerAvPro)
                        {
                            string mf = await OpenInMediaFoundation(fetchUrl);
                            detail += " mf=" + mf;
                            if (!mf.StartsWith("state=Opened", StringComparison.Ordinal) || mf.Contains("audio=[]", StringComparison.Ordinal))
                                throw new InvalidDataException("media foundation could not open it: " + mf);
                        }
                        _out.WriteLine("OK   " + label + " via=" + (relayed ? "relay" : "direct") + " " + detail);
                    }
                    catch (Exception ex)
                    {
                        failures.Add(label + ": " + ex.GetType().Name + ": " + ex.Message);
                        _out.WriteLine("FAIL " + label + ": " + ex.Message);
                    }
                }
            }
        }
        finally
        {
            await relay.StopAsync();
            await ipc.StopAsync();
        }

        Assert.True(failures.Count == 0, string.Join(Environment.NewLine, failures));
    }

    private static async Task<string> ResolveLikeTheWrapper(string pipeName, string video, string player, string format)
    {
        ResolveRequest req = ResolveRequestProfile.BuildWrapperRequest(video, player, format);
        req.WrapperDeadlineMs = (int)ResolveBudget.Total.TotalMilliseconds - 1000;

        using var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
        await pipe.ConnectAsync(TimeSpan.FromSeconds(5), CancellationToken.None);
        await pipe.WriteAsync(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(req, s_json) + "\n"));
        using var reader = new StreamReader(pipe, Encoding.UTF8, leaveOpen: true);
        string? line = await reader.ReadLineAsync().WaitAsync(ResolveBudget.Total + TimeSpan.FromSeconds(5));
        var resp = JsonSerializer.Deserialize<ResolveResponse>(line ?? "null")
            ?? throw new InvalidDataException("empty pipe response");

        if (resp.Action != WireConstants.ActionResolved || string.IsNullOrEmpty(resp.Url))
            throw new InvalidDataException("watchdog answered " + resp.Action + " reason=" + resp.Reason + " message=" + resp.Message);
        if (!ResolvedUrlGuard.IsSafeToEmit(resp.Url))
            throw new InvalidDataException("wrapper would refuse to emit " + LogUtil.BareHost(resp.Url));
        return resp.Url;
    }

    private static async Task<string> Stream(WinInetClient avpro, string url)
    {
        byte[] head;
        string text;
        using (var req = await Fetch(avpro, url))
        {
            head = await Within(() => ReadPrefix(req, 2 * 1024 * 1024));
            text = Encoding.UTF8.GetString(head);
        }

        if (!text.StartsWith("#EXTM3U", StringComparison.Ordinal))
        {
            RequireMedia(head, url);
            return "progressive bytes=" + head.Length;
        }

        string playlistUrl = url;
        if (text.Contains("#EXT-X-STREAM-INF", StringComparison.Ordinal))
        {
            playlistUrl = Resolve(url, UriLines(text).First());
            using var req = await Fetch(avpro, playlistUrl);
            text = await Within(req.ReadText);
        }

        string[] media = UriLines(text).Take(2).ToArray();
        if (media.Length == 0) throw new InvalidDataException("media playlist has no segments");
        long total = 0;
        foreach (string seg in media)
        {
            using var req = await Fetch(avpro, Resolve(playlistUrl, seg));
            byte[] body = await Within(() => ReadPrefix(req, 4 * 1024 * 1024));
            RequireMedia(body, seg);
            total += body.Length;
        }
        return "hls segments=" + media.Length + " bytes=" + total;
    }

    private static async Task<WinInetClient.Request> Fetch(WinInetClient avpro, string url)
    {
        var req = await Within(() => avpro.Send(url));
        if (req.Status is not (200 or 206))
        {
            string body = Encoding.UTF8.GetString(ReadPrefix(req, 300));
            req.Dispose();
            throw new InvalidDataException("HTTP " + req.Status + " for " + url + " body=" + body);
        }
        return req;
    }

    private static byte[] ReadPrefix(WinInetClient.Request req, int max)
    {
        using var ms = new MemoryStream();
        byte[] buffer = new byte[64 * 1024];
        int n;
        while (ms.Length < max && (n = req.Read(buffer)) > 0) ms.Write(buffer, 0, n);
        return ms.ToArray();
    }

    private static void RequireMedia(byte[] body, string what)
    {
        if (body.Length < 188) throw new InvalidDataException("only " + body.Length + " bytes from " + what);
        bool ts = body[0] == 0x47;
        string box = Encoding.ASCII.GetString(body, 4, 4);
        bool mp4 = box is "ftyp" or "styp" or "moof" or "sidx";
        bool id3 = body[0] == 'I' && body[1] == 'D' && body[2] == '3';
        bool adtsOrMp3 = body[0] == 0xFF && (body[1] & 0xE0) == 0xE0;
        if (!(ts || mp4 || id3 || adtsOrMp3))
            throw new InvalidDataException("not a media payload, starts " + Convert.ToHexString(body, 0, 8));
    }

    private static IEnumerable<string> UriLines(string playlist)
    {
        foreach (string raw in playlist.Split('\n'))
        {
            string line = raw.Trim();
            if (line.StartsWith("#EXT-X-MAP:", StringComparison.Ordinal))
            {
                int start = line.IndexOf("URI=\"", StringComparison.Ordinal);
                if (start >= 0)
                {
                    start += 5;
                    yield return line[start..line.IndexOf('"', start)];
                }
            }
            else if (line.Length > 0 && !line.StartsWith('#'))
            {
                yield return line;
            }
        }
    }

    private static string Resolve(string baseUrl, string relative) => new Uri(new Uri(baseUrl), relative).ToString();

    private static async Task<T> Within<T>(Func<T> work)
        => await Task.Factory.StartNew(work, CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default)
            .WaitAsync(FetchDeadline);

    private static async Task<string> OpenInMediaFoundation(string url)
    {
        string script = """
            Add-Type -AssemblyName System.Runtime.WindowsRuntime
            $asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction' } | Select-Object -First 1
            [void][Windows.Media.Core.MediaSource,Windows.Media.Core,ContentType=WindowsRuntime]
            [void][Windows.Media.Playback.MediaPlaybackItem,Windows.Media.Playback,ContentType=WindowsRuntime]
            $src = [Windows.Media.Core.MediaSource]::CreateFromUri([Uri]$env:MF_URL)
            $item = New-Object Windows.Media.Playback.MediaPlaybackItem $src
            try { [void]$asTask.Invoke($null, @($src.OpenAsync())).Wait(30000) } catch { "open failed: $($_.Exception.InnerException.InnerException.Message)"; exit }
            $v = @($item.VideoTracks | ForEach-Object { $p = $_.GetEncodingProperties(); "$($p.Subtype)" }) -join ';'
            $a = @($item.AudioTracks | ForEach-Object { $p = $_.GetEncodingProperties(); "$($p.Subtype)" }) -join ';'
            "state=$($src.State) video=[$v] audio=[$a]"
            """;
        var psi = new ProcessStartInfo("powershell.exe", "-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand "
            + Convert.ToBase64String(Encoding.Unicode.GetBytes(script)))
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.Environment["MF_URL"] = url;
        using var proc = Process.Start(psi)!;
        Task<string> stdout = proc.StandardOutput.ReadToEndAsync();
        Task<string> stderr = proc.StandardError.ReadToEndAsync();
        await proc.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(60));
        string output = (await stdout).Trim();
        return output.Length > 0 ? output : "no output: " + (await stderr).Trim();
    }
}
