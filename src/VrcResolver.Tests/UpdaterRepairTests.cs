using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class UpdaterRepairTests : IDisposable
{
    private readonly string _root;

    public UpdaterRepairTests()
    {
        _root = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-updater-repair-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_root);
    }

    public void Dispose()
    {
        try { Directory.Delete(_root, recursive: true); } catch { }
    }

    [Fact]
    public void ApplyIfPresent_replaces_stale_updater_with_staged_copy()
    {
        File.WriteAllText(Path.Combine(_root, "vrcresolver.Updater.exe"), "OLD");
        File.WriteAllText(Path.Combine(_root, "vrcresolver.Updater.next.exe"), "NEW");

        bool applied = UpdaterRepair.ApplyIfPresent(_root);

        Assert.True(applied);
        Assert.Equal("NEW", File.ReadAllText(Path.Combine(_root, "vrcresolver.Updater.exe")));
        Assert.False(File.Exists(Path.Combine(_root, "vrcresolver.Updater.next.exe")));
        Assert.Empty(Directory.GetFiles(_root, "*.old-*"));
    }

    [Fact]
    public void ApplyIfPresent_ignores_pre_rename_names()
    {
        File.WriteAllText(Path.Combine(_root, "WKVRCProxy.Updater.exe"), "OLD-REAL-UPDATER");
        File.WriteAllText(Path.Combine(_root, "WKVRCProxy.Updater.next.exe"), "LAUNCHER");

        Assert.False(UpdaterRepair.ApplyIfPresent(_root));
        Assert.Equal("OLD-REAL-UPDATER", File.ReadAllText(Path.Combine(_root, "WKVRCProxy.Updater.exe")));
    }

    [Fact]
    public void ApplyIfPresent_noops_when_no_staged_copy_exists()
    {
        File.WriteAllText(Path.Combine(_root, "vrcresolver.Updater.exe"), "OLD");

        bool applied = UpdaterRepair.ApplyIfPresent(_root);

        Assert.False(applied);
        Assert.Equal("OLD", File.ReadAllText(Path.Combine(_root, "vrcresolver.Updater.exe")));
    }
}
