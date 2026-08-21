using System.Net;
using System.Net.Http.Headers;
using System.Runtime.Versioning;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static partial class LocalRelayManifestLocalizer
{
    // Soft sanity cap. The relay used to buffer the entire manifest into
    // memory and 502 anything over 4 MiB. Once the server started appending
    // an HMAC playback_id token to every segment URL (co-watcher gating),
    // long VOD playlists routinely crossed 4 MiB. The current path is a
    // line-by-line streaming rewrite that never holds the full body, so the
    // cap is now just a defense-in-depth bound against a pathological
    // upstream.
    public const int MaxManifestBytes = 64 * 1024 * 1024;

    private static readonly HashSet<string> s_manifestExts = new(StringComparer.OrdinalIgnoreCase)
    {
        "m3u8",
        "mpd",
    };

    public readonly record struct LocalizeStreamResult(
        long CharsIn,
        long CharsOut,
        bool Changed,
        bool Exceeded);

    public static bool IsLikelyManifest(string localPath, string targetUrl, MediaTypeHeaderValue? contentType)
    {
        if (HasManifestExtension(localPath)) return true;
        if (LooksLikeManifestPath(localPath)) return true;
        if (TryGetUriPath(targetUrl, out string targetPath)
            && (HasManifestExtension(targetPath) || LooksLikeManifestPath(targetPath)))
        {
            return true;
        }

        string mediaType = contentType?.MediaType ?? "";
        return mediaType.Contains("mpegurl", StringComparison.OrdinalIgnoreCase)
            || mediaType.Contains("x-mpegurl", StringComparison.OrdinalIgnoreCase)
            || mediaType.Contains("dash+xml", StringComparison.OrdinalIgnoreCase)
            || mediaType.Equals("application/xml", StringComparison.OrdinalIgnoreCase)
            || mediaType.Equals("text/xml", StringComparison.OrdinalIgnoreCase);
    }

    public static string Localize(string content, string localPath)
    {
        if (string.IsNullOrEmpty(content)) return content;

        var output = new StringBuilder(content.Length + 256);
        using var reader = new StringReader(content);
        string? line;
        bool wrote = false;

        while ((line = reader.ReadLine()) != null)
        {
            if (wrote) output.Append('\n');
            wrote = true;
            output.Append(LocalizeLine(line));
        }

        return output.ToString();
    }

    public static async Task<LocalizeStreamResult> LocalizeStreamAsync(
        Stream input,
        Encoding? inputEncoding,
        Stream output,
        long maxChars,
        CancellationToken ct)
    {
        Encoding readerEncoding = inputEncoding ?? Encoding.UTF8;
        using var reader = new StreamReader(
            input,
            readerEncoding,
            detectEncodingFromByteOrderMarks: true,
            bufferSize: 16 * 1024,
            leaveOpen: true);
        var writerEncoding = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false);
        var writer = new StreamWriter(
            output,
            writerEncoding,
            bufferSize: 16 * 1024,
            leaveOpen: true)
        {
            NewLine = "\n",
        };

        long charsIn = 0;
        long charsOut = 0;
        bool changed = false;
        bool exceeded = false;
        bool first = true;
        try
        {
            while (true)
            {
                ct.ThrowIfCancellationRequested();
                string? line = await reader.ReadLineAsync(ct).ConfigureAwait(false);
                if (line == null) break;
                charsIn += line.Length;

                string localized = LocalizeLine(line);
                if (!ReferenceEquals(line, localized)) changed = true;

                if (!first)
                {
                    await writer.WriteAsync('\n').ConfigureAwait(false);
                    charsOut++;
                }
                first = false;

                await writer.WriteAsync(localized.AsMemory(), ct).ConfigureAwait(false);
                charsOut += localized.Length;

                if (charsOut > maxChars)
                {
                    exceeded = true;
                    break;
                }
            }
        }
        finally
        {
            await writer.FlushAsync(ct).ConfigureAwait(false);
            writer.Dispose();
        }

        return new LocalizeStreamResult(charsIn, charsOut, changed, exceeded);
    }

    private static string LocalizeLine(string line)
    {
        string localized = RewriteAbsoluteFirstPartyProxyUrls(line);
        if (!localized.StartsWith("#", StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(localized))
        {
            string trimmed = localized.Trim();
            if (!trimmed.StartsWith("data:", StringComparison.OrdinalIgnoreCase)
                && TryBuildLocalRelativeTarget(trimmed, out string replacement))
            {
                int start = localized.IndexOf(trimmed, StringComparison.Ordinal);
                localized = localized.Substring(0, start)
                    + replacement
                    + localized.Substring(start + trimmed.Length);
            }
        }
        return localized;
    }

    private static string RewriteAbsoluteFirstPartyProxyUrls(string line)
    {
        return AbsoluteUrlRegex().Replace(line, match =>
        {
            string original = WebUtility.HtmlDecode(match.Value.Trim());
            if (!TryBuildLocalRelativeTarget(original, out string replacement))
                return match.Value;

            return replacement;
        });
    }

    private static bool TryBuildLocalRelativeTarget(string original, out string replacement)
    {
        replacement = "";
        if (!Uri.TryCreate(original, UriKind.Absolute, out var uri)) return false;
        if (!FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUri(uri)) return false;

        string ext = FirstPartyUrlPolicy.ExtractAllowedPathExtensionFromPath(uri.AbsolutePath);
        if (string.IsNullOrEmpty(ext)) ext = "bin";
        string name = Path.GetFileName(uri.AbsolutePath);
        if (string.IsNullOrEmpty(name) || !name.Contains('.', StringComparison.Ordinal))
            name = "proxy." + ext;

        string normalized = uri.ToString();
        string encoded = LocalRelayTargetResolver.EncodeTargetParam(normalized);
        replacement = "proxy/" + StableTargetNamespace(normalized) + "/" + name + "?target=" + encoded;

        return true;
    }

    private static string StableTargetNamespace(string targetUrl)
    {
        byte[] hash = SHA256.HashData(Encoding.UTF8.GetBytes(targetUrl));
        return Convert.ToHexString(hash, 0, 8).ToLowerInvariant();
    }

    private static bool HasManifestExtension(string path)
    {
        string ext = FirstPartyUrlPolicy.ExtractAllowedPathExtensionFromPath(path);
        return s_manifestExts.Contains(ext);
    }

    private static bool LooksLikeManifestPath(string path)
    {
        string fileName = Path.GetFileName(path);
        if (fileName.Equals("manifest", StringComparison.OrdinalIgnoreCase))
            return true;
        // A "manifest.<ext>" filename is only a manifest when <ext> is one of
        // the known manifest extensions. The relay also constructs
        // /play/<id>/manifest.mp4 for progressive MP4 responses, and the
        // previous unconditional StartsWith branch swept those into the
        // streaming text rewriter -- the response then went out as
        // Transfer-Encoding: chunked with no Content-Length and AVPro/WMF
        // disconnected on the first byte without playing anything.
        if (fileName.StartsWith("manifest.", StringComparison.OrdinalIgnoreCase))
            return HasManifestExtension(fileName);
        return false;
    }

    private static bool TryGetUriPath(string url, out string path)
    {
        path = "";
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)) return false;
        path = uri.AbsolutePath;
        return true;
    }

    [GeneratedRegex("https?://[^\\s\"'<>]+", RegexOptions.IgnoreCase)]
    private static partial Regex AbsoluteUrlRegex();
}
