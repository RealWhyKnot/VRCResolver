using System.Buffers.Binary;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
internal static class PeFixtures
{
    public const int CookieTrailerBytes = 80;

    private const int PeOffset = 0x80;
    private const int DirectoryCount = 16;

    public static byte[] Signed(int length, int certificateLength = 2048, bool pe32Plus = true)
    {
        byte[] data = Skeleton(length, pe32Plus);
        WriteSecurityDirectory(data, pe32Plus, (uint)(length - certificateLength), (uint)certificateLength);
        return data;
    }

    public static byte[] Unsigned(int length, bool withCookie, bool pe32Plus = true)
    {
        byte[] data = Skeleton(length, pe32Plus);
        if (withCookie)
        {
            int cookieAt = length - WrapperIdentity.PyInstallerCookie.Length - CookieTrailerBytes;
            WrapperIdentity.PyInstallerCookie.CopyTo(data.AsSpan(cookieAt));
            data.AsSpan(cookieAt + WrapperIdentity.PyInstallerCookie.Length).Clear();
        }
        return data;
    }

    public static byte[] NonPe(int length)
    {
        byte[] data = new byte[length];
        for (int i = 0; i < data.Length; i++) data[i] = (byte)((i * 7 + 1) & 0xFF);
        return data;
    }

    public static byte[] Truncated(byte[] data, int bytesCut) => data[..^bytesCut];

    private static byte[] Skeleton(int length, bool pe32Plus)
    {
        byte[] data = NonPe(length);
        int optionalHeader = PeOffset + 24;
        int directories = optionalHeader + (pe32Plus ? 112 : 96);
        data.AsSpan(PeOffset, directories + DirectoryCount * 8 - PeOffset).Clear();

        data[0] = (byte)'M';
        data[1] = (byte)'Z';
        BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(0x3C), PeOffset);
        "PE\0\0"u8.CopyTo(data.AsSpan(PeOffset));
        BinaryPrimitives.WriteUInt16LittleEndian(data.AsSpan(optionalHeader), (ushort)(pe32Plus ? 0x20B : 0x10B));
        return data;
    }

    private static void WriteSecurityDirectory(byte[] data, bool pe32Plus, uint offset, uint size)
    {
        int directories = PeOffset + 24 + (pe32Plus ? 112 : 96);
        BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(directories + 32), offset);
        BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(directories + 36), size);
    }
}
