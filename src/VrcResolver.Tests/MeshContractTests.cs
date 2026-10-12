using System.Reflection;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using MessagePack;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public sealed class MeshContractTests : IDisposable
{
    private const string Node = "vrcresolver.com";
    private const string ServerFixtureRelativePath =
        "../../Deployed/vrcresolver-server/apex/src/VrcResolver.Backend.Tests/Fixtures/mesh-contract.json";

    private readonly string _dir = Path.Combine(Path.GetTempPath(), "vrcresolver-contract-" + Guid.NewGuid().ToString("N"));
    private readonly JsonDocument _contract;

    public MeshContractTests()
    {
        Directory.CreateDirectory(_dir);
        _contract = JsonDocument.Parse(File.ReadAllBytes(FixturePath()));
    }

    public void Dispose()
    {
        _contract.Dispose();
        try { Directory.Delete(_dir, true); } catch { }
    }

    private static string FixturePath()
        => Path.Combine(AppContext.BaseDirectory, "Fixtures", "mesh-contract.json");

    private JsonElement Root => _contract.RootElement;

    private string[] Strings(string name)
        => Root.GetProperty(name).EnumerateArray().Select(e => e.GetString()!).ToArray();

    private int Number(string section, string name)
        => Root.GetProperty(section).GetProperty(name).GetInt32();

    private string JsonSample(string action)
        => Root.GetProperty("json_samples").GetProperty(action).GetString()!;

    private byte[] MsgpackSample(string action)
        => Convert.FromHexString(Root.GetProperty("msgpack_samples_hex").GetProperty(action).GetString()!);

    private static string[] Sorted(IEnumerable<string> values)
        => values.OrderBy(v => v, StringComparer.Ordinal).ToArray();

    private static string? Text(JsonElement element, string name)
        => element.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString()
            : null;

    private static void Set(MeshClient client, string name, object value)
    {
        var field = typeof(MeshClient).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        field!.SetValue(client, value);
    }

    private MeshClient NewClient()
    {
        var cache = new WelcomeCache(Path.Combine(_dir, "v3_welcome_cache.json"));
        cache.Store(Node, new WelcomeFrame { ProtocolVersion = 3, Node = "node1" }, "hash1");
        var client = new MeshClient();
        Set(client, "_welcomeCache", cache);
        Set(client, "_isV3Connection", true);
        Set(client, "_currentNodeHost", Node);
        return client;
    }

    private static Task DispatchAsync(MeshClient client, string json)
    {
        var method = typeof(MeshClient).GetMethod("DispatchFrameAsync", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);
        return (Task)method!.Invoke(client, new object[] { Encoding.UTF8.GetBytes(json), CancellationToken.None })!;
    }

    private static MessagePackSerializerOptions ClientMsgpackOptions()
    {
        var field = typeof(MeshClient).GetField("s_msgpackOpts", BindingFlags.Static | BindingFlags.NonPublic);
        Assert.NotNull(field);
        return (MessagePackSerializerOptions)field!.GetValue(null)!;
    }

    [Fact]
    public void Subprotocol_matches_the_server()
    {
        string current = Root.GetProperty("subprotocols").GetProperty("current").GetString()!;
        string legacy = Root.GetProperty("subprotocols").GetProperty("legacy").GetString()!;

        Assert.Equal(WireConstants.SubprotocolV3, current);
        Assert.Equal(WireConstants.ClientProtocolVersion, Root.GetProperty("protocol_version").GetInt32());
        Assert.True(MeshClient.ShouldSendClientHello(current));
        Assert.False(MeshClient.ShouldSendClientHello(legacy));
    }

    [Fact]
    public async Task Every_action_the_watchdog_receives_is_handled()
    {
        var samples = Root.GetProperty("json_samples").EnumerateObject().ToArray();
        Assert.Equal(Sorted(Strings("watchdog_received_actions")), Sorted(samples.Select(s => s.Name)));

        var original = Console.Out;
        var problems = new List<string>();
        try
        {
            foreach (var sample in samples)
            {
                var captured = new StringWriter();
                Console.SetOut(captured);
                await using var client = NewClient();

                await DispatchAsync(client, sample.Value.GetString()!);

                string output = captured.ToString();
                if (output.Contains("unknown action", StringComparison.Ordinal)
                    || output.Contains("parse failed", StringComparison.Ordinal))
                {
                    problems.Add(sample.Name + ": " + output.Trim());
                }
            }
        }
        finally
        {
            Console.SetOut(original);
            FirstPartyUrlPolicy.ResetServerProvidedForTests();
        }

        Assert.Empty(problems);
    }

    [Fact]
    public void Client_fallback_reasons_exist_on_the_server()
    {
        string[] clientReasons =
        {
            WireConstants.FallbackAllConfigsFailed,
            WireConstants.FallbackExtractorUnsupported,
            WireConstants.FallbackInternalError,
            WireConstants.FallbackDiscoveryInProgress,
            WireConstants.ReasonUnityUnsupportedFormat,
            WireConstants.ReasonWarpDown,
        };

        Assert.Empty(clientReasons.Except(Strings("fallback_reasons"), StringComparer.Ordinal));
    }

    [Fact]
    public void Playback_feedback_kinds_match_the_server()
    {
        string[] kinds =
        {
            WireConstants.PlaybackFeedbackLoadFailure,
            WireConstants.PlaybackFeedbackSilentStall,
            WireConstants.PlaybackFeedbackPlaying,
            WireConstants.PlaybackFeedbackResolvedRejected,
            WireConstants.PlaybackFeedbackOgFailed,
            WireConstants.PlaybackFeedbackCachePlay,
        };

        Assert.Equal(Sorted(Strings("playback_feedback_kinds")), Sorted(kinds));
    }

    [Fact]
    public void Resolved_rejected_details_match_the_server()
    {
        string[] details =
        {
            WireConstants.OgFallbackReasonAvProIncompatible,
            WireConstants.OgFallbackReasonResolvedUrlRejected,
        };

        Assert.Equal(Sorted(Strings("resolved_rejected_details")), Sorted(details));
    }

    [Fact]
    [SupportedOSPlatform("windows")]
    public void Og_failed_details_match_the_server()
    {
        Assert.Equal(Sorted(Strings("og_failed_details")), Sorted(LocalIpcServer.OgFailedDetails));
        Assert.All(LocalIpcServer.OgFailedDetails, detail => Assert.Equal(detail, LocalIpcServer.OgFailedDetailFor(detail)));
        Assert.Equal(LocalIpcServer.OgFailedDetailUnknown, LocalIpcServer.OgFailedDetailFor("something_else"));
    }

    [Fact]
    [SupportedOSPlatform("windows")]
    public void Delivered_height_bounds_match_the_server()
    {
        Assert.Equal(VrcLogMonitor.MinDeliveredHeight, Number("delivered_height", "min"));
        Assert.Equal(VrcLogMonitor.MaxDeliveredHeight, Number("delivered_height", "max"));
    }

    [Fact]
    public void Proxy_extensions_match_the_server()
    {
        Assert.Equal(Sorted(Strings("proxy_path_extensions")), Sorted(FirstPartyUrlPolicy.AllowedPathExtensions));
    }

    [Fact]
    public void Client_needed_features_are_listed_by_the_server()
    {
        string[] needed =
        {
            WireConstants.FeaturePlaybackFeedback,
            WireConstants.FeaturePlaybackFeedbackV2,
            WireConstants.FeatureWelcomeHosts,
        };

        Assert.Equal(Sorted(needed), Sorted(Strings("client_required_features")));
        Assert.Empty(needed.Except(Strings("server_features"), StringComparer.Ordinal));

        var welcome = JsonSerializer.Deserialize(JsonSample("welcome"), MeshJsonContext.Default.WelcomeFrame);
        Assert.NotNull(welcome);
        Assert.NotNull(welcome!.Features);
        Assert.Empty(needed.Except(welcome.Features!, StringComparer.Ordinal));
    }

    [Fact]
    public void Msgpack_samples_decode_with_client_types()
    {
        Assert.Equal(
            Sorted(new[] { WireConstants.ActionResolved, WireConstants.ActionFallbackNative, WireConstants.ActionResolveLog }),
            Sorted(Root.GetProperty("msgpack_samples_hex").EnumerateObject().Select(p => p.Name)));
        var options = ClientMsgpackOptions();

        using (var json = JsonDocument.Parse(JsonSample(WireConstants.ActionResolved)))
        {
            var expected = json.RootElement;
            var frame = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(MsgpackSample(WireConstants.ActionResolved), options);

            Assert.NotNull(frame);
            Assert.Equal(Text(expected, "action"), frame!.Action);
            Assert.Equal(Text(expected, "id"), frame.Id);
            Assert.Equal(Text(expected, "url"), frame.Url);
            Assert.Equal(Text(expected, "engine"), frame.Engine);
            Assert.Equal(Text(expected, "config"), frame.Config);
            Assert.Equal(Text(expected, "container"), frame.Container);
            Assert.Equal(Text(expected, "video_codec"), frame.VideoCodec);
            Assert.Equal(Text(expected, "audio_codec"), frame.AudioCodec);
            Assert.Equal(Text(expected, "protocol"), frame.Protocol);
            Assert.Equal(expected.GetProperty("audio_channels").GetInt32(), frame.AudioChannels);
            Assert.Equal(expected.GetProperty("bytes_estimate").GetInt64(), frame.BytesEstimate);
            Assert.Equal(Text(expected, "expires_at"), frame.ExpiresAt);
            Assert.Equal(expected.GetProperty("resolved_height").GetInt32(), frame.ResolvedHeight);
        }

        using (var json = JsonDocument.Parse(JsonSample(WireConstants.ActionFallbackNative)))
        {
            var expected = json.RootElement;
            var frame = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(MsgpackSample(WireConstants.ActionFallbackNative), options);

            Assert.NotNull(frame);
            Assert.Equal(Text(expected, "action"), frame!.Action);
            Assert.Equal(Text(expected, "id"), frame.Id);
            Assert.Equal(Text(expected, "reason"), frame.Reason);
            Assert.Equal(Text(expected, "public_message"), frame.PublicMessage);
            Assert.Equal(expected.GetProperty("retry_after_ms").GetInt32(), frame.RetryAfterMs);
        }

        using (var json = JsonDocument.Parse(JsonSample(WireConstants.ActionResolveLog)))
        {
            var expected = json.RootElement;
            var frame = MessagePackSerializer.Deserialize<MsgpackResolveLogFrame>(MsgpackSample(WireConstants.ActionResolveLog), options);

            Assert.NotNull(frame);
            Assert.Equal(Text(expected, "action"), frame!.Action);
            Assert.Equal(Text(expected, "id"), frame.Id);
            Assert.Equal(Text(expected, "message"), frame.Message);
        }
    }

    [Fact]
    [SupportedOSPlatform("windows")]
    public void Deadlines_nest_with_the_server_window()
    {
        int margin = Number("resolve_window", "deadline_margin_ms");
        int minWindow = Number("resolve_window", "min_ms");
        int maxWindow = Number("resolve_window", "max_ms");
        var now = DateTime.UtcNow;
        var violations = new List<string>();

        for (int budget = 2_000; budget <= 28_000; budget += 500)
        {
            int sent = budget - 1_000;
            int watchdog = LocalIpcServer.EffectiveBudgetMs(sent);
            int wire = MeshClient.WireDeadlineFor(now.AddMilliseconds(watchdog), now);
            int server = Math.Clamp(wire - margin, minWindow, maxWindow);
            string line = "budget=" + budget + " sent=" + sent + " watchdog=" + watchdog + " wire=" + wire + " server=" + server;

            if (watchdog >= budget)
                violations.Add("watchdog does not finish before the wrapper: " + line);
            if (budget >= 7_000 && server >= watchdog)
                violations.Add("server window does not finish before the watchdog: " + line);
        }

        Assert.Empty(violations);
    }

    [Fact]
    public void Fixture_is_a_copy_of_the_server_fixture()
    {
        string? root = ClientRepoRoot();
        if (root == null) return;

        string serverCopy = Path.GetFullPath(Path.Combine(root, ServerFixtureRelativePath));
        if (!File.Exists(serverCopy)) return;

        Assert.Equal(ReadNormalized(serverCopy), ReadNormalized(FixturePath()));
    }

    private static string? ClientRepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir != null)
        {
            if (File.Exists(Path.Combine(dir.FullName, "vrcresolver.slnx"))) return dir.FullName;
            dir = dir.Parent;
        }
        return null;
    }

    private static string ReadNormalized(string path)
        => new UTF8Encoding(false).GetString(File.ReadAllBytes(path)).Replace("\r\n", "\n");
}
