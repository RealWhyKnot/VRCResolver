using VrcResolver.Shared;

namespace VrcResolver;

internal static class ClientIdentity
{
    internal const string FileName = "client_id.txt";

    internal const long MaxClientIdFileBytes = 256;

    public static string LoadOrCreate() => LoadOrCreate(DefaultPath());

    internal static string LoadOrCreate(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                var info = new FileInfo(path);
                if (info.Length > MaxClientIdFileBytes)
                {
                }
                else
                {
                    string content = File.ReadAllText(path).Trim();
                    if (Guid.TryParseExact(content, "N", out var g))
                        return g.ToString("N");
                }
            }
        }
        catch { }

        string fresh = Guid.NewGuid().ToString("N");
        string tmp = path + ".new";
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(tmp, fresh);
            File.Move(tmp, path, overwrite: true);
        }
        catch
        {
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { }
        }
        return fresh;
    }

    private static string DefaultPath() =>
        Path.Combine(AppPaths.StateRoot(), FileName);
}
