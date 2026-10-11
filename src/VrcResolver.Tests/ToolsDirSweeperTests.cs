using System.Runtime.Versioning;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class ToolsDirSweeperTests : IDisposable
{
    private readonly string _tempDir;

    public ToolsDirSweeperTests()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-sweeper-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_tempDir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_tempDir, recursive: true); } catch { }
    }

    private string Touch(string name)
    {
        string path = Path.Combine(_tempDir, name);
        File.WriteAllText(path, "");
        return path;
    }

    private string MakeDir(string name)
    {
        string path = Path.Combine(_tempDir, name);
        Directory.CreateDirectory(path);
        File.WriteAllText(Path.Combine(path, "python313.dll"), "");
        return path;
    }

    [Fact]
    public void Sweep_deletes_known_sidecars_and_leaves_everything_else()
    {
        Touch("yt-dlp.exe.new-ab12cd34");
        Touch("yt-dlp.exe.stale-20260503120000123");
        Touch("yt-dlp-og.exe.new-ef56gh78");
        Touch("yt-dlp-og.exe.stale-20260503120000456");

        Touch("yt-dlp.exe");
        Touch("yt-dlp-og.exe");
        Touch("yt-dlp.exe.bak");
        Touch("yt-dlp.exe.config");
        Touch("yt-dlp.exe.log");
        Touch("yt-dlp-patched.exe.new-xyz123ab");
        Touch("ytdlp.exe.new-ab12cd34");
        Touch("README.txt");
        Touch("VRChat.log");

        ToolsDirSweeper.Sweep(_tempDir);

        var survivors = Directory.GetFiles(_tempDir).Select(Path.GetFileName).Order().ToArray();
        var expected = new[]
        {
            "README.txt",
            "VRChat.log",
            "yt-dlp-og.exe",
            "yt-dlp-patched.exe.new-xyz123ab",
            "yt-dlp.exe",
            "yt-dlp.exe.bak",
            "yt-dlp.exe.config",
            "yt-dlp.exe.log",
            "ytdlp.exe.new-ab12cd34",
        };
        Assert.Equal(expected, survivors);
    }

    [Fact]
    public void Sweep_handles_missing_directory_silently()
    {
        ToolsDirSweeper.Sweep(Path.Combine(_tempDir, "does-not-exist"));
        ToolsDirSweeper.Sweep(null);
        ToolsDirSweeper.Sweep("");
    }

    [Fact]
    public void Sweep_is_case_insensitive()
    {
        Touch("YT-DLP.EXE.NEW-ABCD1234");
        Touch("yt-dlp.EXE.STALE-20260503");

        ToolsDirSweeper.Sweep(_tempDir);

        Assert.Empty(Directory.GetFiles(_tempDir));
    }

    [Fact]
    public void Sweep_deletes_legacy_yt_dlp_wrapper_log()
    {
        Touch("yt-dlp-wrapper.log");
        Touch("YT-DLP-WRAPPER.LOG");
        Touch("yt-dlp-wrapper.log.bak");
        Touch("yt-dlp.exe");

        ToolsDirSweeper.Sweep(_tempDir);

        var survivors = Directory.GetFiles(_tempDir).Select(Path.GetFileName).Order().ToArray();
        Assert.Equal(new[] { "yt-dlp-wrapper.log.bak", "yt-dlp.exe" }, survivors);
    }

    [Fact]
    public void Sweep_deletes_pyinstaller_temp_dirs_whose_owning_process_is_gone()
    {
        MakeDir("_MEI999999998");
        MakeDir("_MEI" + Environment.ProcessId);
        MakeDir("_MEInotapid");
        MakeDir("_MEI");
        MakeDir("notes");
        Touch("yt-dlp.exe");

        ToolsDirSweeper.Sweep(_tempDir);

        var survivors = Directory.GetDirectories(_tempDir).Select(Path.GetFileName).Order().ToArray();
        var expected = new[]
        {
            "_MEI",
            "_MEI" + Environment.ProcessId,
            "_MEInotapid",
            "notes",
        }.Order().ToArray();
        Assert.Equal(expected, survivors);
        Assert.Equal(new[] { "yt-dlp.exe" }, Directory.GetFiles(_tempDir).Select(Path.GetFileName).ToArray());
    }

    [Fact]
    public void Sweep_restores_a_bundled_stale_when_target_and_og_are_missing()
    {
        byte[] bundled = ToolsRestoreTests.BundledBytes(1);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.stale-20261011120000"), bundled);

        ToolsDirSweeper.Sweep(_tempDir);

        Assert.Equal(new[] { "yt-dlp.exe" }, Directory.GetFiles(_tempDir).Select(Path.GetFileName).ToArray());
        Assert.Equal(bundled, File.ReadAllBytes(Path.Combine(_tempDir, "yt-dlp.exe")));
    }

    [Fact]
    public void Sweep_restores_only_the_newest_bundled_stale_and_deletes_the_rest()
    {
        byte[] older = ToolsRestoreTests.BundledBytes(1);
        byte[] newer = ToolsRestoreTests.BundledBytes(2);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.stale-20261010090000"), older);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.stale-20261011120000"), newer);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.new-ab12cd34"), ToolsRestoreTests.WrapperBytes());

        ToolsDirSweeper.Sweep(_tempDir);

        Assert.Equal(new[] { "yt-dlp.exe" }, Directory.GetFiles(_tempDir).Select(Path.GetFileName).ToArray());
        Assert.Equal(newer, File.ReadAllBytes(Path.Combine(_tempDir, "yt-dlp.exe")));
    }

    [Fact]
    public void Sweep_still_deletes_a_wrapper_stale()
    {
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.stale-20261011120000"), ToolsRestoreTests.WrapperBytes());

        ToolsDirSweeper.Sweep(_tempDir);

        Assert.Empty(Directory.GetFiles(_tempDir));
    }

    [Fact]
    public void Sweep_does_not_restore_a_bundled_stale_over_an_existing_target()
    {
        byte[] target = ToolsRestoreTests.BundledBytes(1);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe"), target);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.stale-20261011120000"), ToolsRestoreTests.BundledBytes(2));

        ToolsDirSweeper.Sweep(_tempDir);

        Assert.Equal(new[] { "yt-dlp.exe" }, Directory.GetFiles(_tempDir).Select(Path.GetFileName).ToArray());
        Assert.Equal(target, File.ReadAllBytes(Path.Combine(_tempDir, "yt-dlp.exe")));
    }

    [Fact]
    public void Sweep_does_not_restore_a_bundled_stale_while_the_og_exists()
    {
        byte[] og = ToolsRestoreTests.BundledBytes(1);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp-og.exe"), og);
        File.WriteAllBytes(Path.Combine(_tempDir, "yt-dlp.exe.stale-20261011120000"), ToolsRestoreTests.BundledBytes(2));

        ToolsDirSweeper.Sweep(_tempDir);

        Assert.Equal(new[] { "yt-dlp-og.exe" }, Directory.GetFiles(_tempDir).Select(Path.GetFileName).ToArray());
        Assert.Equal(og, File.ReadAllBytes(Path.Combine(_tempDir, "yt-dlp-og.exe")));
    }
}
