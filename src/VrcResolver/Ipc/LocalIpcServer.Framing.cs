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
    // " cid=<id>" suffix only when correlation_id is populated.
    private static string CidSuffix(string? correlationId) =>
        string.IsNullOrEmpty(correlationId) ? "" : " cid=" + LogUtil.SanitizeForConsole(correlationId, 64);

    // Append the NDJSON framing newline to a payload byte[] so the wire
    // send is one WriteAsync instead of two (payload + separate newline).
    // Named pipes (PIPE_TYPE_BYTE | PIPE_WAIT) dispatch the write atomically,
    // so coalescing also lets the caller drop the explicit FlushAsync that
    // used to follow the newline write.
    private static byte[] AppendNewline(byte[] payload)
    {
        byte[] framed = new byte[payload.Length + 1];
        Buffer.BlockCopy(payload, 0, framed, 0, payload.Length);
        framed[payload.Length] = (byte)'\n';
        return framed;
    }

    // Returns the line, or null on empty connection. Sets `truncated` to
    // true if MaxRequestBytes was hit before a '\n' arrived — the caller
    // can then surface a "request_too_large" diagnostic instead of
    // confusing "malformed JSON" (which is what JsonSerializer would
    // report against a truncated payload).
    //
    // Buffered: one ReadAsync per 4 KiB chunk, then scan in-process for the
    // newline terminator. Pre-fix this read one byte per syscall — a 100 KiB
    // request needed 100k async syscalls.
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

    // Pass through pre-serialized JSON bytes from MeshClient. Appends the
    // NDJSON framing newline in-place and writes once. No JsonDocument
    // re-encode on the hot path — earlier impl took a JsonDocument and
    // called SerializeToUtf8Bytes(doc.RootElement) here, which re-emitted
    // the same JSON the dispatch handler had just parsed.
    private static async Task WriteFrameAsync(Stream s, byte[] frame, CancellationToken ct)
    {
        byte[] payload = AppendNewline(frame);
        await s.WriteAsync(payload, ct).ConfigureAwait(false);
    }

    // Skip null fields when serializing the synthetic fallback frame so the
    // wire shape stays v1-identical for v1 patched-yt-dlp consumers. Without
    // this, the v2 ResolveResponse fields (container, video_codec, etc.)
    // would each emit "field":null, forcing every fallback recipient to
    // tolerate keys it doesn't know.
    //
    // AOT migration: the WhenWritingNull options used to live here as
    // FallbackSerializerOptions, now baked into MeshFallbackJsonContext
    // via [JsonSourceGenerationOptions(DefaultIgnoreCondition = ...)].
    // The source-gen produces a parallel formatter set for ResolveResponse
    // with the omit-nulls behaviour applied at codegen time.

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
        catch { /* peer may have hung up -- we tried */ }
    }

    // True iff the URL's host is exactly `localhost.youtube.com`. Used for
    // the `[via lh-yt]` console tag and the heartbeat's via-lh-yt counter.
    // Match is exact (not substring); a longer host like
    // `notlocalhost.youtube.com` does NOT count.
    private static bool IsLocalhostYoutubeUrl(string url)
    {
        if (string.IsNullOrEmpty(url)) return false;
        try
        {
            if (Uri.TryCreate(url, UriKind.Absolute, out var u))
                return u.Host.Equals(HostsManager.MarkerHost, StringComparison.OrdinalIgnoreCase);
        }
        catch { /* best-effort */ }
        return false;
    }

    // Bare hostname (host minus optional "www." prefix) for the user-facing
    // per-resolve summary. Path / query are NEVER printed to console — they
    // can carry user-identifying tokens (YouTube video ids, twitch streams,
    // etc.). The full URL stays in the watchdog log file via Logger.
    // Player + target resolution label for the request line. The wrapper
    // doesn't populate maxHeight today (the constraint lives in the
    // vrchat_format_arg's `[height<=N]` selector instead) so we parse that
    // when the explicit field is absent. Falls back to "max" when neither
    // is available.
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
