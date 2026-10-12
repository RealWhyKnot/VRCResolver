using System.Diagnostics;
using System.Runtime.Versioning;
using System.Text;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class ToolsRestoreTests : IDisposable
{
    private readonly string _toolsDir;

    public ToolsRestoreTests()
    {
        ToolsRestore.MoveFile = File.Move;
        ToolsRestore.ComputeHash = WrapperIdentity.ComputeSha256;
        _toolsDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-toolsrestore-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_toolsDir);
    }

    public void Dispose()
    {
        ToolsRestore.MoveFile = File.Move;
        ToolsRestore.ComputeHash = WrapperIdentity.ComputeSha256;
        try { Directory.Delete(_toolsDir, recursive: true); } catch { }
    }

    private string TargetPath => Path.Combine(_toolsDir, "yt-dlp.exe");
    private string OgPath => Path.Combine(_toolsDir, "yt-dlp-og.exe");

    internal static byte[] BundledBytes(int seed)
    {
        byte[] content = new byte[(int)WrapperIdentity.OursSizeCeiling + 4096];
        for (int i = 0; i < content.Length; i++) content[i] = (byte)((i * 7 + seed) & 0xFF);
        return content;
    }

    internal static byte[] CompleteBundledBytes(int seed)
        => PeFixtures.Signed((int)WrapperIdentity.OursSizeCeiling + 4096, certificateLength: 1024 * seed);

    internal static byte[] WrapperBytes()
    {
        byte[] content = new byte[8192];
        for (int i = 0; i < content.Length; i++) content[i] = (byte)((i * 31) & 0xFF);
        Encoding.UTF8.GetBytes(WrapperIdentity.Marker).CopyTo(content, 256);
        return content;
    }

    private string[] FileNames() => Directory.GetFiles(_toolsDir).Select(p => Path.GetFileName(p)).Order().ToArray();

    [Fact]
    public void Restore_moves_a_running_wrapper_aside()
    {
        string ping = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "PING.EXE");
        File.Copy(ping, TargetPath);
        byte[] og = BundledBytes(1);
        File.WriteAllBytes(OgPath, og);

        using var process = Process.Start(new ProcessStartInfo(TargetPath, "-n 30 127.0.0.1")
        {
            UseShellExecute = false,
            CreateNoWindow = true,
        })!;
        try
        {
            RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out string detail);

            Assert.Equal(RestoreOutcome.Restored, outcome);
            Assert.NotEqual("", detail);
            Assert.Equal(og, File.ReadAllBytes(TargetPath));
            Assert.False(File.Exists(OgPath));
            Assert.Single(Directory.GetFiles(_toolsDir, "yt-dlp.exe.stale-*"));
            Assert.False(process.HasExited);
        }
        finally
        {
            try { process.Kill(); } catch { }
            process.WaitForExit();
        }
    }

    [Fact]
    public void Restore_keeps_a_newer_vrchat_binary()
    {
        byte[] newer = CompleteBundledBytes(1);
        File.WriteAllBytes(TargetPath, newer);
        File.WriteAllBytes(OgPath, CompleteBundledBytes(2));

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out string detail);

        Assert.Equal(RestoreOutcome.KeptNewerVrchatBinary, outcome);
        Assert.NotEqual("", detail);
        Assert.Equal(newer, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Restore_moves_the_og_over_a_vrchat_binary_that_is_not_complete()
    {
        File.WriteAllBytes(TargetPath, PeFixtures.Truncated(CompleteBundledBytes(1), 100));
        byte[] og = CompleteBundledBytes(2);
        File.WriteAllBytes(OgPath, og);

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.Restored, outcome);
        Assert.Equal(og, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Theory]
    [InlineData("yt-dlp.exe")]
    [InlineData("yt-dlp-og.exe")]
    public void Restore_moves_the_og_over_a_vrchat_binary_when_a_hash_fails(string unhashable)
    {
        File.WriteAllBytes(TargetPath, CompleteBundledBytes(1));
        byte[] og = CompleteBundledBytes(2);
        File.WriteAllBytes(OgPath, og);
        ToolsRestore.ComputeHash = path => Path.GetFileName(path) == unhashable ? null : WrapperIdentity.ComputeSha256(path);

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.Restored, outcome);
        Assert.Equal(og, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Restore_never_destroys_the_target_when_the_og_vanishes()
    {
        byte[] original = WrapperBytes();
        File.WriteAllBytes(TargetPath, original);
        File.WriteAllBytes(OgPath, BundledBytes(1));

        int calls = 0;
        ToolsRestore.MoveFile = (from, to, overwrite) =>
        {
            calls++;
            if (calls == 1) throw new IOException("simulated lock");
            throw new FileNotFoundException("og vanished", from);
        };

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out string detail);

        Assert.Equal(RestoreOutcome.Failed, outcome);
        Assert.NotEqual("", detail);
        Assert.True(File.Exists(TargetPath));
        Assert.Equal(original, File.ReadAllBytes(TargetPath));
        Assert.Empty(Directory.GetFiles(_toolsDir, "*.stale-*"));
    }

    [Fact]
    public void Restore_leaves_the_target_alone_when_the_og_is_gone_after_the_first_move_fails()
    {
        byte[] original = WrapperBytes();
        File.WriteAllBytes(TargetPath, original);
        File.WriteAllBytes(OgPath, BundledBytes(1));

        int calls = 0;
        ToolsRestore.MoveFile = (from, to, overwrite) =>
        {
            calls++;
            File.Delete(from);
            throw new IOException("simulated lock");
        };

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.Failed, outcome);
        Assert.Equal(1, calls);
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
        Assert.Equal(original, File.ReadAllBytes(TargetPath));
    }

    [Fact]
    public void Restore_puts_the_target_back_when_the_og_cannot_move_in_after_the_stale_step()
    {
        byte[] original = WrapperBytes();
        File.WriteAllBytes(TargetPath, original);
        byte[] og = BundledBytes(1);
        File.WriteAllBytes(OgPath, og);

        int calls = 0;
        ToolsRestore.MoveFile = (from, to, overwrite) =>
        {
            calls++;
            if (calls == 1) throw new UnauthorizedAccessException("simulated lock");
            throw new IOException("simulated failure");
        };

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.Failed, outcome);
        Assert.Equal(original, File.ReadAllBytes(TargetPath));
        Assert.Equal(og, File.ReadAllBytes(OgPath));
        Assert.Empty(Directory.GetFiles(_toolsDir, "*.stale-*"));
    }

    [Fact]
    public void Restore_without_og_removes_our_wrapper()
    {
        File.WriteAllBytes(TargetPath, WrapperBytes());

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out string detail);

        Assert.Equal(RestoreOutcome.RemovedOrphanWrapper, outcome);
        Assert.NotEqual("", detail);
        Assert.Empty(Directory.GetFiles(_toolsDir));
    }

    [Fact]
    public void Restore_without_og_leaves_a_target_that_is_not_ours()
    {
        byte[] vrc = BundledBytes(1);
        File.WriteAllBytes(TargetPath, vrc);

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.NothingToDo, outcome);
        Assert.Equal(vrc, File.ReadAllBytes(TargetPath));
    }

    [Fact]
    public void Restore_with_nothing_in_tools_has_nothing_to_do()
    {
        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.NothingToDo, outcome);
        Assert.Empty(Directory.GetFiles(_toolsDir));
    }

    [Fact]
    public void Restore_with_only_the_og_moves_it_into_place()
    {
        byte[] og = BundledBytes(1);
        File.WriteAllBytes(OgPath, og);

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out string detail);

        Assert.Equal(RestoreOutcome.Restored, outcome);
        Assert.NotEqual("", detail);
        Assert.Equal(og, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Restore_replaces_an_idle_wrapper_without_leaving_a_stale()
    {
        File.WriteAllBytes(TargetPath, WrapperBytes());
        byte[] og = BundledBytes(1);
        File.WriteAllBytes(OgPath, og);

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.Restored, outcome);
        Assert.Equal(og, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Restore_replaces_a_vrchat_binary_that_matches_the_og()
    {
        byte[] same = BundledBytes(1);
        File.WriteAllBytes(TargetPath, same);
        File.WriteAllBytes(OgPath, same);

        RestoreOutcome outcome = ToolsRestore.Restore(_toolsDir, out _);

        Assert.Equal(RestoreOutcome.Restored, outcome);
        Assert.Equal(same, File.ReadAllBytes(TargetPath));
        Assert.Equal(new[] { "yt-dlp.exe" }, FileNames());
    }

    [Fact]
    public void Restore_fails_for_a_missing_directory()
    {
        RestoreOutcome outcome = ToolsRestore.Restore(Path.Combine(_toolsDir, "does-not-exist"), out string detail);

        Assert.Equal(RestoreOutcome.Failed, outcome);
        Assert.NotEqual("", detail);
    }

    [Fact]
    public void Restore_details_never_carry_a_full_path()
    {
        File.WriteAllBytes(TargetPath, WrapperBytes());
        File.WriteAllBytes(OgPath, BundledBytes(1));
        ToolsRestore.MoveFile = (_, _, _) => throw new IOException("simulated at " + _toolsDir);

        ToolsRestore.Restore(_toolsDir, out string detail);

        Assert.DoesNotContain(_toolsDir, detail);
        Assert.DoesNotContain(Path.GetTempPath(), detail);
        Assert.True(detail.All(c => c >= 0x20 && c < 0x7F));
    }
}
