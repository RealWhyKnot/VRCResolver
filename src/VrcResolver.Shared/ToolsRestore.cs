using System.Globalization;
using System.Runtime.Versioning;

namespace VrcResolver.Shared;

public enum RestoreOutcome
{
    Restored,
    KeptNewerVrchatBinary,
    RemovedOrphanWrapper,
    NothingToDo,
    Failed,
}

[SupportedOSPlatform("windows")]
public static class ToolsRestore
{
    private const string TargetName = "yt-dlp.exe";
    private const string OgName = "yt-dlp-og.exe";

    internal static Action<string, string, bool> MoveFile = File.Move;
    internal static Func<string, string?> ComputeHash = WrapperIdentity.ComputeSha256;

    public static RestoreOutcome Restore(string toolsDir, out string detail, string? knownHashesPath = null)
    {
        if (string.IsNullOrEmpty(toolsDir) || !Directory.Exists(toolsDir))
        {
            detail = "tools folder not found";
            return RestoreOutcome.Failed;
        }

        try
        {
            return RestoreCore(Path.Combine(toolsDir, TargetName), Path.Combine(toolsDir, OgName), knownHashesPath, out detail);
        }
        catch (Exception ex)
        {
            detail = "restore failed: " + ex.GetType().Name;
            return RestoreOutcome.Failed;
        }
    }

    private static RestoreOutcome RestoreCore(string target, string og, string? knownHashesPath, out string detail)
    {
        bool targetExists = File.Exists(target);

        if (!File.Exists(og))
        {
            if (!targetExists)
            {
                detail = "no yt-dlp.exe or yt-dlp-og.exe in Tools";
                return RestoreOutcome.NothingToDo;
            }
            if (WrapperIdentity.Classify(target, knownHashesPath) != WrapperKind.Ours)
            {
                detail = "yt-dlp-og.exe is gone and yt-dlp.exe is not our wrapper";
                return RestoreOutcome.NothingToDo;
            }
            if (Discard(target))
            {
                detail = "yt-dlp-og.exe is gone; removed our wrapper";
                return RestoreOutcome.RemovedOrphanWrapper;
            }
            detail = "yt-dlp-og.exe is gone and our wrapper could not be removed";
            return RestoreOutcome.Failed;
        }

        if (targetExists
            && WrapperIdentity.Classify(target, knownHashesPath) == WrapperKind.VrcBundledYtDlp
            && WrapperIdentity.LooksComplete(target)
            && KnownToDiffer(target, og))
        {
            detail = Discard(og)
                ? "yt-dlp.exe is a newer VRChat binary; dropped yt-dlp-og.exe"
                : "yt-dlp.exe is a newer VRChat binary; yt-dlp-og.exe could not be dropped";
            return RestoreOutcome.KeptNewerVrchatBinary;
        }

        if (!targetExists)
        {
            MoveFile(og, target, false);
            detail = "moved yt-dlp-og.exe into place";
            return RestoreOutcome.Restored;
        }

        try
        {
            MoveFile(og, target, true);
            detail = "moved yt-dlp-og.exe over yt-dlp.exe";
            return RestoreOutcome.Restored;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
        }

        if (!File.Exists(og))
        {
            detail = "yt-dlp-og.exe disappeared; yt-dlp.exe left alone";
            return RestoreOutcome.Failed;
        }

        string stale = target + ".stale-" + Stamp();
        try
        {
            File.Move(target, stale);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            detail = "yt-dlp.exe is locked and could not be moved aside: " + ex.GetType().Name;
            return RestoreOutcome.Failed;
        }

        try
        {
            MoveFile(og, target, false);
            detail = "yt-dlp.exe was in use; moved aside as " + Path.GetFileName(stale) + " and moved yt-dlp-og.exe into place";
            return RestoreOutcome.Restored;
        }
        catch (Exception ex)
        {
            detail = PutBack(stale, target, ex);
            return RestoreOutcome.Failed;
        }
    }

    private static string PutBack(string stale, string target, Exception cause)
    {
        string prefix = "could not move yt-dlp-og.exe into place (" + cause.GetType().Name + "); ";
        try
        {
            File.Move(stale, target, overwrite: false);
            return prefix + "yt-dlp.exe put back";
        }
        catch (Exception ex)
        {
            return prefix + "yt-dlp.exe left as " + Path.GetFileName(stale) + " (" + ex.GetType().Name + ")";
        }
    }

    private static bool Discard(string path)
    {
        try
        {
            File.Delete(path);
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
        }

        try
        {
            File.Move(path, path + ".stale-" + Stamp());
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    private static bool KnownToDiffer(string a, string b)
    {
        string? hashA = ComputeHash(a);
        if (hashA == null) return false;
        string? hashB = ComputeHash(b);
        return hashB != null && !string.Equals(hashA, hashB, StringComparison.Ordinal);
    }

    private static string Stamp() => DateTime.UtcNow.ToString("yyyyMMddHHmmss", CultureInfo.InvariantCulture);
}
