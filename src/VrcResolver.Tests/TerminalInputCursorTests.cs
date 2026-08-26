using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public sealed class TerminalInputCursorTests
{
    private static TerminalInputBuffer Typed(string text)
    {
        var buffer = new TerminalInputBuffer();
        foreach (char c in text) buffer.Append(c);
        return buffer;
    }

    [Fact]
    public void TypingLeavesTheCaretAtTheEnd()
    {
        var buffer = Typed("status");

        Assert.Equal("status", buffer.Text());
        Assert.Equal(6, buffer.Cursor);
        Assert.True(buffer.AtEnd);
    }

    [Fact]
    public void InsertsAtTheCaretRatherThanTheEnd()
    {
        var buffer = Typed("sttus");
        buffer.MoveLeft();
        buffer.MoveLeft();
        buffer.MoveLeft();
        buffer.Append('a');

        Assert.Equal("status", buffer.Text());
        Assert.Equal(3, buffer.Cursor);
        Assert.False(buffer.AtEnd);
    }

    [Fact]
    public void BackspaceRemovesTheCharacterBeforeTheCaret()
    {
        var buffer = Typed("stxatus");
        buffer.MoveHome();
        buffer.MoveRight();
        buffer.MoveRight();
        buffer.MoveRight();
        buffer.Backspace();

        Assert.Equal("status", buffer.Text());
        Assert.Equal(2, buffer.Cursor);
    }

    [Fact]
    public void DeleteRemovesTheCharacterUnderTheCaret()
    {
        var buffer = Typed("sttatus");
        buffer.MoveHome();
        buffer.MoveRight();
        buffer.Delete();

        Assert.Equal("status", buffer.Text());
        Assert.Equal(1, buffer.Cursor);
    }

    [Fact]
    public void CaretStopsAtBothEnds()
    {
        var buffer = Typed("hi");

        buffer.MoveLeft();
        buffer.MoveLeft();
        buffer.MoveLeft();
        Assert.Equal(0, buffer.Cursor);

        buffer.MoveRight();
        buffer.MoveRight();
        buffer.MoveRight();
        Assert.Equal(2, buffer.Cursor);
    }

    [Fact]
    public void BackspaceAtTheStartIsANoOp()
    {
        var buffer = Typed("hi");
        buffer.MoveHome();
        buffer.Backspace();

        Assert.Equal("hi", buffer.Text());
        Assert.Equal(0, buffer.Cursor);
    }

    [Fact]
    public void DeleteAtTheEndIsANoOp()
    {
        var buffer = Typed("hi");
        buffer.Delete();

        Assert.Equal("hi", buffer.Text());
        Assert.Equal(2, buffer.Cursor);
    }

    [Fact]
    public void HomeAndEndJumpToTheEdges()
    {
        var buffer = Typed("settings");

        buffer.MoveHome();
        Assert.Equal(0, buffer.Cursor);

        buffer.MoveEnd();
        Assert.Equal(8, buffer.Cursor);
    }

    [Fact]
    public void SetPlacesTheCaretAfterTheReplacement()
    {
        var buffer = Typed("st");
        buffer.Set("status ");

        Assert.Equal("status ", buffer.Text());
        Assert.Equal(7, buffer.Cursor);
        Assert.True(buffer.AtEnd);
    }

    [Fact]
    public void ClearAndTakeResetTheCaret()
    {
        var buffer = Typed("status");
        buffer.MoveHome();
        Assert.Equal("status", buffer.Take());
        Assert.Equal(0, buffer.Cursor);

        buffer.Set("again");
        buffer.MoveHome();
        buffer.Clear();
        Assert.Equal(0, buffer.Cursor);
    }

    [Fact]
    public void HistoryRecallPutsTheCaretAtTheEnd()
    {
        var buffer = new TerminalInputBuffer(new[] { "settings high-quality on" });
        buffer.Append('x');
        buffer.MoveHome();
        buffer.PreviousHistory();

        Assert.Equal("settings high-quality on", buffer.Text());
        Assert.Equal(24, buffer.Cursor);
        Assert.True(buffer.AtEnd);
    }

    [Fact]
    public void MidLineEditingSurvivesAFullRoundTrip()
    {
        var buffer = Typed("settins high-quality on");
        for (int i = 0; i < 17; i++) buffer.MoveLeft();
        buffer.Append('g');

        Assert.Equal("settings high-quality on", buffer.Text());
        Assert.Equal("settings high-quality on", buffer.Take());
    }
}
