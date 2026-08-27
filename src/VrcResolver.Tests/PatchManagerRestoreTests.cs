using System.Runtime.Versioning;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class PatchManagerRestoreTests : IDisposable
{
    private readonly string _toolsDir;

    public PatchManagerRestoreTests()
    {
        _toolsDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-restore-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_toolsDir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_toolsDir, recursive: true); } catch { }
    }

    private string TargetPath => Path.Combine(_toolsDir, "yt-dlp.exe");
    private string BackupPath => Path.Combine(_toolsDir, "yt-dlp-og.exe");

    [Fact]
    public void Restore_succeeds_via_atomic_move_when_target_unlocked()
    {
        File.WriteAllText(TargetPath, "PATCHED");
        File.WriteAllText(BackupPath, "VANILLA");

        Assert.True(PatchManager.RestoreYtDlpInTools(_toolsDir));
        Assert.True(File.Exists(TargetPath));
        Assert.False(File.Exists(BackupPath));
        Assert.Equal("VANILLA", File.ReadAllText(TargetPath));

        var siblings = Directory.GetFiles(_toolsDir);
        Assert.Single(siblings);
    }

    [Fact]
    public void Restore_returns_false_when_target_is_fully_locked()
    {
        File.WriteAllText(TargetPath, "PATCHED");
        File.WriteAllText(BackupPath, "VANILLA");

        var lockHandle = new FileStream(TargetPath, FileMode.Open, FileAccess.Read, FileShare.None);
        bool ok;
        try
        {
            ok = PatchManager.RestoreYtDlpInTools(_toolsDir);
        }
        finally
        {
            lockHandle.Dispose();
        }
        Assert.False(ok);
        Assert.True(File.Exists(BackupPath));
    }

    [Fact]
    public void Restore_returns_false_when_backup_missing()
    {
        File.WriteAllText(TargetPath, "PATCHED");

        bool ok = PatchManager.RestoreYtDlpInTools(_toolsDir);
        Assert.False(ok);
        Assert.True(File.Exists(TargetPath));
    }

    [Fact]
    public void Restore_handles_missing_directory()
    {
        bool ok = PatchManager.RestoreYtDlpInTools(Path.Combine(_toolsDir, "does-not-exist"));
        Assert.False(ok);
    }

    [Fact]
    public void AtomicCopy_replaces_existing_destination_atomically()
    {
        string src = Path.Combine(_toolsDir, "source.bin");
        string dst = Path.Combine(_toolsDir, "dest.bin");
        File.WriteAllText(src, "NEW");
        File.WriteAllText(dst, "OLD");

        PatchManager.AtomicCopy(src, dst);

        Assert.Equal("NEW", File.ReadAllText(dst));
        var leftovers = Directory.GetFiles(_toolsDir, "dest.bin.new-*");
        Assert.Empty(leftovers);
    }

    [Fact]
    public void AtomicCopy_cleans_up_tmp_on_failure()
    {
        string src = Path.Combine(_toolsDir, "missing.bin");
        string dst = Path.Combine(_toolsDir, "dest.bin");
        File.WriteAllText(dst, "OLD");

        Assert.ThrowsAny<Exception>(() => PatchManager.AtomicCopy(src, dst));

        Assert.Equal("OLD", File.ReadAllText(dst));
        var leftovers = Directory.GetFiles(_toolsDir, "dest.bin.new-*");
        Assert.Empty(leftovers);
    }

    [Fact]
    public void IsTargetInUse_returns_false_when_no_handle_held()
    {
        File.WriteAllText(TargetPath, "stub");
        Assert.False(PatchManager.IsTargetInUse(TargetPath));
    }

    [Fact]
    public void IsTargetInUse_returns_true_when_held_with_FileShare_None()
    {
        File.WriteAllText(TargetPath, "stub");
        using var holder = new FileStream(TargetPath, FileMode.Open, FileAccess.Read, FileShare.None);
        Assert.True(PatchManager.IsTargetInUse(TargetPath));
    }

    [Fact]
    public void IsTargetInUse_returns_true_when_held_with_FileShare_Read()
    {
        File.WriteAllText(TargetPath, "stub");
        using var holder = new FileStream(TargetPath, FileMode.Open, FileAccess.Read, FileShare.Read);
        Assert.True(PatchManager.IsTargetInUse(TargetPath));
    }

    [Fact]
    public void IsTargetInUse_returns_true_when_held_with_FileShare_ReadWrite()
    {
        File.WriteAllText(TargetPath, "stub");
        using var holder = new FileStream(TargetPath, FileMode.Open, FileAccess.ReadWrite, FileShare.ReadWrite);
        Assert.True(PatchManager.IsTargetInUse(TargetPath));
    }

    [Fact]
    public void IsTargetInUse_returns_false_when_path_missing()
    {
        Assert.False(PatchManager.IsTargetInUse(Path.Combine(_toolsDir, "does-not-exist.exe")));
    }

    [Fact]
    public void IsTargetInUse_returns_false_when_directory_missing()
    {
        Assert.False(PatchManager.IsTargetInUse(Path.Combine(_toolsDir, "no-such-dir", "yt-dlp.exe")));
    }
}
