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

    // Raise a request to the best rung its source offers. Returns true when the request was
    // actually changed, so callers can log the one case that matters.
    //
    // Clearing VrchatFormatArg is the load-bearing half. The server prepends that selector
    // ahead of the chain it builds and yt-dlp takes the first clause that matches, so leaving
    // VRChat's own height<= in place would pin the result to the height VRChat asked for and
    // the raised cap would do nothing at all.
    //
    // AVPro only. The Unity player is progressive H.264 capped at 720, and InferPlayer above
    // derives that from the very cap this would overwrite.
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

    // Returns the value following "-f" or "--format", or null if absent.
    // Matches the form `-f <selector>` and `--format <selector>`; does NOT
    // attempt to handle `--format=<selector>` (VRChat uses the spaced form).
    // Sits beside TryGetHeightCap, which consumes its output.
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
