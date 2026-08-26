using System.IO;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

// Regression cover for a prompt that appeared frozen while typing. Ghost text makes the rendered
// string invariant as the user types into a suggestion, so a change detector that only compares
// text suppresses every repaint and nothing the user types ever appears.
public sealed class TerminalGhostRepaintTests
{
    private static TerminalFrame PromptFrame(string input, string ghost)
    {
        var runs = new[]
        {
            new TerminalTextRun("vrcr> ", TerminalStyle.Bright),
            new TerminalTextRun(input, TerminalStyle.Plain),
            new TerminalTextRun(ghost, TerminalStyle.Dim),
        };
        return new TerminalFrame(runs) { CursorColumn = 6 + input.Length };
    }

    private static void Write(TextWriter w, System.Collections.Generic.IReadOnlyList<TerminalTextRun> runs)
    {
        foreach (var r in runs) w.Write(r.Text);
    }

    [Fact]
    public void TypingIntoAGhostStillRepaints()
    {
        var line = new TerminalOverlayLine();
        var writer = new StringWriter();

        // Every one of these renders the identical string "vrcr> status ".
        Assert.True(line.RenderIfChanged(writer, PromptFrame("sta", "tus "), Write));
        Assert.True(line.RenderIfChanged(writer, PromptFrame("stat", "us "), Write));
        Assert.True(line.RenderIfChanged(writer, PromptFrame("statu", "s "), Write));
        Assert.True(line.RenderIfChanged(writer, PromptFrame("status", " "), Write));
    }

    [Fact]
    public void TheRenderedTextReallyIsIdenticalAcrossThoseKeystrokes()
    {
        // If this ever stops being true the test above stops covering the bug it was written for.
        Assert.Equal(PromptFrame("sta", "tus ").PlainText, PromptFrame("stat", "us ").PlainText);
    }

    [Fact]
    public void AnUnchangedFrameStillSkipsTheRepaint()
    {
        var line = new TerminalOverlayLine();
        var writer = new StringWriter();

        Assert.True(line.RenderIfChanged(writer, PromptFrame("sta", "tus "), Write));
        Assert.False(line.RenderIfChanged(writer, PromptFrame("sta", "tus "), Write));
    }

    [Fact]
    public void MovingTheCaretWithoutChangingTextRepaints()
    {
        var line = new TerminalOverlayLine();
        var writer = new StringWriter();
        var atEnd = PromptFrame("status", "");
        var midLine = new TerminalFrame(atEnd.Runs) { CursorColumn = 8 };

        Assert.True(line.RenderIfChanged(writer, atEnd, Write));
        Assert.True(line.RenderIfChanged(writer, midLine, Write));
    }

    [Fact]
    public void FramesWithoutACaretKeepTheOldTextOnlyBehaviour()
    {
        var line = new TerminalOverlayLine();
        var writer = new StringWriter();

        Assert.True(line.RenderIfChanged(writer, TerminalFrame.Plain("idle"), Write));
        Assert.False(line.RenderIfChanged(writer, TerminalFrame.Plain("idle"), Write));
        Assert.True(line.RenderIfChanged(writer, TerminalFrame.Plain("busy"), Write));
    }
}
