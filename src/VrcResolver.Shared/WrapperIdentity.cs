using System.Buffers.Binary;
using System.Diagnostics;
using System.Runtime.Versioning;
using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;

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

    private const int CookieSearchBytes = 4096;

    private const string OurCompanyName = "RealWhyKnot";
    private const string OurProductName = "VRCResolver";

    public static ReadOnlySpan<byte> PyInstallerCookie => "MEI\x0C\x0B\x0A\x0B\x0E"u8;

    public static string Marker => MarkerString;

    public static bool LooksComplete(string path)
    {
        try
        {
            using SafeFileHandle handle = File.OpenHandle(
                path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            long length = RandomAccess.GetLength(handle);

            Span<byte> word = stackalloc byte[4];
            if (!TryReadAt(handle, length, 0x3C, word)) return false;
            long peOffset = BinaryPrimitives.ReadUInt32LittleEndian(word);

            if (!TryReadAt(handle, length, peOffset, word) || !word.SequenceEqual("PE\0\0"u8)) return false;

            Span<byte> magic = stackalloc byte[2];
            if (!TryReadAt(handle, length, peOffset + 24, magic)) return false;
            long directories = BinaryPrimitives.ReadUInt16LittleEndian(magic) switch
            {
                0x10B => peOffset + 24 + 96,
                0x20B => peOffset + 24 + 112,
                _ => -1,
            };
            if (directories < 0) return false;

            Span<byte> security = stackalloc byte[8];
            if (!TryReadAt(handle, length, directories + 32, security)) return false;
            long certificateOffset = BinaryPrimitives.ReadUInt32LittleEndian(security);
            long certificateSize = BinaryPrimitives.ReadUInt32LittleEndian(security[4..]);
            if (certificateSize > 0) return certificateOffset + certificateSize == length;

            Span<byte> tail = stackalloc byte[CookieSearchBytes];
            tail = tail[..(int)Math.Min(length, CookieSearchBytes)];
            if (!TryReadAt(handle, length, length - tail.Length, tail)) return false;
            return tail.IndexOf(PyInstallerCookie) >= 0;
        }
        catch
        {
            return false;
        }
    }

    private static bool TryReadAt(SafeFileHandle handle, long length, long offset, Span<byte> destination)
    {
        if (offset < 0 || offset + destination.Length > length) return false;
        int filled = 0;
        while (filled < destination.Length)
        {
            int read = RandomAccess.Read(handle, destination[filled..], offset + filled);
            if (read <= 0) return false;
            filled += read;
        }
        return true;
    }

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
