using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public sealed class TerminalQuickEditTests
{
    [Theory]
    [InlineData(0x01F7u, 0x01B7u)]
    [InlineData(0x0040u, 0x0080u)]
    [InlineData(0x0007u, 0x0087u)]
    public void WithoutQuickEdit_ClearsOnlyQuickEditAndKeepsExtendedFlags(uint mode, uint expected)
    {
        Assert.Equal(expected, TerminalCapabilities.WithoutQuickEdit(mode));
    }

    [Fact]
    public void DisableQuickEdit_NeverThrowsAndReportsWhatHappened()
    {
        Assert.Contains(TerminalCapabilities.DisableQuickEdit(), new[] { "disabled", "already_off", "unavailable", "failed" });
    }
}
