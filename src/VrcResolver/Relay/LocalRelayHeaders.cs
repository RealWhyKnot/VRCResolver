using System.Net;
using System.Runtime.Versioning;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class LocalRelayHeaders
{
    // Upstream response headers that must not be re-emitted to AVPro. The
    // relay pretends to be localhost.youtube.com, but the actual upstream is
    // Cloudflare in front of the server. CF-specific headers and Alt-Svc do
    // not belong on the local trusted hostname.
    public static bool ShouldDropResponseHeader(string name)
    {
        if (string.Equals(name, "Transfer-Encoding", StringComparison.OrdinalIgnoreCase)) return true;
        if (string.Equals(name, "Alt-Svc", StringComparison.OrdinalIgnoreCase)) return true;
        if (string.Equals(name, "Server", StringComparison.OrdinalIgnoreCase)) return true;
        if (string.Equals(name, "cf-cache-status", StringComparison.OrdinalIgnoreCase)) return true;
        if (string.Equals(name, "Speculation-Rules", StringComparison.OrdinalIgnoreCase)) return true;
        if (string.Equals(name, "Report-To", StringComparison.OrdinalIgnoreCase)) return true;
        if (string.Equals(name, "Nel", StringComparison.OrdinalIgnoreCase)) return true;
        if (name.StartsWith("CF-", StringComparison.OrdinalIgnoreCase)) return true;
        return false;
    }

    public static void ForwardRequestHeaders(HttpListenerRequest src, HttpRequestMessage dst)
    {
        foreach (string? key in src.Headers.AllKeys)
        {
            if (string.IsNullOrEmpty(key)) continue;
            if (key.Equals("Host", StringComparison.OrdinalIgnoreCase)) continue;
            if (key.Equals("Connection", StringComparison.OrdinalIgnoreCase)) continue;
            if (key.Equals("Cookie", StringComparison.OrdinalIgnoreCase)) continue;
            if (key.Equals("Authorization", StringComparison.OrdinalIgnoreCase)) continue;
            if (key.Equals("Accept-Encoding", StringComparison.OrdinalIgnoreCase)) continue;
            if (key.Equals("Content-Length", StringComparison.OrdinalIgnoreCase)) continue;
            if (key.Equals("Content-Type", StringComparison.OrdinalIgnoreCase)) continue;

            string? value = src.Headers[key];
            if (value == null) continue;
            dst.Headers.TryAddWithoutValidation(key, value);
        }

        // Keep local relay responses byte-simple. The local hop is loopback, so
        // compressed manifests save nothing and make first-party URL localization
        // unsafe unless the relay also owns every content-encoding edge case.
        dst.Headers.TryAddWithoutValidation("Accept-Encoding", "identity");
    }
}
