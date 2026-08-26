using VrcResolver.Shared;

namespace VrcResolver;

internal static class UpdaterRepair
{
    // Staged-updater pairs applied at watchdog startup: the running updater
    // cannot overwrite itself, so the new one arrives under a .next name and
    // is swapped in here on the following launch.
    private static readonly (string Staged, string Target)[] Pairs =
    {
        ("vrcresolver.Updater.next.exe", "vrcresolver.Updater.exe"),
    };

    public static bool ApplyIfPresent(string installDir)
    {
        bool applied = false;
        foreach (var (stagedName, targetName) in Pairs)
        {
            if (ApplyPair(installDir, stagedName, targetName)) applied = true;
        }
        return applied;
    }

    private static bool ApplyPair(string installDir, string stagedName, string targetName)
    {
        string staged = Path.Combine(installDir, stagedName);
        if (!File.Exists(staged)) return false;

        string target = Path.Combine(installDir, targetName);
        string backup = target + ".old-" + Guid.NewGuid().ToString("N").Substring(0, 8);

        try
        {
            if (File.Exists(target))
            {
                MoveWithRetry(target, backup, retries: 3);
            }

            MoveWithRetry(staged, target, retries: 3);
            try { if (File.Exists(backup)) File.Delete(backup); } catch { /* best-effort */ }
            ConsoleUx.Success(LogComponent.Update, "updater refreshed for future updates.");
            return true;
        }
        catch (Exception ex)
        {
            try
            {
                if (!File.Exists(target) && File.Exists(backup))
                    File.Move(backup, target);
            }
            catch { /* best-effort */ }

            Logger.WriteFileOnly("[update] staged updater repair failed: " + ex.GetType().Name + ": " + ex.Message);
            ConsoleUx.Warn(LogComponent.Update, "could not refresh updater yet; it will retry on next launch.");
            return false;
        }
    }

    private static void MoveWithRetry(string src, string dst, int retries)
    {
        for (int attempt = 1; attempt <= retries; attempt++)
        {
            try
            {
                if (File.Exists(dst)) File.Move(src, dst, overwrite: true);
                else File.Move(src, dst);
                return;
            }
            catch (IOException) when (attempt < retries)
            {
                Thread.Sleep(200);
            }
        }
    }
}
