namespace VrcResolver;

internal readonly record struct TerminalGlyphSet(
    string Bullet,
    string Detail,
    string Branch,
    string Horizontal,
    string Vertical,
    string TopLeft,
    string TopRight,
    string BottomLeft,
    string BottomRight,
    string[] Spinner,
    string[] Sparkline)
{
    public static TerminalGlyphSet Unicode { get; } = new(
        Bullet: "•",
        Detail: "◦",
        Branch: "└",
        Horizontal: "─",
        Vertical: "│",
        TopLeft: "┌",
        TopRight: "┐",
        BottomLeft: "└",
        BottomRight: "┘",
        Spinner: new[] { "◐", "◓", "◑", "◒" },
        Sparkline: new[] { "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█" });

    public static TerminalGlyphSet Ascii { get; } = new(
        Bullet: "*",
        Detail: "-",
        Branch: "`",
        Horizontal: "-",
        Vertical: "|",
        TopLeft: "+",
        TopRight: "+",
        BottomLeft: "+",
        BottomRight: "+",
        Spinner: new[] { "|", "/", "-", "\\" },
        Sparkline: new[] { "_", ".", "-", "=", "#", "#", "#", "#" });

    public static TerminalGlyphSet For(bool unicode)
    {
        return unicode ? Unicode : Ascii;
    }
}
