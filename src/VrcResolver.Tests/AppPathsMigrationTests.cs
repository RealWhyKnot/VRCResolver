using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class AppPathsMigrationTests : IDisposable
{
    private readonly string _root;

    public AppPathsMigrationTests()
    {
        _root = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-migrate-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_root);
    }

    public void Dispose()
    {
        try { Directory.Delete(_root, recursive: true); } catch { }
    }

    private string Dir(string name)
    {
        string p = Path.Combine(_root, name);
        Directory.CreateDirectory(p);
        return p;
    }

    [Fact]
    public void CopyLegacyRoot_copies_state_skips_logs_and_leaves_source_in_place()
    {
        string legacy = Dir("legacy");
        File.WriteAllText(Path.Combine(legacy, "settings.json"), "{}");
        Directory.CreateDirectory(Path.Combine(legacy, "crashes"));
        File.WriteAllText(Path.Combine(legacy, "crashes", "crash-1.log"), "boom");
        Directory.CreateDirectory(Path.Combine(legacy, "logs"));
        File.WriteAllText(Path.Combine(legacy, "logs", "watchdog-1.log"), "noise");

        string newRoot = Path.Combine(_root, "renamed");
        AppPaths.CopyLegacyRoot(legacy, newRoot);

        Assert.Equal("{}", File.ReadAllText(Path.Combine(newRoot, "settings.json")));
        Assert.Equal("boom", File.ReadAllText(Path.Combine(newRoot, "crashes", "crash-1.log")));
        Assert.False(Directory.Exists(Path.Combine(newRoot, "logs")));
        Assert.True(File.Exists(Path.Combine(newRoot, AppPaths.RenameMigrationMarker)));

        Assert.True(File.Exists(Path.Combine(legacy, "settings.json")));
        Assert.True(File.Exists(Path.Combine(legacy, "logs", "watchdog-1.log")));
    }

    [Fact]
    public void CopyLegacyRoot_does_not_overwrite_existing_destination_files()
    {
        string legacy = Dir("legacy");
        File.WriteAllText(Path.Combine(legacy, "settings.json"), "OLD");
        string newRoot = Dir("renamed");
        File.WriteAllText(Path.Combine(newRoot, "settings.json"), "NEW");

        AppPaths.CopyLegacyRoot(legacy, newRoot);

        Assert.Equal("NEW", File.ReadAllText(Path.Combine(newRoot, "settings.json")));
    }

    [Fact]
    public void RenamedRootAlreadyPopulated_gates_on_marker_or_content()
    {
        string empty = Path.Combine(_root, "missing");
        Assert.False(AppPaths.RenamedRootAlreadyPopulated(empty));

        string populated = Dir("populated");
        Assert.False(AppPaths.RenamedRootAlreadyPopulated(populated));
        File.WriteAllText(Path.Combine(populated, "anything.txt"), "x");
        Assert.True(AppPaths.RenamedRootAlreadyPopulated(populated));

        string marked = Dir("marked");
        File.WriteAllText(Path.Combine(marked, AppPaths.RenameMigrationMarker), "2026-07-14");
        Assert.True(AppPaths.RenamedRootAlreadyPopulated(marked));
    }

    [Fact]
    public void CopyLegacyRoot_then_populated_gate_makes_migration_idempotent()
    {
        string legacy = Dir("legacy");
        File.WriteAllText(Path.Combine(legacy, "client_id.txt"), "id-1");
        string newRoot = Path.Combine(_root, "renamed");

        AppPaths.CopyLegacyRoot(legacy, newRoot);
        Assert.True(AppPaths.RenamedRootAlreadyPopulated(newRoot));

        File.WriteAllText(Path.Combine(newRoot, "client_id.txt"), "id-2");
        AppPaths.CopyLegacyRoot(legacy, newRoot);
        Assert.Equal("id-2", File.ReadAllText(Path.Combine(newRoot, "client_id.txt")));
    }

    [Fact]
    public void MigrateProgramData_copies_files_once_and_leaves_source()
    {
        string legacy = Dir("pd-legacy");
        File.WriteAllText(Path.Combine(legacy, "localhost-youtube-relay-ports.txt"), "51234");
        string newRoot = Path.Combine(_root, "pd-new");

        AppPaths.MigrateProgramData(legacy, newRoot);
        Assert.Equal("51234", File.ReadAllText(Path.Combine(newRoot, "localhost-youtube-relay-ports.txt")));
        Assert.True(File.Exists(Path.Combine(legacy, "localhost-youtube-relay-ports.txt")));

        File.WriteAllText(Path.Combine(legacy, "extra.txt"), "late");
        AppPaths.MigrateProgramData(legacy, newRoot);
        Assert.False(File.Exists(Path.Combine(newRoot, "extra.txt")));
    }

    [Fact]
    public void MigrateLegacyLocalAppState_moves_state_and_plants_marker()
    {
        string source = Dir("localapp");
        File.WriteAllText(Path.Combine(source, "settings.json"), "{}");
        Directory.CreateDirectory(Path.Combine(source, "logs"));
        File.WriteAllText(Path.Combine(source, "logs", "old.log"), "noise");
        string legacyLow = Path.Combine(_root, "locallow-legacy");

        AppPaths.MigrateLegacyLocalAppState(source, legacyLow);

        Assert.Equal("{}", File.ReadAllText(Path.Combine(legacyLow, "settings.json")));
        Assert.False(Directory.Exists(Path.Combine(legacyLow, "logs")));
        Assert.True(File.Exists(Path.Combine(legacyLow, ".migrated-from-localapp")));
        Assert.False(Directory.Exists(source));
    }
}

public class LegacyCompatEnvTests
{
    [Fact]
    public void GetEnvWithLegacyFallback_prefers_new_prefix_over_legacy()
    {
        const string suffix = "TEST_PRECEDENCE_4X9Q";
        try
        {
            Environment.SetEnvironmentVariable("VRCRESOLVER_" + suffix, "new");
            Environment.SetEnvironmentVariable("WKVRCPROXY_" + suffix, "old");
            Assert.Equal("new", LegacyCompat.GetEnvWithLegacyFallback(suffix));
        }
        finally
        {
            Environment.SetEnvironmentVariable("VRCRESOLVER_" + suffix, null);
            Environment.SetEnvironmentVariable("WKVRCPROXY_" + suffix, null);
        }
    }

    [Fact]
    public void GetEnvWithLegacyFallback_falls_back_to_legacy_prefix()
    {
        const string suffix = "TEST_FALLBACK_4X9Q";
        try
        {
            Environment.SetEnvironmentVariable("WKVRCPROXY_" + suffix, "old-opt-out");
            Assert.Equal("old-opt-out", LegacyCompat.GetEnvWithLegacyFallback(suffix));
        }
        finally
        {
            Environment.SetEnvironmentVariable("WKVRCPROXY_" + suffix, null);
        }
    }

    [Fact]
    public void GetEnvWithLegacyFallback_returns_null_when_neither_set()
    {
        Assert.Null(LegacyCompat.GetEnvWithLegacyFallback("TEST_UNSET_4X9Q"));
    }
}
