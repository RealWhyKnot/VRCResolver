using System.Linq;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public sealed class TerminalCompletionTests
{
    private static TerminalCommandRegistry Registry() => TerminalCommandRegistry.CreateDefault();

    [Theory]
    [InlineData("stat", "us ")]
    [InlineData("/stat", "us ")]
    [InlineData("quie", "")]
    public void Suggest_CompletesOnlyWhenUnambiguous(string input, string expected)
    {
        Assert.Equal(expected, Registry().Suggest(input));
    }

    [Fact]
    public void Suggest_IsEmptyWhenSeveralCommandsMatch()
    {
        Assert.Equal("", Registry().Suggest("s"));
    }

    [Fact]
    public void Suggest_IsEmptyAfterTrailingSpace()
    {
        Assert.Equal("", Registry().Suggest("status "));
    }

    [Fact]
    public void Suggest_AppendedToInputYieldsTheFullCommand()
    {
        const string typed = "diagn";
        string ghost = Registry().Suggest(typed);

        Assert.Equal("diagnostics ", typed + ghost);
    }

    [Theory]
    [InlineData("stat", "status")]
    [InlineData("staus", "status")]
    [InlineData("setings", "settings")]
    [InlineData("hlep", "help")]
    public void NearestCommands_RecoversFromTypos(string typo, string expected)
    {
        var nearest = Registry().NearestCommands(typo);

        Assert.NotEmpty(nearest);
        Assert.Equal(expected, nearest[0].Text);
    }

    [Fact]
    public void NearestCommands_GivesUpOnNonsense()
    {
        Assert.Empty(Registry().NearestCommands("zzzzzzzzzzz"));
    }

    [Fact]
    public void NearestCommands_ResolvesThroughAliases()
    {
        var nearest = Registry().NearestCommands("dashbord");

        Assert.NotEmpty(nearest);
        Assert.Equal("status", nearest[0].Text);
    }

    [Fact]
    public void HelpArguments_CompleteToCommandNames()
    {
        TerminalCompletion completion = Registry().Complete("help sett");

        Assert.Equal("help settings", completion.Replacement);
    }

    [Fact]
    public void HelpArguments_ListEveryCommandWhenEmpty()
    {
        TerminalCompletion completion = Registry().Complete("help ");

        Assert.Contains(completion.Suggestions, s => s.Text == "status");
        Assert.Contains(completion.Suggestions, s => s.Text == "settings");
    }

    [Fact]
    public void CommandsWithoutACompleterOfferNothing()
    {
        Assert.Empty(Registry().Complete("clear ").Suggestions);
        Assert.Equal("", Registry().Complete("clear ").Replacement);
    }

    [Fact]
    public void SettingsArgumentsStillComplete()
    {
        TerminalCompletion completion = Registry().Complete("settings high-q");

        Assert.Contains("high-quality", completion.Replacement);
    }

    [Fact]
    public void HighQualitySettingIsRegisteredAndOffByDefault()
    {
        Assert.True(AppSettingsRegistry.TryFind("high-quality", out AppSettingDefinition? setting));
        Assert.NotNull(setting);
        Assert.Equal("off", setting!.Get(new AppSettings()));

        Assert.True(AppSettingsRegistry.TryFind("hq", out AppSettingDefinition? viaAlias));
        Assert.Equal("high-quality", viaAlias!.Key);
    }

    [Fact]
    public void HighQualitySettingRoundTripsThroughTheModel()
    {
        var settings = new AppSettings();
        Assert.True(AppSettingsRegistry.TryFind("high-quality", out AppSettingDefinition? setting));

        Assert.True(setting!.TrySet(settings, "on", out string error));
        Assert.Equal("", error);
        Assert.True(settings.Playback.HighQuality);
        Assert.True(settings.Clone().Playback.HighQuality);

        setting.Reset(settings);
        Assert.False(settings.Playback.HighQuality);
    }
}
