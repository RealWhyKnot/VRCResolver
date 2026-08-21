using System.Diagnostics;

namespace VrcResolver;

internal static class UpdateCommand
{
    internal const string UpdateRequestedEnvVar = "VRCRESOLVER_UPDATE_REQUESTED";
    private const string UpdaterExeName = "vrcresolver.Updater.exe";

    internal static Func<ProcessStartInfo, Process?> StartProcess { get; set; } = Process.Start;

    public static Task ExecuteAsync(TerminalCommandContext ctx, string args, CancellationToken ct)
    {
        string updaterPath = Path.Combine(AppContext.BaseDirectory, UpdaterExeName);
        if (!File.Exists(updaterPath))
        {
            ctx.Renderer.Error("updater not found in install folder: " + updaterPath);
            return Task.CompletedTask;
        }

        ProcessStartInfo startInfo = BuildStartInfo(AppContext.BaseDirectory);
        Process? process = StartProcess(startInfo);
        if (process == null)
        {
            ctx.Renderer.Warn("update could not start.");
            return Task.CompletedTask;
        }

        ctx.Renderer.Info("update started; this window may close while files are replaced.");
        return Task.CompletedTask;
    }

    internal static ProcessStartInfo BuildStartInfo(string installDir)
    {
        string updaterPath = Path.Combine(installDir, UpdaterExeName);
        var startInfo = new ProcessStartInfo
        {
            FileName = updaterPath,
            WorkingDirectory = installDir,
            UseShellExecute = false,
        };
        startInfo.Environment[UpdateRequestedEnvVar] = "1";
        return startInfo;
    }
}
