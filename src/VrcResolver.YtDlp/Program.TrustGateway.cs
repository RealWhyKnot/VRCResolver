using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

internal static partial class Program
{
    private static string TryWrapForTrustGateway(string url, bool probeRelay = false)
        => TrustGatewayUrlBuilder.WrapForRelay(AppPaths.StateRoot(), url, probeRelay);
}
