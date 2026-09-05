using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public sealed class TerminalFailureLogTests
{
    [Fact]
    public void CombinedRecordContainsBothFailureReasonsAndTiming()
    {
        string line = TerminalFailureLog.Build(
            "abc12345", "youtube.com", "avpro",
            "validation_failed", "sign_in_required", 12345);

        Assert.Equal(
            "terminal_failure request_id=abc12345 domain=youtube.com player=avpro "
            + "server_reason=validation_failed bundled_ytdlp_reason=sign_in_required elapsed_ms=12345",
            line);
    }
}
