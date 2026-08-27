using System.Runtime.Versioning;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class LocalIpcServerLogicTests
{
    [Theory]
    [InlineData(null, "resolved", null, true)]
    [InlineData(null, "fallback_native", "all_configs_failed", true)]
    [InlineData(null, "fallback_native", "rate_limited", true)]
    [InlineData(null, "fallback_native", "protocol_error", true)]
    [InlineData(null, "fallback_native", "warp_down", true)]
    [InlineData(null, "fallback_native", "server_unreachable", false)]
    [InlineData(null, "fallback_native", "internal_error", false)]
    [InlineData("server_unreachable", "fallback_native/server_unreachable", null, false)]
    public void IsHealthyOutcome_CountsOnlySynthesizedFailuresAsUnhealthy(
        string? failReason, string outcome, string? serverReason, bool expected)
        => Assert.Equal(expected, LocalIpcServer.IsHealthyOutcome(failReason, outcome, serverReason));

    [Theory]
    [InlineData("cf_403", "cf_403")]
    [InlineData("rate_limited", "rate_limited")]
    [InlineData("sign_in_required", "sign_in_required")]
    [InlineData("content_not_found", "content_not_found")]
    [InlineData("unknown", "unknown")]
    [InlineData("something_else", "unknown")]
    [InlineData(null, "unknown")]
    public void OgFailedDetailFor_CollapsesToTheServerSet(string? wrapperReason, string expected)
        => Assert.Equal(expected, LocalIpcServer.OgFailedDetailFor(wrapperReason));
}
