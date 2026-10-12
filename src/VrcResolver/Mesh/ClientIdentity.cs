using VrcResolver.Shared;

namespace VrcResolver;

internal static class ClientIdentity
{
    internal const string FileName = "client_id.txt";

    internal const long MaxClientIdFileBytes = 256;

    private const int ReadRetries = 3;
    private const int ReadRetryDelayMs = 50;

    public static string LoadOrCreate() => LoadOrCreate(DefaultPath());

    internal static string LoadOrCreate(string path)
    {
        for (int attempt = 0; attempt <= ReadRetries; attempt++)
        {
            try
            {
                if (!File.Exists(path)) break;
                if (new FileInfo(path).Length > MaxClientIdFileBytes) break;
                string content = File.ReadAllText(path).Trim();
                if (Guid.TryParseExact(content, "N", out var g))
                    return g.ToString("N");
                break;
            }
            catch (Exception ex) when (ex is FileNotFoundException or DirectoryNotFoundException)
            {
                break;
            }
            catch (IOException) when (attempt < ReadRetries)
            {
                Thread.Sleep(ReadRetryDelayMs);
            }
            catch
            {
                return Guid.NewGuid().ToString("N");
            }
        }

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
