using System.Text;

namespace VrcResolver.Shared;

public static class Base64UrlText
{
    public static string Encode(string value)
    {
        return Convert.ToBase64String(Encoding.UTF8.GetBytes(value))
            .Replace('+', '-')
            .Replace('/', '_')
            .TrimEnd('=');
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
