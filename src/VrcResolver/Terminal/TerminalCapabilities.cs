using System.Runtime.InteropServices;
using VrcResolver.Shared;

namespace VrcResolver;

internal static class TerminalCapabilities
{
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

    private const uint EnableQuickEditMode = 0x0040;
    private const uint EnableExtendedFlags = 0x0080;
    private const int StdInputHandle = -10;

    internal static uint WithoutQuickEdit(uint mode) => (mode & ~EnableQuickEditMode) | EnableExtendedFlags;

    public static string DisableQuickEdit()
    {
        if (!OperatingSystem.IsWindows() || Console.IsInputRedirected) return "unavailable";
        try
        {
            IntPtr input = GetStdHandle(StdInputHandle);
            if (!GetConsoleMode(input, out uint mode)) return "unavailable";
            if ((mode & EnableQuickEditMode) == 0) return "already_off";
            return SetConsoleMode(input, WithoutQuickEdit(mode)) ? "disabled" : "failed";
        }
        catch
        {
            return "failed";
        }
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GetStdHandle(int handle);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetConsoleMode(IntPtr handle, out uint mode);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetConsoleMode(IntPtr handle, uint mode);

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
        catch { }
    }
}
