using Xunit;

namespace VrcResolver.Tests;

public class ClientIdentityTests : IDisposable
{
    private readonly string _tempDir;
    private readonly string _path;

    public ClientIdentityTests()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-clientid-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_tempDir);
        _path = Path.Combine(_tempDir, ClientIdentity.FileName);
    }

    public void Dispose()
    {
        try { Directory.Delete(_tempDir, recursive: true); } catch { }
    }

    [Fact]
    public void LoadOrCreate_FirstCall_GeneratesAndPersists()
    {
        Assert.False(File.Exists(_path));
        string id = ClientIdentity.LoadOrCreate(_path);
        Assert.Matches("^[0-9a-fA-F]{32}$", id);
        Assert.True(File.Exists(_path));
        Assert.Equal(id, File.ReadAllText(_path).Trim());
    }

    [Fact]
    public void LoadOrCreate_SubsequentCalls_ReturnSameIdentity()
    {
        string first = ClientIdentity.LoadOrCreate(_path);
        string second = ClientIdentity.LoadOrCreate(_path);
        string third = ClientIdentity.LoadOrCreate(_path);
        Assert.Equal(first, second);
        Assert.Equal(second, third);
    }

    [Fact]
    public void LoadOrCreate_CorruptFile_RegeneratesAndOverwrites()
    {
        File.WriteAllText(_path, "not-a-guid");
        string id = ClientIdentity.LoadOrCreate(_path);
        Assert.Matches("^[0-9a-fA-F]{32}$", id);
        Assert.Equal(id, File.ReadAllText(_path).Trim());

        string id2 = ClientIdentity.LoadOrCreate(_path);
        Assert.Equal(id, id2);
    }

    [Fact]
    public void LoadOrCreate_FileWithLeadingTrailingWhitespace_AcceptedAfterTrim()
    {
        var g = Guid.NewGuid();
        File.WriteAllText(_path, "  " + g.ToString("N") + "\r\n");
        string id = ClientIdentity.LoadOrCreate(_path);
        Assert.Equal(g.ToString("N"), id);
    }

    [Fact]
    public void LoadOrCreate_GuidWithDashesFormat_RejectedAsCorrupt()
    {
        var g = Guid.NewGuid();
        File.WriteAllText(_path, g.ToString("D"));
        string id = ClientIdentity.LoadOrCreate(_path);
        Assert.NotEqual(g.ToString("N"), id);
        Assert.Matches("^[0-9a-fA-F]{32}$", id);
    }

    [Fact]
    public void LoadOrCreate_AtomicWrite_LeavesNoTmpResidue()
    {
        ClientIdentity.LoadOrCreate(_path);
        Assert.True(File.Exists(_path));
        Assert.False(File.Exists(_path + ".new"));
    }

    [Fact]
    public void LoadOrCreate_MissingParentDir_StillReturnsValidGuid()
    {
        string deepPath = Path.Combine(_tempDir, "subdir", "nested", ClientIdentity.FileName);
        Assert.False(Directory.Exists(Path.GetDirectoryName(deepPath)!));
        string id = ClientIdentity.LoadOrCreate(deepPath);
        Assert.Matches("^[0-9a-fA-F]{32}$", id);
        Assert.True(File.Exists(deepPath));
    }

    [Fact]
    public void LoadOrCreate_OversizeFile_RegeneratesFresh()
    {
        long cap = ClientIdentity.MaxClientIdFileBytes;
        File.WriteAllBytes(_path, new byte[cap + 1]);

        string id = ClientIdentity.LoadOrCreate(_path);

        Assert.Matches("^[0-9a-fA-F]{32}$", id);
        Assert.Equal(id, File.ReadAllText(_path).Trim());
        Assert.Equal(32, new FileInfo(_path).Length);
    }

    [Fact]
    public void LoadOrCreate_SaveFailure_CleansTmpResidue()
    {
        string seeded = ClientIdentity.LoadOrCreate(_path);
        Assert.True(File.Exists(_path));

        using (var lockFs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.None))
        {
        }

        Assert.False(File.Exists(_path + ".new"),
            "After successful LoadOrCreate, no tmp residue should remain");

        using (var lockFs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.None))
        {
            string id = ClientIdentity.LoadOrCreate(_path);
            Assert.Matches("^[0-9a-fA-F]{32}$", id);
        }

        Assert.False(File.Exists(_path + ".new"),
            "LoadOrCreate catch should have deleted the .new tmp residue");
    }
}
