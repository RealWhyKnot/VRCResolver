using System.Text;

namespace VrcResolver.Shared;

// The one base64url text codec (RFC 4648 alphabet, padding stripped) shared by
// the trust-gateway URL builder and the relay target resolver. Verbatim
// consolidation of the logic both carried -- TryDecode tolerates padded AND
// unpadded input, which is wire-adjacent behavior the BCL Base64Url type does
// not promise, so it stays hand-rolled.
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
