using System.Collections.Concurrent;
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

    private static byte[] VrcBundledBytes()
    {
        byte[] content = new byte[(int)WrapperIdentity.OursSizeCeiling + 4096];
        for (int i = 0; i < content.Length; i++) content[i] = (byte)((i * 7) & 0xFF);
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
}
