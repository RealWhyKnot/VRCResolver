using System.Text.Json;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class V3ProtocolTests
{
    [Fact]
    public void ClientHelloFrame_serializes_with_null_hash()
    {
        var hello = new ClientHelloFrame
        {
            WelcomeHash = null,
            ClientId = "abc123",
        };
        string json = JsonSerializer.Serialize(hello);
        Assert.Contains("\"welcome_hash\":null", json);
        Assert.Contains("\"action\":\"client_hello\"", json);
        Assert.Contains("\"client_id\":\"abc123\"", json);
    }

    [Fact]
    public void ClientHelloFrame_serializes_with_hash()
    {
        var hello = new ClientHelloFrame
        {
            WelcomeHash = "deadbeef0123",
            ClientId = "xyz",
        };
        string json = JsonSerializer.Serialize(hello);
        Assert.Contains("\"welcome_hash\":\"deadbeef0123\"", json);
    }

    [Fact]
    public void ClientHelloFrame_round_trip_preserves_extras()
    {
        string json = "{\"action\":\"client_hello\",\"welcome_hash\":\"abc\",\"client_id\":\"c\",\"future_field\":42}";
        var parsed = JsonSerializer.Deserialize<ClientHelloFrame>(json);
        Assert.NotNull(parsed);
        Assert.Equal("abc", parsed!.WelcomeHash);
        Assert.NotNull(parsed.Extra);
        Assert.True(parsed.Extra!.ContainsKey("future_field"));
    }

    [Fact]
    public void WelcomeCachedFrame_deserialize_minimal()
    {
        string json = "{\"action\":\"welcome_cached\",\"protocol_version\":3}";
        var f = JsonSerializer.Deserialize<WelcomeCachedFrame>(json);
        Assert.NotNull(f);
        Assert.Equal(3, f!.ProtocolVersion);
        Assert.Null(f.Node);
        Assert.Null(f.WarpActive);
    }

    [Fact]
    public void WelcomeCachedFrame_deserialize_with_warp_active()
    {
        string json = "{\"action\":\"welcome_cached\",\"protocol_version\":3,\"node\":\"node1\",\"warp_active\":true}";
        var f = JsonSerializer.Deserialize<WelcomeCachedFrame>(json);
        Assert.NotNull(f);
        Assert.Equal("node1", f!.Node);
        Assert.True(f.WarpActive);
    }

    [Fact]
    public void WelcomeFrame_with_welcome_hash_field_round_trips()
    {
        var welcome = new WelcomeFrame
        {
            ProtocolVersion = 3,
            Node = "node1",
            Engines = new[] { "yt-dlp" },
            Features = new[] { "v3_compression", "welcome_hash_ack" },
            WelcomeHash = "fingerprint",
            ServerVersion = "2026.5.4.7-3F2A",
        };
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(welcome);
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(bytes);
        Assert.NotNull(parsed);
        Assert.Equal("fingerprint", parsed!.WelcomeHash);
        Assert.Equal(3, parsed.ProtocolVersion);
        Assert.Contains("v3_compression", parsed.Features!);
    }

    [Fact]
    public void WelcomeFrame_v2_payload_round_trips_without_welcome_hash()
    {
        string json = "{\"action\":\"welcome\",\"protocol_version\":2,\"node\":\"node1\",\"engines\":[\"yt-dlp\"],\"features\":[]}";
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(json);
        Assert.NotNull(parsed);
        Assert.Null(parsed!.WelcomeHash);
        Assert.Equal(2, parsed.ProtocolVersion);
    }

    [Theory]
    [InlineData("vrcresolver-v3", true)]
    [InlineData(null, false)]
    [InlineData("", false)]
    [InlineData("whyknot-v3", false)]
    [InlineData("Vrcresolver-V3", false)]
    [InlineData("vrcresolver-v3 ", false)]
    public void ShouldSendClientHello_OnlyExactSubprotocolMatch(string? negotiated, bool expected)
    {
        Assert.Equal(expected, MeshClient.ShouldSendClientHello(negotiated));
    }

    [Fact]
    public void WireConstants_v3_strings_match_server_spec()
    {
        Assert.Equal("vrcresolver-v3", WireConstants.SubprotocolV3);
        Assert.Equal("client_hello", WireConstants.ActionClientHello);
        Assert.Equal("welcome_cached", WireConstants.ActionWelcomeCached);
        Assert.Equal("welcome_hash", WireConstants.FieldWelcomeHash);
        Assert.Equal(3, WireConstants.ClientProtocolVersion);
    }

    [Fact]
    public void WireConstants_v3_1_strings_match_server_spec()
    {
        Assert.Equal("accept_formats", WireConstants.FieldAcceptFormats);
        Assert.Equal("negotiated_format", WireConstants.FieldNegotiatedFormat);
        Assert.Equal("json", WireConstants.FormatJson);
        Assert.Equal("msgpack", WireConstants.FormatMsgpack);

        Assert.Equal(new[] { "msgpack", "json" }, WireConstants.AcceptFormatsPreference);
        Assert.Equal(new[] { "json" }, WireConstants.AcceptFormatsJsonOnly);
    }

    [Fact]
    public void WireConstants_welcome_hosts_and_feedback_v2_strings_match_server_spec()
    {
        Assert.Equal("welcome_hosts", WireConstants.FeatureWelcomeHosts);
        Assert.Equal("playback_feedback_v2", WireConstants.FeaturePlaybackFeedbackV2);
        Assert.Equal("resolved_rejected", WireConstants.PlaybackFeedbackResolvedRejected);
        Assert.Equal("og_failed", WireConstants.PlaybackFeedbackOgFailed);
        Assert.Equal("cache_play", WireConstants.PlaybackFeedbackCachePlay);
        Assert.Equal("protocol_error", WireConstants.ActionProtocolError);
        Assert.Equal("rate_limited", WireConstants.ActionRateLimited);
        Assert.Equal("rate_limited", WireConstants.FallbackRateLimited);
        Assert.Equal("protocol_error", WireConstants.FallbackProtocolError);
        Assert.Equal("retryAfterSeconds", WireConstants.FieldRetryAfterSeconds);
        Assert.Equal("meshAction", WireConstants.FieldMeshAction);
    }

    [Fact]
    public void WelcomeFrame_welcome_hosts_fields_round_trip()
    {
        string json = "{\"action\":\"welcome\",\"protocol_version\":3,\"node\":\"node1\","
            + "\"features\":[\"welcome_hosts\"],"
            + "\"first_party_hosts\":[\"vrcresolver.com\",\"whyknot.dev\"],"
            + "\"playback_proxy_paths\":[\"/api/proxy\",\"/api/popcorn/proxy\"]}";
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(json);
        Assert.NotNull(parsed);
        Assert.Equal(new[] { "vrcresolver.com", "whyknot.dev" }, parsed!.FirstPartyHosts);
        Assert.Equal(new[] { "/api/proxy", "/api/popcorn/proxy" }, parsed.PlaybackProxyPaths);
    }

    [Fact]
    public void ClientHelloFrame_serializes_with_accept_formats_msgpack_pref()
    {
        var hello = new ClientHelloFrame
        {
            WelcomeHash = null,
            ClientId = "id",
            AcceptFormats = WireConstants.AcceptFormatsPreference,
        };
        string json = JsonSerializer.Serialize(hello);
        Assert.Contains("\"accept_formats\":[\"msgpack\",\"json\"]", json);
    }

    [Fact]
    public void ClientHelloFrame_serializes_with_accept_formats_json_only()
    {
        var hello = new ClientHelloFrame
        {
            WelcomeHash = "h",
            ClientId = "id",
            AcceptFormats = WireConstants.AcceptFormatsJsonOnly,
        };
        string json = JsonSerializer.Serialize(hello);
        Assert.Contains("\"accept_formats\":[\"json\"]", json);
    }

    [Fact]
    public void ClientHelloFrame_v3_0_shape_omits_accept_formats_when_null()
    {
        var hello = new ClientHelloFrame
        {
            WelcomeHash = "h",
            ClientId = "id",
        };
        string json = JsonSerializer.Serialize(hello);
        Assert.DoesNotContain("\"msgpack\"", json);
    }

    [Fact]
    public void WelcomeFrame_negotiated_format_round_trips()
    {
        var welcome = new WelcomeFrame
        {
            ProtocolVersion = 3,
            Node = "node1",
            NegotiatedFormat = "msgpack",
        };
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(welcome);
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(bytes);
        Assert.NotNull(parsed);
        Assert.Equal("msgpack", parsed!.NegotiatedFormat);
    }

    [Fact]
    public void WelcomeFrame_v3_0_payload_round_trips_without_negotiated_format()
    {
        string json = "{\"action\":\"welcome\",\"protocol_version\":3,\"node\":\"node1\"}";
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(json);
        Assert.NotNull(parsed);
        Assert.Null(parsed!.NegotiatedFormat);
    }

    [Fact]
    public void WelcomeCachedFrame_negotiated_format_round_trips()
    {
        string json = "{\"action\":\"welcome_cached\",\"protocol_version\":3,\"node\":\"node1\",\"warp_active\":true,\"negotiated_format\":\"msgpack\"}";
        var f = JsonSerializer.Deserialize<WelcomeCachedFrame>(json);
        Assert.NotNull(f);
        Assert.Equal("msgpack", f!.NegotiatedFormat);
    }
}
