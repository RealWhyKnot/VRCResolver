using System.Diagnostics;
using System.Runtime.Versioning;
using System.Security.Cryptography;

namespace VrcResolver.Shared;

public enum WrapperKind
{
    Ours,
    VrcBundledYtDlp,
    Unknown,
}

[SupportedOSPlatform("windows")]
public static class WrapperIdentity
{
    private const string MarkerString =
        "VRCRESOLVER_WRAPPER_MARKER_v1:6f2a9c41-8d5e-4b7a-a3c9-1e8f7d2b4a60";
    private static ReadOnlySpan<byte> MarkerUtf8 =>
        "VRCRESOLVER_WRAPPER_MARKER_v1:6f2a9c41-8d5e-4b7a-a3c9-1e8f7d2b4a60"u8;

    public const int MaxScanBytes = 16 * 1024 * 1024;
    public const long OursSizeCeiling = 10L * 1024 * 1024;

    private const string OurCompanyName = "RealWhyKnot";
    private const string OurProductName = "VRCResolver";

    public static string Marker => MarkerString;

    public static WrapperKind Classify(string path, string? knownHashesPath = null)
    {
        if (string.IsNullOrEmpty(path) || !File.Exists(path)) return WrapperKind.Unknown;

        long size;
        try { size = new FileInfo(path).Length; }
        catch { return WrapperKind.Unknown; }

        if (size <= MaxScanBytes && ContainsMarker(path)) return WrapperKind.Ours;

        try
        {
            var fvi = FileVersionInfo.GetVersionInfo(path);
            if (string.Equals(fvi.CompanyName, OurCompanyName, StringComparison.Ordinal)
                && (string.Equals(fvi.ProductName, OurProductName, StringComparison.Ordinal)
                    || string.Equals(fvi.ProductName, LegacyCompat.LegacyProductName, StringComparison.Ordinal)))
            {
                return WrapperKind.Ours;
            }
        }
        catch { }

        if (!string.IsNullOrEmpty(knownHashesPath) && File.Exists(knownHashesPath))
        {
            string? sha = ComputeSha256(path);
            if (!string.IsNullOrEmpty(sha) && KnownHashListContains(knownHashesPath, sha))
                return WrapperKind.Ours;
        }

        return size > OursSizeCeiling ? WrapperKind.VrcBundledYtDlp : WrapperKind.Unknown;
    }

    public static bool ContainsMarker(string path)
    {
        if (string.IsNullOrEmpty(path) || !File.Exists(path)) return false;
        try
        {
            long size = new FileInfo(path).Length;
            if (size <= 0 || size > MaxScanBytes) return false;

            using var fs = new FileStream(
                path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            byte[] buf = new byte[(int)size];
            int read = 0;
            while (read < buf.Length)
            {
                int n = fs.Read(buf, read, buf.Length - read);
                if (n <= 0) break;
                read += n;
            }
            if (read < MarkerUtf8.Length) return false;
            var span = buf.AsSpan(0, read);
            return span.IndexOf(MarkerUtf8) >= 0
                || span.IndexOf(LegacyCompat.LegacyWrapperMarkerUtf8) >= 0;
        }
        catch
        {
            return false;
        }
    }

    public static string? ComputeSha256(string path)
    {
        try
        {
            using var fs = new FileStream(
                path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            byte[] hash = SHA256.HashData(fs);
            return Convert.ToHexString(hash).ToLowerInvariant();
        }
        catch
        {
            return null;
        }
    }

    public static bool KnownHashListContains(string listPath, string sha256Hex)
    {
        try
        {
            foreach (string raw in File.ReadLines(listPath))
            {
                string line = raw.Trim();
                if (line.Length == 0 || line[0] == '#') continue;
                int sp = line.IndexOf(' ');
                string head = sp < 0 ? line : line.Substring(0, sp);
                if (string.Equals(head, sha256Hex, StringComparison.OrdinalIgnoreCase))
                    return true;
            }
        }
        catch { }
        return false;
    }
}
