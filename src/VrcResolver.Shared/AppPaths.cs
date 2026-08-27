namespace VrcResolver.Shared;

public static class AppPaths
{
    private const string ProductDirName = "vrcresolver";

    public const string RenameMigrationMarker = ".migrated-from-wkvrcproxy";

    private static string LocalLowDir()
    {
        string local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        if (local.EndsWith(@"\Local", StringComparison.OrdinalIgnoreCase))
            return local[..^"\\Local".Length] + "\\LocalLow";
        return local + "Low";
    }

    public static string StateRoot() => Path.Combine(LocalLowDir(), ProductDirName);

    public static string LogsDir() => Path.Combine(StateRoot(), "logs");
    public static string CrashesDir() => Path.Combine(StateRoot(), "crashes");

    public static string ProgramDataRoot()
        => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), ProductDirName);

    public static void MigrateFromLegacyProduct(Action<string>? log = null)
    {
        try
        {
            string newRoot = StateRoot();
            if (!RenamedRootAlreadyPopulated(newRoot))
            {
                string legacyLowRoot = Path.Combine(LocalLowDir(), LegacyCompat.LegacyStateDirName);
                string legacyLocalApp = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    LegacyCompat.LegacyStateDirName);
                if (!Directory.Exists(legacyLowRoot) && Directory.Exists(legacyLocalApp))
                    MigrateLegacyLocalAppState(legacyLocalApp, legacyLowRoot, log);
                if (Directory.Exists(legacyLowRoot))
                    CopyLegacyRoot(legacyLowRoot, newRoot, log);
            }
        }
        catch (Exception ex)
        {
            log?.Invoke("[migrate] rename migration failed: " + ex.GetType().Name + ": " + ex.Message);
        }

        try
        {
            string legacyProgramData = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
                LegacyCompat.LegacyStateDirName);
            MigrateProgramData(legacyProgramData, ProgramDataRoot(), log);
        }
        catch (Exception ex)
        {
            log?.Invoke("[migrate] ProgramData migration failed: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    internal static bool RenamedRootAlreadyPopulated(string newRoot)
    {
        if (File.Exists(Path.Combine(newRoot, RenameMigrationMarker))) return true;
        try
        {
            return Directory.Exists(newRoot) && Directory.EnumerateFileSystemEntries(newRoot).Any();
        }
        catch
        {
            return false;
        }
    }

    internal static void CopyLegacyRoot(string legacyRoot, string newRoot, Action<string>? log = null)
    {
        Directory.CreateDirectory(newRoot);
        int copied = 0;

        foreach (string dir in Directory.EnumerateDirectories(legacyRoot, "*", SearchOption.AllDirectories))
        {
            string rel = Path.GetRelativePath(legacyRoot, dir);
            if (IsUnderLogs(rel)) continue;
            try { Directory.CreateDirectory(Path.Combine(newRoot, rel)); } catch { }
        }

        foreach (string file in Directory.EnumerateFiles(legacyRoot, "*", SearchOption.AllDirectories))
        {
            string rel = Path.GetRelativePath(legacyRoot, file);
            if (IsUnderLogs(rel)) continue;
            string dst = Path.Combine(newRoot, rel);
            if (File.Exists(dst)) continue;
            try { File.Copy(file, dst); copied++; } catch { }
        }

        File.WriteAllText(Path.Combine(newRoot, RenameMigrationMarker), DateTime.UtcNow.ToString("o"));
        if (copied > 0)
            log?.Invoke($"[migrate] state copied from {legacyRoot} -> {newRoot} (files={copied})");
    }

    private static bool IsUnderLogs(string relativePath)
    {
        string first = relativePath.Split(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar)[0];
        return string.Equals(first, "logs", StringComparison.OrdinalIgnoreCase);
    }

    internal static void MigrateProgramData(string legacyRoot, string newRoot, Action<string>? log = null)
    {
        if (!Directory.Exists(legacyRoot)) return;
        try
        {
            if (Directory.Exists(newRoot) && Directory.EnumerateFileSystemEntries(newRoot).Any()) return;
        }
        catch { return; }

        Directory.CreateDirectory(newRoot);
        int copied = 0;
        foreach (string file in Directory.EnumerateFiles(legacyRoot))
        {
            string dst = Path.Combine(newRoot, Path.GetFileName(file));
            if (File.Exists(dst)) continue;
            try { File.Copy(file, dst); copied++; } catch { }
        }
        if (copied > 0)
            log?.Invoke($"[migrate] machine state copied from {legacyRoot} -> {newRoot} (files={copied})");
    }

    internal static void MigrateLegacyLocalAppState(string legacySource, string legacyLowRoot, Action<string>? log = null)
    {
        try
        {
            string marker = Path.Combine(legacyLowRoot, ".migrated-from-localapp");
            if (File.Exists(marker)) return;

            Directory.CreateDirectory(legacyLowRoot);

            if (!Directory.Exists(legacySource))
            {
                File.WriteAllText(marker, DateTime.UtcNow.ToString("o"));
                return;
            }

            int movedDirs = 0, movedFiles = 0;

            foreach (string sub in Directory.EnumerateDirectories(legacySource))
            {
                string subName = Path.GetFileName(sub);

                if (string.Equals(subName, "logs", StringComparison.OrdinalIgnoreCase))
                {
                    try { Directory.Delete(sub, recursive: true); } catch { }
                    continue;
                }

                string dst = Path.Combine(legacyLowRoot, subName);
                if (Directory.Exists(dst))
                {
                    foreach (string f in Directory.EnumerateFiles(sub))
                    {
                        string fileName = Path.GetFileName(f);
                        string fDst = Path.Combine(dst, fileName);
                        if (!File.Exists(fDst))
                        {
                            try { File.Move(f, fDst); movedFiles++; } catch { }
                        }
                    }
                    try { Directory.Delete(sub, recursive: true); } catch { }
                }
                else
                {
                    try { Directory.Move(sub, dst); movedDirs++; } catch { }
                }
            }

            foreach (string file in Directory.EnumerateFiles(legacySource))
            {
                string fileName = Path.GetFileName(file);
                string dst = Path.Combine(legacyLowRoot, fileName);
                if (!File.Exists(dst))
                {
                    try { File.Move(file, dst); movedFiles++; } catch { }
                }
            }

            File.WriteAllText(marker, DateTime.UtcNow.ToString("o"));

            try { Directory.Delete(legacySource, recursive: true); } catch { }

            if (movedDirs > 0 || movedFiles > 0)
                log?.Invoke($"[migrate] state moved from {legacySource} -> {legacyLowRoot} (dirs={movedDirs}, files={movedFiles})");
        }
        catch (Exception ex)
        {
            log?.Invoke("[migrate] failed: " + ex.GetType().Name + ": " + ex.Message);
        }
    }
}
