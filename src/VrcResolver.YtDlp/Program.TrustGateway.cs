using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

internal static partial class Program
{
    private static string TryWrapForTrustGateway(string url, bool probeRelay = false)
    {
        if (string.IsNullOrEmpty(url)) return url;

        int? port = TryReadRelayPort();
        if (!port.HasValue) return url;
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
        catch { }
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
        catch { }
        return "http";
    }
}
