using System.Diagnostics;
using VrcResolver.Shared;
using Xunit;
using UpdaterProgram = VrcResolver.Updater.Program;

namespace VrcResolver.Tests;

public class UpdaterFinishTests : IDisposable
{
    private readonly TextWriter _originalOut = Console.Out;
    private readonly TextWriter _originalErr = Console.Error;
    private readonly string _root;
    private readonly string _payload;
    private readonly string _install;

    public UpdaterFinishTests()
    {
        _root = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-finish-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        _payload = Path.Combine(_root, "payload");
        _install = Path.Combine(_root, "install");
        Directory.CreateDirectory(_payload);
        Directory.CreateDirectory(_install);
    }

    public void Dispose()
    {
        Logger.Close();
        Console.SetOut(_originalOut);
        Console.SetError(_originalErr);
        try { Directory.Delete(_root, recursive: true); } catch { }
    }

    [Fact]
    public void FinishInstall_relaunches_with_a_detached_console()
    {
        File.WriteAllText(Path.Combine(_install, "vrcresolver.exe"), "OLD");
        File.WriteAllText(Path.Combine(_payload, "vrcresolver.exe"), "NEW");
        Console.SetOut(new ThrowingWriter());
        Console.SetError(new ThrowingWriter());
        var started = new List<ProcessStartInfo>();

        int exitCode = -1;
        Exception? thrown = Record.Exception(() =>
            exitCode = UpdaterProgram.FinishInstall(_payload, _install, psi =>
            {
                started.Add(psi);
                return null;
            }));

        Assert.Null(thrown);
        Assert.Equal(0, exitCode);
        ProcessStartInfo launched = Assert.Single(started);
        Assert.EndsWith("vrcresolver.exe", launched.FileName);
        Assert.Equal(_install, launched.WorkingDirectory);
        Assert.True(launched.UseShellExecute);
        Assert.Equal("NEW", File.ReadAllText(Path.Combine(_install, "vrcresolver.exe")));
    }

    [Fact]
    public void FinishInstall_logs_the_relaunch_when_the_console_is_gone()
    {
        File.WriteAllText(Path.Combine(_payload, "vrcresolver.exe"), "NEW");
        string logPath = Path.Combine(_root, "updater.log");
        Logger.InstallForTests(logPath, new ThrowingWriter());

        UpdaterProgram.FinishInstall(_payload, _install, _ => null);
        Logger.Close();

        Assert.Contains("Relaunching", File.ReadAllText(logPath));
    }

    [Fact]
    public void FinishInstall_surfaces_a_failed_copy_and_does_not_relaunch()
    {
        string target = Path.Combine(_install, "vrcresolver.exe");
        File.WriteAllText(target, "OLD");
        File.WriteAllText(Path.Combine(_payload, "vrcresolver.exe"), "NEW");
        Console.SetOut(new ThrowingWriter());
        Console.SetError(new ThrowingWriter());
        var started = new List<ProcessStartInfo>();

        using var held = new FileStream(target, FileMode.Open, FileAccess.Read, FileShare.None);

        Assert.ThrowsAny<IOException>(() =>
            UpdaterProgram.FinishInstall(_payload, _install, psi =>
            {
                started.Add(psi);
                return null;
            }));

        Assert.Empty(started);
    }

    [Fact]
    public void ReportFailure_reaches_the_log_file_and_returns_1_when_the_console_is_gone()
    {
        string logPath = Path.Combine(_root, "failure.log");
        Logger.InstallForTests(logPath, new ThrowingWriter());

        int exitCode = -1;
        Exception? thrown = Record.Exception(() =>
            exitCode = UpdaterProgram.ReportFailure(new InvalidOperationException("copy blew up")));
        Logger.Close();

        Assert.Null(thrown);
        Assert.Equal(1, exitCode);
        string text = File.ReadAllText(logPath);
        Assert.Contains("Updater failed", text);
        Assert.Contains("copy blew up", text);
    }

    [Fact]
    public void ReportFailure_prints_the_failure_when_the_console_works()
    {
        var error = new StringWriter();
        Console.SetError(error);

        int exitCode = UpdaterProgram.ReportFailure(new InvalidOperationException("copy blew up"));

        Assert.Equal(1, exitCode);
        Assert.Contains("Updater failed: System.InvalidOperationException: copy blew up", error.ToString());
    }
}
