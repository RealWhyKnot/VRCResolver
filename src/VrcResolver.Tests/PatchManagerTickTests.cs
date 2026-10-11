using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.Versioning;
using System.Text;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class PatchManagerTickTests : IDisposable
{
    private readonly string _scratchDir;
    private readonly string _installDir;
    private readonly string _toolsDir;
    private readonly string _stateDir;
    private readonly string _wrapperPath;
    private readonly string _targetPath;
    private readonly string _backupPath;

    public PatchManagerTickTests()
    {
        _scratchDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-patchtick-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        _installDir = Path.Combine(_scratchDir, "install");
        _toolsDir = Path.Combine(_scratchDir, "vrc-tools");
        _stateDir = Path.Combine(_scratchDir, "state");
        Directory.CreateDirectory(Path.Combine(_installDir, "tools"));
        Directory.CreateDirectory(_toolsDir);
        _wrapperPath = Path.Combine(_installDir, "tools", "yt-dlp.exe");
        _targetPath = Path.Combine(_toolsDir, "yt-dlp.exe");
        _backupPath = Path.Combine(_toolsDir, "yt-dlp-og.exe");
    }

    public void Dispose()
    {
        try { Directory.Delete(_scratchDir, recursive: true); } catch { }
    }

    private const int BundledLength = (int)WrapperIdentity.OursSizeCeiling + 4096;

    private static byte[] VrcBundledBytes() => PeFixtures.Unsigned(BundledLength, withCookie: true);

    private void WriteWrapper() => File.WriteAllBytes(_wrapperPath, WrapperBytes());

    private static byte[] PingPaddedPastTheCeiling()
    {
        byte[] ping = File.ReadAllBytes(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "PING.EXE"));
        byte[] content = new byte[BundledLength];
        ping.CopyTo(content, 0);
        int cookieAt = content.Length - WrapperIdentity.PyInstallerCookie.Length - PeFixtures.CookieTrailerBytes;
        WrapperIdentity.PyInstallerCookie.CopyTo(content.AsSpan(cookieAt));
        return content;
    }

    private static byte[] WrapperBytes()
    {
        byte[] content = new byte[8192];
        for (int i = 0; i < content.Length; i++) content[i] = (byte)((i * 31) & 0xFF);
        Encoding.UTF8.GetBytes(WrapperIdentity.Marker).CopyTo(content, 256);
        return content;
    }

    private PatchManager NewManager() => new(_installDir, _toolsDir, _stateDir);

    private static void WaitForQuiet(ConcurrentQueue<string> events)
    {
        int last;
        do
        {
            last = events.Count;
            Thread.Sleep(150);
        }
        while (events.Count != last);
    }

    [Fact]
    public void Missing_wrapper_with_backup_restores_once_and_never_moves_target_again()
    {
        byte[] vrc = VrcBundledBytes();
        File.WriteAllBytes(_backupPath, vrc);

        using var manager = NewManager();
        using var watcher = new FileSystemWatcher(_toolsDir) { EnableRaisingEvents = false };
        var moves = new ConcurrentQueue<string>();
        watcher.Renamed += (_, e) => moves.Enqueue(e.OldName + "->" + e.Name);
        watcher.Created += (_, e) => moves.Enqueue("created " + e.Name);
        watcher.Deleted += (_, e) => moves.Enqueue("deleted " + e.Name);
        watcher.EnableRaisingEvents = true;

        manager.TickOnce();
        WaitForQuiet(moves);

        Assert.True(File.Exists(_targetPath));
        Assert.False(File.Exists(_backupPath));
        Assert.Equal(vrc.Length, new FileInfo(_targetPath).Length);

        moves.Clear();
        for (int i = 0; i < 5; i++) manager.TickOnce();
        WaitForQuiet(moves);

        Assert.Empty(moves);
        Assert.True(File.Exists(_targetPath));
        Assert.False(File.Exists(_backupPath));
        Assert.Equal(vrc, File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void Wrapper_returning_after_missing_episode_resumes_backup_and_install()
    {
        byte[] vrc = VrcBundledBytes();
        File.WriteAllBytes(_backupPath, vrc);

        using var manager = NewManager();
        for (int i = 0; i < 3; i++) manager.TickOnce();
        Assert.Equal(vrc, File.ReadAllBytes(_targetPath));
        Assert.False(File.Exists(_backupPath));

        byte[] wrapper = WrapperBytes();
        File.WriteAllBytes(_wrapperPath, wrapper);

        for (int i = 0; i < 3; i++) manager.TickOnce();

        Assert.True(File.Exists(_backupPath));
        Assert.Equal(vrc, File.ReadAllBytes(_backupPath));
        Assert.True(File.Exists(_targetPath));
        Assert.Equal(wrapper, File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void Tick_does_not_back_up_a_truncated_signed_binary()
    {
        WriteWrapper();
        byte[] truncated = PeFixtures.Truncated(PeFixtures.Signed(BundledLength), 100);
        File.WriteAllBytes(_targetPath, truncated);

        using var manager = NewManager();
        manager.TickOnce();
        manager.TickOnce();

        Assert.False(File.Exists(_backupPath));
        Assert.Equal(truncated, File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void Tick_backs_up_only_after_the_size_settles()
    {
        WriteWrapper();
        byte[] bundled = PeFixtures.Signed(BundledLength);
        File.WriteAllBytes(_targetPath, bundled);

        using var manager = NewManager();
        manager.TickOnce();

        Assert.False(File.Exists(_backupPath));
        Assert.Equal(bundled, File.ReadAllBytes(_targetPath));

        manager.TickOnce();

        Assert.True(File.Exists(_backupPath));
        Assert.Equal(bundled, File.ReadAllBytes(_backupPath));
        Assert.Equal(File.ReadAllBytes(_wrapperPath), File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void Tick_waits_while_the_target_is_still_growing()
    {
        WriteWrapper();
        const int mib = 1024 * 1024;
        byte[] bundled = PeFixtures.Signed(12 * mib);
        using var manager = NewManager();

        int written = 0;
        foreach (int upTo in new[] { 4 * mib, 11 * mib, 12 * mib })
        {
            using (var download = new FileStream(_targetPath, FileMode.Append, FileAccess.Write, FileShare.Read))
            {
                download.Write(bundled, written, upTo - written);
            }
            written = upTo;
            manager.TickOnce();
            Assert.False(File.Exists(_backupPath));
        }

        manager.TickOnce();

        Assert.True(File.Exists(_backupPath));
        Assert.Equal(bundled, File.ReadAllBytes(_backupPath));
    }

    [Fact]
    public void Tick_refreshes_the_backup_only_from_a_settled_complete_replacement()
    {
        WriteWrapper();
        byte[] wrapper = File.ReadAllBytes(_wrapperPath);
        byte[] oldOg = PeFixtures.Signed(BundledLength, certificateLength: 1024);
        File.WriteAllBytes(_backupPath, oldOg);
        File.WriteAllBytes(_targetPath, wrapper);

        using var manager = NewManager();
        manager.TickOnce();
        manager.TickOnce();
        Assert.Equal(wrapper, File.ReadAllBytes(_targetPath));

        byte[] replacement = PeFixtures.Signed(BundledLength, certificateLength: 4096);
        byte[] partial = PeFixtures.Truncated(replacement, 100);
        File.WriteAllBytes(_targetPath, partial);
        manager.TickOnce();
        manager.TickOnce();

        Assert.Equal(partial, File.ReadAllBytes(_targetPath));
        Assert.Equal(oldOg, File.ReadAllBytes(_backupPath));

        File.WriteAllBytes(_targetPath, replacement);
        manager.TickOnce();

        Assert.Equal(replacement, File.ReadAllBytes(_targetPath));
        Assert.Equal(oldOg, File.ReadAllBytes(_backupPath));

        manager.TickOnce();

        Assert.Equal(replacement, File.ReadAllBytes(_backupPath));
        Assert.Equal(wrapper, File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void Tick_reapply_with_a_running_target_defers_quietly()
    {
        WriteWrapper();
        byte[] og = VrcBundledBytes();
        File.WriteAllBytes(_backupPath, og);
        byte[] ping = PingPaddedPastTheCeiling();
        File.WriteAllBytes(_targetPath, ping);

        using var manager = NewManager();
        using var process = Process.Start(new ProcessStartInfo(_targetPath, "-n 30 127.0.0.1")
        {
            UseShellExecute = false,
            CreateNoWindow = true,
        })!;
        try
        {
            for (int i = 0; i < 3; i++) manager.TickOnce();

            Assert.False(process.HasExited);
            Assert.Equal(ping, File.ReadAllBytes(_targetPath));
            Assert.Empty(Directory.GetFiles(_toolsDir, "*.new-*"));
        }
        finally
        {
            try { process.Kill(); } catch { }
            process.WaitForExit();
        }

        for (int i = 0; i < 2 && !WrapperIdentity.ContainsMarker(_targetPath); i++) manager.TickOnce();

        Assert.Equal(File.ReadAllBytes(_wrapperPath), File.ReadAllBytes(_targetPath));
        Assert.Equal(ping, File.ReadAllBytes(_backupPath));
    }

    private string[] ToolsEntries() => Directory.GetFiles(_toolsDir).Select(Path.GetFileName).OfType<string>().ToArray();

    private string CleanExitFlagPath => Path.Combine(_stateDir, "clean_exit.flag");

    [Fact]
    public void RestoreNow_after_patch_leaves_only_vanilla_and_writes_the_flag()
    {
        WriteWrapper();
        byte[] vrc = VrcBundledBytes();
        File.WriteAllBytes(_targetPath, vrc);

        using var manager = NewManager();
        manager.TickOnce();
        manager.TickOnce();
        Assert.True(WrapperIdentity.ContainsMarker(_targetPath));
        Assert.True(File.Exists(_backupPath));

        Assert.True(manager.RestoreNow("test"));

        Assert.Equal(new[] { "yt-dlp.exe" }, ToolsEntries());
        Assert.Equal(vrc, File.ReadAllBytes(_targetPath));
        Assert.True(File.Exists(CleanExitFlagPath));
    }

    [Fact]
    public void RestoreNow_racing_a_running_loop_never_leaves_the_wrapper()
    {
        WriteWrapper();
        byte[] vrc = VrcBundledBytes();

        for (int i = 0; i < 50; i++)
        {
            foreach (string path in Directory.GetFiles(_toolsDir)) File.Delete(path);
            File.WriteAllBytes(_targetPath, vrc);

            using var manager = new PatchManager(_installDir, _toolsDir, _stateDir, TimeSpan.FromMilliseconds(20));
            Assert.True(manager.Start());
            Thread.Sleep(i % 10 * 7);

            Assert.True(manager.RestoreNow("race " + i));
            Thread.Sleep(60);

            Assert.Equal(new[] { "yt-dlp.exe" }, ToolsEntries());
            Assert.Equal(vrc, File.ReadAllBytes(_targetPath));
        }
    }

    [Fact]
    public void Tick_after_RestoreNow_does_not_reapply()
    {
        WriteWrapper();
        byte[] vrc = VrcBundledBytes();
        File.WriteAllBytes(_targetPath, vrc);

        using var manager = NewManager();
        manager.TickOnce();
        manager.TickOnce();
        Assert.True(manager.RestoreNow("test"));

        for (int i = 0; i < 4; i++) manager.TickOnce();

        Assert.Equal(new[] { "yt-dlp.exe" }, ToolsEntries());
        Assert.Equal(vrc, File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void RestoreNow_without_a_tools_folder_still_writes_the_flag()
    {
        WriteWrapper();
        using var manager = new PatchManager(_installDir, null, _stateDir);

        Assert.True(manager.RestoreNow("patch-refused"));

        Assert.True(File.Exists(CleanExitFlagPath));
    }

    [Fact]
    public void Recover_after_RestoreNow_keeps_the_clean_exit_flag()
    {
        WriteWrapper();
        byte[] vrc = VrcBundledBytes();
        File.WriteAllBytes(_targetPath, vrc);

        using var manager = NewManager();
        Assert.True(manager.RestoreNow("ctrl-c"));
        manager.RecoverFromUncleanShutdown();

        Assert.True(File.Exists(CleanExitFlagPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, ToolsEntries());
        Assert.Equal(vrc, File.ReadAllBytes(_targetPath));
    }

    [Fact]
    public void Recover_after_a_kill_mid_restore()
    {
        byte[] og = VrcBundledBytes();
        File.WriteAllBytes(_backupPath, og);
        File.WriteAllBytes(_targetPath + ".stale-x", WrapperBytes());

        using var manager = NewManager();
        manager.RecoverFromUncleanShutdown();

        string[] left = Directory.GetFiles(_toolsDir).Select(Path.GetFileName).OfType<string>().ToArray();
        Assert.Equal(new[] { "yt-dlp.exe" }, left);
        Assert.Equal(og, File.ReadAllBytes(_targetPath));
    }
}
