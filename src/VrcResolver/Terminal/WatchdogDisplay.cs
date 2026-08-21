using System.Globalization;

namespace VrcResolver;

internal static class WatchdogDisplay
{
    public static string FormatBytes(long bytes)
    {
        const double KB = 1024;
        const double MB = KB * 1024;
        const double GB = MB * 1024;
        const double TB = GB * 1024;

        if (bytes < KB) return bytes.ToString(CultureInfo.InvariantCulture) + " B";
        if (bytes < MB) return (bytes / KB).ToString("0.0", CultureInfo.InvariantCulture) + " KB";
        if (bytes < GB) return (bytes / MB).ToString("0.0", CultureInfo.InvariantCulture) + " MB";
        if (bytes < TB) return (bytes / GB).ToString("0.00", CultureInfo.InvariantCulture) + " GB";
        return (bytes / TB).ToString("0.00", CultureInfo.InvariantCulture) + " TB";
    }

    public static string FormatBytesPerSecond(long bytes)
    {
        return FormatBytes(bytes) + "/s";
    }
}
