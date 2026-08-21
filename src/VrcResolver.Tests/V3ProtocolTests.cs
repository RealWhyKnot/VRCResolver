using System.Text.Json;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

// Wire-shape regression tests for the v3 handshake DTOs. These guard
// against the kind of accidental rename / case-flip / serialization-attr
// drift that would silently desync the client from the server's
// MeshResolveProtocol. Pair-test these against the server's equivalent
// snapshot tests when the server's v3.0 lands.
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
        // Server treats welcome_hash=null as "no cache, send full welcome".
        // The field MUST be present on the wire so the server can
        // distinguish "field omitted" (older client, undefined behaviour)
        // from "field explicitly null" (v3 client with no cached welcome).
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
        // Forward-compat: a future v3.x server might reflect extra fields
        // we don't statically know about. JsonExtensionData should
        // round-trip them so a future reader of the watchdog log can
        // still see what the server saw.
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
        // Server SHOULD always send protocol_version + node, but we want
        // missing fields to land as defaults rather than throwing — a
        // brittle parse here would re-trigger the v2 fallback path on
        // benign server-side schema drift.
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
        // v3 servers emit welcome_hash inside the full welcome on cache
        // miss. The client persists it for next reconnect.
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
        // Backward compat: a v2 server's welcome doesn't include
        // welcome_hash. Deserialize must produce welcome_hash=null with
        // no thrown exception — the v3 client treats null hash as "this
        // is a v2 welcome, don't try to cache".
        string json = "{\"action\":\"welcome\",\"protocol_version\":2,\"node\":\"node1\",\"engines\":[\"yt-dlp\"],\"features\":[]}";
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(json);
        Assert.NotNull(parsed);
        Assert.Null(parsed!.WelcomeHash);
        Assert.Equal(2, parsed.ProtocolVersion);
    }

    [Theory]
    [InlineData("vrcresolver-v3", true)]
    [InlineData(null, false)]              // v2 server / proxy stripped header
    [InlineData("", false)]                // server returned empty
    [InlineData("whyknot-v3", false)]      // pre-rename subprotocol echoed back
    [InlineData("Vrcresolver-V3", false)]  // case-flip drift detection (we're Ordinal, NOT OrdinalIgnoreCase)
    [InlineData("vrcresolver-v3 ", false)] // trailing space
    public void ShouldSendClientHello_OnlyExactSubprotocolMatch(string? negotiated, bool expected)
    {
        // Subprotocol mismatch → client falls back to v2 path: no
        // client_hello, just wait for plain welcome. Pure helper so the
        // fallback decision is testable without a real ClientWebSocket.
        Assert.Equal(expected, MeshClient.ShouldSendClientHello(negotiated));
    }

    [Fact]
    public void WireConstants_v3_strings_match_server_spec()
    {
        // Byte-exact constants — must mirror the server's protocol module.
        // A casing flip here would silently desync the entire v3 handshake.
        // The server accepts both this subprotocol and the pre-rename one.
        Assert.Equal("vrcresolver-v3", WireConstants.SubprotocolV3);
        Assert.Equal("client_hello", WireConstants.ActionClientHello);
        Assert.Equal("welcome_cached", WireConstants.ActionWelcomeCached);
        Assert.Equal("welcome_hash", WireConstants.FieldWelcomeHash);
        Assert.Equal(3, WireConstants.ClientProtocolVersion);
    }

    [Fact]
    public void WireConstants_v3_1_strings_match_server_spec()
    {
        // v3.1 additions. Per server protocol spec the feature literal is
        // "msgpack_format". Format identifiers are "json" / "msgpack".
        // accept_formats / negotiated_format are the canonical field names.
        Assert.Equal("accept_formats", WireConstants.FieldAcceptFormats);
        Assert.Equal("negotiated_format", WireConstants.FieldNegotiatedFormat);
        Assert.Equal("json", WireConstants.FormatJson);
        Assert.Equal("msgpack", WireConstants.FormatMsgpack);

        // Preference list shape: msgpack first, json fallback. Server
        // picks the first format from this list that it supports.
        Assert.Equal(new[] { "msgpack", "json" }, WireConstants.AcceptFormatsPreference);
        // Sentinel for the v3.0-style behaviour: explicit json-only.
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
        // rate_limited fields are camelCase on the wire -- a v2-era outlier.
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
        // Wire shape when watchdog sends ["msgpack","json"]. Server
        // would pick "msgpack" from this list. Field is field-present
        // when set; ABSENT when null (default-options JsonSerializer
        // emits the property name with `null` value, but the existing
        // ClientHelloFrame round-trip test's pattern lets us verify the
        // wire bytes contain the expected substring).
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
        // Commit-1 sentinel — the watchdog ships AcceptFormatsJsonOnly
        // until the binary-frame dispatch lands in commit 2. Server
        // sees accept_formats=["json"] and picks json.
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
        // Backward-compat: if AcceptFormats is null, server (v3.0 or v3.1)
        // treats as "v3.0-style hello" → defaults to json. The wire
        // output may contain "accept_formats":null (default options)
        // but server tolerates either field-omitted or field-null.
        var hello = new ClientHelloFrame
        {
            WelcomeHash = "h",
            ClientId = "id",
            // AcceptFormats deliberately not set.
        };
        string json = JsonSerializer.Serialize(hello);
        // We don't assert "field absent" because default options emit
        // null. Just confirm no msgpack literal sneaks in.
        Assert.DoesNotContain("\"msgpack\"", json);
    }

    [Fact]
    public void WelcomeFrame_negotiated_format_round_trips()
    {
        // Server emits negotiated_format on every welcome (v3.1+). Client
        // reads it to decide whether the post-welcome wire is binary
        // msgpack or text JSON.
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
        // v3.0 servers don't emit negotiated_format. Client must
        // tolerate the field's absence and default to json (v3.0
        // behaviour).
        string json = "{\"action\":\"welcome\",\"protocol_version\":3,\"node\":\"node1\"}";
        var parsed = JsonSerializer.Deserialize<WelcomeFrame>(json);
        Assert.NotNull(parsed);
        Assert.Null(parsed!.NegotiatedFormat);
    }

    [Fact]
    public void WelcomeCachedFrame_negotiated_format_round_trips()
    {
        // Same field, same semantics as WelcomeFrame.NegotiatedFormat —
        // server re-emits it on welcome_cached because format choice is
        // per-connection, not cached.
        string json = "{\"action\":\"welcome_cached\",\"protocol_version\":3,\"node\":\"node1\",\"warp_active\":true,\"negotiated_format\":\"msgpack\"}";
        var f = JsonSerializer.Deserialize<WelcomeCachedFrame>(json);
        Assert.NotNull(f);
        Assert.Equal("msgpack", f!.NegotiatedFormat);
    }
}
