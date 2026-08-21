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
