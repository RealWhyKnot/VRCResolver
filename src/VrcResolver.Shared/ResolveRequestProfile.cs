using System.Text;

namespace VrcResolver.Shared;

public static class ResolveRequestProfile
{
    private const string HeightCapNeedle = "height<=";

    public static int? TryGetHeightCap(string? formatArg)
    {
        if (string.IsNullOrEmpty(formatArg)) return null;

        int searchFrom = 0;
        while (searchFrom < formatArg.Length)
        {
            int idx = formatArg.IndexOf(HeightCapNeedle, searchFrom, StringComparison.OrdinalIgnoreCase);
            if (idx < 0) return null;

            int valueStart = idx + HeightCapNeedle.Length;
            if (valueStart < formatArg.Length && formatArg[valueStart] == '?')
                valueStart++;

            int valueEnd = valueStart;
            while (valueEnd < formatArg.Length && char.IsDigit(formatArg[valueEnd]))
                valueEnd++;

            if (valueEnd > valueStart
                && int.TryParse(formatArg.AsSpan(valueStart, valueEnd - valueStart), out int cap)
                && cap > 0)
            {
                return cap;
            }

            searchFrom = idx + HeightCapNeedle.Length;
        }

        return null;
    }

    public static string InferPlayer(string? formatArg)
    {
        int? heightCap = TryGetHeightCap(formatArg);
        return heightCap == 720
            ? WireConstants.PlayerUnity
            : WireConstants.PlayerAvPro;
    }

    public static bool ApplyHighQuality(ResolveRequest request, bool enabled)
    {
        if (request == null) throw new ArgumentNullException(nameof(request));
        if (!enabled) return false;
        if (!string.Equals(request.Player, WireConstants.PlayerAvPro, StringComparison.Ordinal))
            return false;

        request.MaxHeight = WireConstants.HighQualityMaxHeight;
        request.VrchatFormatArg = null;
        request.PreferHighest = true;
        return true;
    }

    public static bool ApplyDefaultQualityCap(ResolveRequest request)
    {
        if (request == null) throw new ArgumentNullException(nameof(request));
        if (!string.Equals(request.Player, WireConstants.PlayerAvPro, StringComparison.Ordinal))
            return false;

        int capped = request.MaxHeight is int mh && mh > 0
            ? Math.Min(mh, WireConstants.DefaultMaxHeight)
            : WireConstants.DefaultMaxHeight;

        bool changed = request.MaxHeight != capped;
        request.MaxHeight = capped;

        string? rewritten = CapHeightIn(request.VrchatFormatArg, capped);
        if (!string.Equals(rewritten, request.VrchatFormatArg, StringComparison.Ordinal))
        {
            request.VrchatFormatArg = rewritten;
            changed = true;
        }
        return changed;
    }

    internal static string? CapHeightIn(string? formatArg, int cap)
    {
        if (string.IsNullOrEmpty(formatArg) || cap <= 0) return formatArg;

        StringBuilder? sb = null;
        int copied = 0;
        int searchFrom = 0;
        while (searchFrom < formatArg.Length)
        {
            int idx = formatArg.IndexOf(HeightCapNeedle, searchFrom, StringComparison.OrdinalIgnoreCase);
            if (idx < 0) break;

            int valueStart = idx + HeightCapNeedle.Length;
            if (valueStart < formatArg.Length && formatArg[valueStart] == '?')
                valueStart++;

            int valueEnd = valueStart;
            while (valueEnd < formatArg.Length && char.IsDigit(formatArg[valueEnd]))
                valueEnd++;

            if (valueEnd > valueStart
                && int.TryParse(formatArg.AsSpan(valueStart, valueEnd - valueStart), out int value)
                && value > cap)
            {
                sb ??= new StringBuilder(formatArg.Length);
                sb.Append(formatArg, copied, valueStart - copied);
                sb.Append(cap);
                copied = valueEnd;
            }

            searchFrom = valueEnd > valueStart ? valueEnd : idx + HeightCapNeedle.Length;
        }

        if (sb == null) return formatArg;
        sb.Append(formatArg, copied, formatArg.Length - copied);
        return sb.ToString();
    }

    public static string? ExtractDashFValue(string[] args)
    {
        for (int i = 0; i < args.Length - 1; i++)
        {
            if (args[i] == "-f" || args[i] == "--format")
                return args[i + 1];
        }
        return null;
    }
}
