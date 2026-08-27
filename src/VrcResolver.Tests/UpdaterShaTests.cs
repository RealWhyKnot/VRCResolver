using System.Security.Cryptography;
using Xunit;
using UpdaterProgram = VrcResolver.Updater.Program;

namespace VrcResolver.Tests;

public class UpdaterShaTests
{
    [Fact]
    public void Sha256Line_matches_canonical_release_body()
    {
        const string body = """
        ## vrcresolver v2026.5.4

        Download zip below.

        SHA256: a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3

        ---
        """;
        var match = UpdaterProgram.Sha256Line.Match(body);
        Assert.True(match.Success);
        Assert.Equal("a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
            match.Groups[1].Value);
    }

    [Fact]
    public void Sha256Line_is_case_insensitive_and_tolerates_extra_whitespace()
    {
        var match = UpdaterProgram.Sha256Line.Match("Sha256:   ABCDEF0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789");
        Assert.True(match.Success);
        Assert.Equal("ABCDEF0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789",
            match.Groups[1].Value);
    }

    [Fact]
    public void Sha256Line_does_not_match_short_or_invalid_hex()
    {
        Assert.False(UpdaterProgram.Sha256Line.Match("SHA256: not-hex").Success);
        Assert.False(UpdaterProgram.Sha256Line.Match("SHA256: a665a4592").Success);
        Assert.False(UpdaterProgram.Sha256Line.Match("MD5: a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3").Success);
    }

    [Fact]
    public void Sha256Line_returns_null_match_on_body_without_sha()
    {
        Assert.False(UpdaterProgram.Sha256Line.Match("").Success);
        Assert.False(UpdaterProgram.Sha256Line.Match("Just some random release notes.").Success);
    }

    [Fact]
    public void Sha256Line_does_not_match_integrity_table_row()
    {
        const string body = "vrcresolver-v2026.6.12.0.zip    149.74 MB    SHA256: E8966F33BE8246922756E3E8234CF8309FB6D3151665594203F53BBF5725164B";
        Assert.False(UpdaterProgram.Sha256Line.Match(body).Success);
    }

    [Fact]
    public void TryParseIntegritySha_reads_zip_row()
    {
        const string sha = "e8966f33be8246922756e3e8234cf8309fb6d3151665594203f53bbf5725164b";
        string tsv = "A24EA7D3DF2B0718AFF60B5B9EBEBDF590ED4938D81A6B08CDEC7A880B326B0C\t1\tvrcresolver.exe\n"
            + sha + "\t157017922\tvrcresolver-v2026.6.12.0.zip\n";

        string? parsed = UpdaterProgram.TryParseIntegritySha(tsv, "vrcresolver-v2026.6.12.0.zip");

        Assert.Equal(sha.ToUpperInvariant(), parsed);
    }

    [Fact]
    public void TryParseAssetDigest_reads_github_digest()
    {
        const string sha = "e8966f33be8246922756e3e8234cf8309fb6d3151665594203f53bbf5725164b";

        string? parsed = UpdaterProgram.TryParseAssetDigest("sha256:" + sha);

        Assert.Equal(sha.ToUpperInvariant(), parsed);
    }

    [Fact]
    public void TryParseLegacyBodySha_reads_bare_line()
    {
        const string sha = "e8966f33be8246922756e3e8234cf8309fb6d3151665594203f53bbf5725164b";

        string? parsed = UpdaterProgram.TryParseLegacyBodySha("notes\nSHA256: " + sha + "\nmore");

        Assert.Equal(sha.ToUpperInvariant(), parsed);
    }

    [Fact]
    public void ComputeSha256_matches_known_vector()
    {
        string tempFile = Path.GetTempFileName();
        try
        {
            File.WriteAllText(tempFile, "hello\n");
            string actual = UpdaterProgram.ComputeSha256(tempFile);
            Assert.Equal("5891B5B522D5DF086D0FF0B110FBD9D21BB4FC7163AF34D08286A2E846F6BE03",
                actual, ignoreCase: true);
        }
        finally
        {
            File.Delete(tempFile);
        }
    }

    [Fact]
    public void ComputeSha256_round_trips_with_self()
    {
        string tempFile = Path.GetTempFileName();
        try
        {
            byte[] payload = new byte[] { 0x50, 0x4B, 0x03, 0x04, 0xAA, 0xBB, 0xCC, 0xDD };
            File.WriteAllBytes(tempFile, payload);
            string a = UpdaterProgram.ComputeSha256(tempFile);
            string b = UpdaterProgram.ComputeSha256(tempFile);
            Assert.Equal(a, b);

            using var sha = SHA256.Create();
            string expected = Convert.ToHexString(sha.ComputeHash(payload));
            Assert.Equal(expected, a);
        }
        finally
        {
            File.Delete(tempFile);
        }
    }

    [Theory]
    [InlineData("v2026.5.4.0", 2026, 5, 4, 0)]
    [InlineData("2026.5.4.0", 2026, 5, 4, 0)]
    [InlineData("v2026.5.4.0-1A2B", 2026, 5, 4, 0)]
    [InlineData("v2026.5.4.7-DEAD", 2026, 5, 4, 7)]
    [InlineData("v2026.5.4.7-1234", 2026, 5, 4, 7)]
    public void ParseTagVersion_handles_release_and_dev_shapes(
        string tag, int major, int minor, int build, int rev)
    {
        var v = UpdaterProgram.ParseTagVersion(tag);
        Assert.Equal(new Version(major, minor, build, rev), v);
    }

    [Fact]
    public void ParseTagVersion_rejects_garbage()
    {
        Assert.Throws<InvalidOperationException>(() => UpdaterProgram.ParseTagVersion("not-a-version"));
        Assert.Throws<InvalidOperationException>(() => UpdaterProgram.ParseTagVersion("v"));
        Assert.Throws<InvalidOperationException>(() => UpdaterProgram.ParseTagVersion(""));
    }
}
