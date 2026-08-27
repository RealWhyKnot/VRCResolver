using System.Runtime.Versioning;

namespace VrcResolver.Shared;

[SupportedOSPlatform("windows")]
public static class VrcPathLocator
{
    public static string? Find(string? customPath = null)
    {
        if (!string.IsNullOrEmpty(customPath) && Directory.Exists(customPath))
            return customPath;

        string localLow = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData) + "Low",
            "VRChat", "VRChat", "Tools");
        if (Directory.Exists(localLow)) return localLow;

        return null;
    }
}
