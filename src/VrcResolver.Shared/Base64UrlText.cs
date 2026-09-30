using System.Buffers.Text;
using System.Text;

namespace VrcResolver.Shared;

public static class Base64UrlText
{
    public static string Encode(string value)
    {
        int max = Encoding.UTF8.GetMaxByteCount(value.Length);
        if (max > 1024)
            return Base64Url.EncodeToString(Encoding.UTF8.GetBytes(value));

        Span<byte> buffer = stackalloc byte[1024];
        int written = Encoding.UTF8.GetBytes(value, buffer);
        return Base64Url.EncodeToString(buffer[..written]);
    }

    public static bool TryDecode(string? encoded, out string value)
    {
        value = "";
        if (string.IsNullOrWhiteSpace(encoded)) return false;
        try
        {
            string b64 = encoded.Replace('-', '+').Replace('_', '/');
            switch (b64.Length % 4)
            {
                case 2: b64 += "=="; break;
                case 3: b64 += "="; break;
            }
            value = Encoding.UTF8.GetString(Convert.FromBase64String(b64));
            return true;
        }
        catch
        {
            value = "";
            return false;
        }
    }
}
