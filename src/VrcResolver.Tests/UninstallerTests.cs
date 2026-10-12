using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;
using Xunit;
using UninstallerProgram = VrcResolver.Uninstaller.Program;
using WatchdogMatch = VrcResolver.Uninstaller.WatchdogMatch;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class UninstallerTests : IDisposable
{
    private const string UninspectableMessage =
        "a VRCResolver watchdog is running that the uninstaller can't inspect -- close it and run the uninstaller again";

    private const string InstallDir = @"C:\Apps\VRCResolver";

    private readonly TextWriter _originalOut = Console.Out;
    private readonly TextWriter _originalErr = Console.Error;
    private readonly StringWriter _out = new();
    private readonly StringWriter _err = new();
    private readonly string _toolsDir;

    public UninstallerTests()
    {
        ToolsRestore.MoveFile = File.Move;
        Console.SetOut(_out);
        Console.SetError(_err);
        _toolsDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-uninstaller-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_toolsDir);
    }

    public void Dispose()
    {
        ToolsRestore.MoveFile = File.Move;
        Console.SetOut(_originalOut);
        Console.SetError(_originalErr);
        try { Directory.Delete(_toolsDir, recursive: true); } catch { }
    }

    private string TargetPath => Path.Combine(_toolsDir, "yt-dlp.exe");
    private string OgPath => Path.Combine(_toolsDir, "yt-dlp-og.exe");

    private string[] FileNames() => Directory.GetFiles(_toolsDir).Select(p => Path.GetFileName(p)).Order().ToArray();

    [Theory]
    [InlineData(@"C:\Apps\VRCResolver\vrcresolver.exe", "Ours")]
    [InlineData(@"C:\Apps\VRCResolver\WKVRCProxy.exe", "Ours")]
    [InlineData(@"c:\apps\vrcresolver\VRCRESOLVER.EXE", "Ours")]
    [InlineData(@"C:\Apps\VRCResolver\.\vrcresolver.exe", "Ours")]
    [InlineData(@"C:\Other\VRCResolver\vrcresolver.exe", "OtherInstall")]
    [InlineData(@"C:\Apps\VRCResolver\sub\vrcresolver.exe", "OtherInstall")]
    [InlineData(@"C:\Apps\VRCResolver\other.exe", "OtherInstall")]
    [InlineData(null, "Uninspectable")]
    [InlineData("", "Uninspectable")]
    public void MatchWatchdog_classifies_the_main_module_path(string? mainModulePath, string expected)
    {
        WatchdogMatch match = UninstallerProgram.MatchWatchdog(mainModulePath, @"C:\Apps\VRCResolver");

        Assert.Equal(expected, match.ToString());
    }

    [Fact]
    public void MatchWatchdog_ignores_a_trailing_separator_on_the_install_dir()
    {
        WatchdogMatch match = UninstallerProgram.MatchWatchdog(@"C:\Apps\VRCResolver\vrcresolver.exe", @"C:\Apps\VRCResolver\");

        Assert.Equal(WatchdogMatch.Ours, match);
    }

    [Fact]
    public void CloseWatchdogs_fails_when_a_running_watchdog_denies_access()
    {
        var stopped = new List<Process>();

        var ex = Assert.Throws<InvalidOperationException>(() =>
            UninstallerProgram.CloseWatchdogs(
                new[] { Process.GetCurrentProcess() }, InstallDir, _ => throw new Win32Exception(5), stopped.Add));

        Assert.Equal(UninspectableMessage, ex.Message);
        Assert.Empty(stopped);
        Assert.Contains("uninspectable=1", _out.ToString());
    }

    [Fact]
    public void CloseWatchdogs_fails_when_a_running_watchdog_has_no_main_module()
    {
        var stopped = new List<Process>();

        var ex = Assert.Throws<InvalidOperationException>(() =>
            UninstallerProgram.CloseWatchdogs(new[] { Process.GetCurrentProcess() }, InstallDir, _ => null, stopped.Add));

        Assert.Equal(UninspectableMessage, ex.Message);
        Assert.Empty(stopped);
    }

    [Fact]
    public void CloseWatchdogs_skips_a_process_that_has_already_exited()
    {
        using Process gone = StartAndFinishProcess();
        var stopped = new List<Process>();

        int closed = UninstallerProgram.CloseWatchdogs(
            new[] { gone }, InstallDir, UninstallerProgram.ReadMainModulePath, stopped.Add);

        Assert.Equal(0, closed);
        Assert.Empty(stopped);
        string log = _out.ToString();
        Assert.Contains("already_exited=1", log);
        Assert.Contains("uninspectable=0", log);
        Assert.Contains("skipped_other_installs=0", log);
    }

    [Fact]
    public void CloseWatchdogs_still_fails_for_a_denied_process_beside_an_exited_one()
    {
        using Process gone = StartAndFinishProcess();
        var stopped = new List<Process>();

        var ex = Assert.Throws<InvalidOperationException>(() =>
            UninstallerProgram.CloseWatchdogs(
                new[] { gone, Process.GetCurrentProcess() }, InstallDir, _ => throw new Win32Exception(5), stopped.Add));

        Assert.Equal(UninspectableMessage, ex.Message);
        Assert.Empty(stopped);
        string log = _out.ToString();
        Assert.Contains("already_exited=1", log);
        Assert.Contains("uninspectable=1", log);
    }

    [Fact]
    public void CloseWatchdogs_leaves_a_process_from_another_install_alone()
    {
        var stopped = new List<Process>();

        int closed = UninstallerProgram.CloseWatchdogs(
            new[] { Process.GetCurrentProcess() }, InstallDir, UninstallerProgram.ReadMainModulePath, stopped.Add);

        Assert.Equal(0, closed);
        Assert.Empty(stopped);
        string log = _out.ToString();
        Assert.Contains("skipped_other_installs=1", log);
        Assert.Contains("uninspectable=0", log);
    }

    [Fact]
    public void CloseWatchdogs_stops_a_process_from_this_install()
    {
        var stopped = new List<int>();
        int ownId = Environment.ProcessId;

        int closed = UninstallerProgram.CloseWatchdogs(
            new[] { Process.GetCurrentProcess() },
            InstallDir,
            _ => Path.Combine(InstallDir, "vrcresolver.exe"),
            p => stopped.Add(p.Id));

        Assert.Equal(1, closed);
        Assert.Equal(new[] { ownId }, stopped);
        Assert.Contains("matched=1", _out.ToString());
    }

    [Fact]
    public void Uninstall_restore_uses_the_shared_restore()
    {
        File.WriteAllBytes(TargetPath, ToolsRestoreTests.WrapperBytes());
        byte[] vanilla = ToolsRestoreTests.BundledBytes(1);
        File.WriteAllBytes(OgPath, vanilla);

        UninstallerProgram.RestoreYtDlp(_toolsDir);

        Assert.Equal(vanilla, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
        Assert.Contains("Restored", _out.ToString());
    }

    [Fact]
    public void Uninstall_restore_keeps_a_newer_vrchat_binary()
    {
        byte[] newer = ToolsRestoreTests.CompleteBundledBytes(1);
        File.WriteAllBytes(TargetPath, newer);
        File.WriteAllBytes(OgPath, ToolsRestoreTests.CompleteBundledBytes(2));

        UninstallerProgram.RestoreYtDlp(_toolsDir);

        Assert.Equal(newer, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
        Assert.Contains("KeptNewerVrchatBinary", _out.ToString());
    }

    [Fact]
    public void Uninstall_restore_removes_our_wrapper_when_the_og_is_missing_and_says_so()
    {
        File.WriteAllBytes(TargetPath, ToolsRestoreTests.WrapperBytes());

        UninstallerProgram.RestoreYtDlp(_toolsDir);

        Assert.Empty(Directory.GetFiles(_toolsDir));
        Assert.Contains("yt-dlp-og.exe was missing", _out.ToString());
    }

    [Fact]
    public void Uninstall_restore_leaves_a_vrchat_binary_alone_when_the_og_is_missing()
    {
        byte[] vrc = ToolsRestoreTests.BundledBytes(1);
        File.WriteAllBytes(TargetPath, vrc);

        UninstallerProgram.RestoreYtDlp(_toolsDir);

        Assert.Equal(vrc, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Uninstall_restore_sweeps_leftover_sidecars()
    {
        File.WriteAllBytes(TargetPath, ToolsRestoreTests.WrapperBytes());
        File.WriteAllBytes(OgPath, ToolsRestoreTests.BundledBytes(1));
        File.WriteAllText(Path.Combine(_toolsDir, "yt-dlp.exe.stale-20260101000000"), "x");
        File.WriteAllText(Path.Combine(_toolsDir, "yt-dlp-wrapper.log"), "x");

        UninstallerProgram.RestoreYtDlp(_toolsDir);

        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Uninstall_restore_fails_the_step_when_the_shared_restore_fails()
    {
        byte[] wrapper = ToolsRestoreTests.WrapperBytes();
        File.WriteAllBytes(TargetPath, wrapper);
        File.WriteAllBytes(OgPath, ToolsRestoreTests.BundledBytes(1));
        ToolsRestore.MoveFile = (_, _, _) => throw new IOException("simulated lock");

        var ex = Assert.Throws<InvalidOperationException>(() => UninstallerProgram.RestoreYtDlp(_toolsDir));

        Assert.Contains("yt-dlp.exe", ex.Message);
        Assert.Equal(wrapper, File.ReadAllBytes(TargetPath));
        Assert.Contains("WARNING", _err.ToString());
    }

    [Fact]
    public void Uninstall_restore_skips_when_the_tools_folder_is_not_known()
    {
        UninstallerProgram.RestoreYtDlp(null);

        Assert.Contains("skipped", _out.ToString());
    }

    private static Process StartAndFinishProcess()
    {
        string cmd = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "cmd.exe");
        Process process = Process.Start(new ProcessStartInfo(cmd, "/c exit")
        {
            UseShellExecute = false,
            CreateNoWindow = true,
        })!;
        process.WaitForExit();
        return process;
    }
}
