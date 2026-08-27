using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

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
}
