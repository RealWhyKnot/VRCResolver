using System.Globalization;
using System.Runtime.Versioning;
using Microsoft.Win32;

namespace VrcResolver.Shared;

[SupportedOSPlatform("windows")]
public static class WinInetConnectionLimit
{
    public const int Target = 16;
    public const string KeyPath = @"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    public const string ValueName = "MaxConnectionsPerServer";
    private const string MarkerFileName = "wininet_limit_previous.txt";
    private const string Absent = "absent";

    public static int? DecideRaise(int? current) => current is >= Target ? null : Target;

    public static void Ensure(Action<string> log)
    {
        using RegistryKey key = Registry.CurrentUser.CreateSubKey(KeyPath);
        Ensure(key, Path.Combine(AppPaths.StateRoot(), MarkerFileName), log);
    }

    public static void Restore(Action<string> log)
    {
        using RegistryKey? key = Registry.CurrentUser.OpenSubKey(KeyPath, writable: true);
        string marker = Path.Combine(AppPaths.StateRoot(), MarkerFileName);
        if (key == null)
        {
            TryDelete(marker);
            return;
        }
        Restore(key, marker, log);
    }

    public static void Ensure(RegistryKey key, string markerPath, Action<string> log)
    {
        int? current = ReadCurrent(key);
        if (DecideRaise(current) is not int target) return;

        if (!File.Exists(markerPath))
        {
            Directory.CreateDirectory(Path.GetDirectoryName(markerPath)!);
            File.WriteAllText(markerPath, current?.ToString(CultureInfo.InvariantCulture) ?? Absent);
        }
        key.SetValue(ValueName, target, RegistryValueKind.DWord);
        log("raised WinINet connections per server from "
            + (current?.ToString(CultureInfo.InvariantCulture) ?? "default 2") + " to " + target
            + "; VRChat picks this up the next time it starts");
    }

    public static void Restore(RegistryKey key, string markerPath, Action<string> log)
    {
        if (!File.Exists(markerPath)) return;
        string previous = File.ReadAllText(markerPath).Trim();

        if (ReadCurrent(key) == Target)
        {
            if (int.TryParse(previous, NumberStyles.Integer, CultureInfo.InvariantCulture, out int value))
            {
                key.SetValue(ValueName, value, RegistryValueKind.DWord);
                log("restored WinINet connections per server to " + value);
            }
            else
            {
                key.DeleteValue(ValueName, throwOnMissingValue: false);
                log("removed WinINet connections per server override");
            }
        }
        TryDelete(markerPath);
    }

    private static int? ReadCurrent(RegistryKey key)
        => key.GetValue(ValueName) is int value ? value : null;

    private static void TryDelete(string path)
    {
        try { if (File.Exists(path)) File.Delete(path); }
        catch { }
    }
}
