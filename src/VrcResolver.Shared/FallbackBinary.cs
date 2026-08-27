namespace VrcResolver.Shared;

public static class FallbackBinary
{
    public static string? Select(
        string exeDir,
        string? vrcToolsDir,
        Func<string, bool> exists,
        Func<string, bool> isOurWrapper)
    {
        string c1 = Path.Combine(exeDir, "yt-dlp-og.exe");
        if (exists(c1) && !isOurWrapper(c1)) return c1;

        if (!string.IsNullOrEmpty(vrcToolsDir))
        {
            string c2 = Path.Combine(vrcToolsDir, "yt-dlp-og.exe");
            if (exists(c2) && !isOurWrapper(c2)) return c2;

            string c3 = Path.Combine(vrcToolsDir, "yt-dlp.exe");
            if (exists(c3) && !isOurWrapper(c3)) return c3;
        }

        return null;
    }
}
