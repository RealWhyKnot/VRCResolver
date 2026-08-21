using VrcResolver.Shared;

namespace VrcResolver;

internal static class TerminalCapabilities
{
    // NO_COLOR and ASCII_TERMINAL are each read in exactly one place
    // (ConsoleUx); this type just adds the terminal-specific animation gate.
    public static bool UseColor() => ConsoleUx.UseColor();

    public static bool UseAnimations()
    {
        if (!Environment.UserInteractive) return false;
        if (Console.IsInputRedirected) return false;
        if (!ConsoleUx.UseColor()) return false;
        string? disabled = LegacyCompat.GetEnvWithLegacyFallback("NO_ANIMATIONS");
        return !string.Equals(disabled, "1", StringComparison.Ordinal)
            && !string.Equals(disabled, "true", StringComparison.OrdinalIgnoreCase);
    }

    public static bool UseUnicode() => ConsoleUx.UseUnicode();

    public static bool TrySetCursorVisible(bool visible, out bool previous)
    {
        previous = true;
        if (!OperatingSystem.IsWindows())
            return false;

        try
        {
            previous = Console.CursorVisible;
            Console.CursorVisible = visible;
            return true;
        }
        catch
        {
            return false;
        }
    }

    public static void RestoreCursorVisible(bool visible)
    {
        if (!OperatingSystem.IsWindows())
            return;

        try { Console.CursorVisible = visible; }
        catch { /* no cursor */ }
    }
}
