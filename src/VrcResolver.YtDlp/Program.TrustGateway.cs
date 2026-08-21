using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

internal static partial class Program
{
    // Trust-gateway URL wrap. The watchdog binds a local listener at
    // 127.0.0.1:{port} and writes relay_port.txt plus relay_scheme.txt to
    // %LOCALAPPDATA%Low\vrcresolver\. We rewrite the resolved URL to
    // `{scheme}://localhost.youtube.com:{port}/play/<session>/manifest.<ext>?target=<base64>`
    // so VRChat's AVPro allowlist (which has *.youtube.com) accepts it in
    // default-public worlds. The hosts file pins
    // `localhost.youtube.com -> 127.0.0.1` so the request lands on the
    // watchdog's listener which forwards bytes to the real URL.
    //
    // The path component carries a per-resolve namespace, a static
    // "manifest" placeholder, and the upstream URL's path extension.
    // AVPro/MediaFoundation dispatches its byte-stream handler primarily
    // on path extension (.m3u8 -> HLS, .mpd -> DASH); without a recognised
    // extension, MF mis-dispatches and playback stalls. The namespace lets
    // the relay forward relative playlist subrequests without parsing or
    // rewriting arbitrary manifest bodies.
    //
    // Failure modes -- ALL fall through to the raw URL (today's behavior):
    //   - port file missing (watchdog not running OR didn't bind)
    //   - port file unreadable / malformed
    //   - resolved URL is not a first-party playback proxy URL
    //   - URL is already wrapped (defensive; avoid double-wrap)
    private static string TryWrapForTrustGateway(string url, bool probeRelay = false)
    {
        if (string.IsNullOrEmpty(url)) return url;

        int? port = TryReadRelayPort();
        if (!port.HasValue) return url;
        // Direct-URL invocations reach here without a pipe roundtrip, so the
        // port file may have outlived the relay (watchdog killed hard). Probe
        // before wrapping; a dead localhost URL is a silent black player.
        // Pipe-resolved URLs skip the probe -- the resolve just proved the
        // watchdog alive, and it deletes the port file when the relay is down.
        if (probeRelay && !RelayLiveness.IsListening(port.Value)) return url;
        string scheme = TryReadRelayScheme();

        return TrustGatewayUrlBuilder.TryBuild(port.Value, url, session: null, scheme, out string localUrl)
            ? localUrl
            : url;
    }

    private static int? TryReadRelayPort()
    {
        try
        {
            string portFile = Path.Combine(AppPaths.StateRoot(), "relay_port.txt");
            if (!File.Exists(portFile)) return null;
            string text = File.ReadAllText(portFile).Trim();
            if (int.TryParse(text, System.Globalization.NumberStyles.Integer,
                    System.Globalization.CultureInfo.InvariantCulture, out int p)
                && p > 1024 && p < 65536) return p;
        }
        catch { /* best-effort; missing port = no wrap */ }
        return null;
    }

    private static string TryReadRelayScheme()
    {
        try
        {
            string schemeFile = Path.Combine(AppPaths.StateRoot(), "relay_scheme.txt");
            if (!File.Exists(schemeFile)) return "http";
            string text = File.ReadAllText(schemeFile).Trim();
            return TrustGatewayUrlBuilder.IsAllowedGatewayScheme(text) ? text.ToLowerInvariant() : "http";
        }
        catch { /* best-effort; bad scheme = existing http behaviour */ }
        return "http";
    }
}
