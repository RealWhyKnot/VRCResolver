namespace VrcResolver;

internal readonly record struct TerminalStyle(ConsoleColor Foreground)
{
    public static TerminalStyle Plain { get; } = new(ConsoleColor.Gray);
    public static TerminalStyle Dim { get; } = new(ConsoleColor.DarkGray);
    public static TerminalStyle Bright { get; } = new(ConsoleColor.White);
    public static TerminalStyle Success { get; } = new(ConsoleColor.Green);
    public static TerminalStyle Warning { get; } = new(ConsoleColor.Yellow);
    public static TerminalStyle Error { get; } = new(ConsoleColor.Red);
    public static TerminalStyle Accent { get; } = new(ConsoleColor.DarkCyan);
}

internal readonly record struct TerminalTextRun(string Text, TerminalStyle Style)
{
    public TerminalTextRun(string text, ConsoleColor color)
        : this(text, new TerminalStyle(color))
    {
    }

    public ConsoleColor Color => Style.Foreground;
}

internal sealed class TerminalFrame
{
    public TerminalFrame(IReadOnlyList<TerminalTextRun> runs)
    {
        Runs = runs ?? throw new ArgumentNullException(nameof(runs));
        PlainText = BuildPlainText(runs);
    }

    public TerminalFrame(string plainText, IReadOnlyList<TerminalTextRun> runs)
    {
        PlainText = plainText ?? "";
        Runs = runs ?? throw new ArgumentNullException(nameof(runs));
    }

    public string PlainText { get; }
    public IReadOnlyList<TerminalTextRun> Runs { get; }

    public static TerminalFrame Empty { get; } =
        new("", Array.Empty<TerminalTextRun>());

    public static TerminalFrame Plain(string text, ConsoleColor color = ConsoleColor.Gray)
    {
        text ??= "";
        return new TerminalFrame(text, new[] { new TerminalTextRun(text, color) });
    }

    public static TerminalFrame FromRuns(params TerminalTextRun[] runs)
    {
        return new TerminalFrame(runs);
    }

    private static string BuildPlainText(IReadOnlyList<TerminalTextRun> runs)
    {
        int length = 0;
        for (int i = 0; i < runs.Count; i++)
            length += runs[i].Text.Length;

        return string.Create(length, runs, static (span, source) =>
        {
            int offset = 0;
            for (int i = 0; i < source.Count; i++)
            {
                string text = source[i].Text;
                text.AsSpan().CopyTo(span[offset..]);
                offset += text.Length;
            }
        });
    }
}
