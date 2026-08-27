using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class LocalIpcServer : IDisposable
{
    [GeneratedRegex(@"height<=(\d+)")]
    private static partial Regex HeightCapRegex();
    private static string CidSuffix(string? correlationId) =>
        string.IsNullOrEmpty(correlationId) ? "" : " cid=" + LogUtil.SanitizeForConsole(correlationId, 64);

    private static byte[] AppendNewline(byte[] payload)
    {
        byte[] framed = new byte[payload.Length + 1];
        Buffer.BlockCopy(payload, 0, framed, 0, payload.Length);
        framed[payload.Length] = (byte)'\n';
        return framed;
    }

    private static async Task<(string? Line, bool Truncated)> ReadLineAsync(Stream s, CancellationToken ct)
    {
        using var ms = new MemoryStream();
        var buf = new byte[4096];
        bool sawNewline = false;
        while (ms.Length < MaxRequestBytes)
        {
            int n = await s.ReadAsync(buf.AsMemory(), ct).ConfigureAwait(false);
            if (n == 0) break;
            int consume = n;
            int nlIdx = Array.IndexOf(buf, (byte)'\n', 0, n);
            if (nlIdx >= 0) { sawNewline = true; consume = nlIdx; }
            for (int i = 0; i < consume && ms.Length < MaxRequestBytes; i++)
            {
                byte b = buf[i];
                if (b == (byte)'\r') continue;
                ms.WriteByte(b);
            }
            if (sawNewline) break;
        }
        if (ms.Length == 0) return (null, false);
        bool truncated = !sawNewline && ms.Length >= MaxRequestBytes;
        return (Encoding.UTF8.GetString(ms.GetBuffer(), 0, (int)ms.Length), truncated);
    }

    private static async Task WriteFrameAsync(Stream s, byte[] frame, CancellationToken ct)
    {
        byte[] payload = AppendNewline(frame);
        await s.WriteAsync(payload, ct).ConfigureAwait(false);
    }

    private static async Task WriteFallbackAsync(Stream s, string id, string reason, CancellationToken ct)
    {
        var frame = new ResolveResponse
        {
            Action = WireConstants.ActionFallbackNative,
            Id = id,
            Reason = reason,
        };
        byte[] payload = AppendNewline(
            JsonSerializer.SerializeToUtf8Bytes(frame, MeshFallbackJsonContext.Default.ResolveResponse));
        try
        {
            await s.WriteAsync(payload, ct).ConfigureAwait(false);
        }
        catch { }
    }

    private static bool IsLocalhostYoutubeUrl(string url)
    {
        if (string.IsNullOrEmpty(url)) return false;
        try
        {
            if (Uri.TryCreate(url, UriKind.Absolute, out var u))
                return u.Host.Equals(HostsManager.MarkerHost, StringComparison.OrdinalIgnoreCase);
        }
        catch { }
        return false;
    }

    private static string FormatPlayerLabel(ResolveRequest req)
    {
        string player = req.Player == WireConstants.PlayerUnity ? "Unity" : "AVPro";
        if (req.MaxHeight is int mh && mh > 0)
            return player + " " + mh + "p";
        if (!string.IsNullOrEmpty(req.VrchatFormatArg))
        {
            var m = HeightCapRegex().Match(req.VrchatFormatArg);
            if (m.Success) return player + " " + m.Groups[1].Value + "p";
        }
        return player + " max";
    }

}
