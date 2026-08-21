namespace VrcResolver;

internal static class TerminalEffectEngine
{
    public static string ActivityGlyph(TerminalGlyphSet glyphs, bool active, int frame, bool animationsEnabled)
    {
        if (!active)
            return "";
        if (!animationsEnabled)
            return glyphs == TerminalGlyphSet.Ascii ? "*" : "●";

        string[] spinner = glyphs.Spinner;
        return spinner[(frame & int.MaxValue) % spinner.Length];
    }

    public static ConsoleColor PulseColor(int frame, int phase, bool active)
    {
        if (!active)
            return ConsoleColor.DarkGray;

        return ((frame + phase) & 3) switch
        {
            0 => ConsoleColor.Gray,
            1 => ConsoleColor.White,
            2 => ConsoleColor.DarkCyan,
            _ => ConsoleColor.Green,
        };
    }

    public static string Sparkline(IReadOnlyList<long> samples, int width, TerminalGlyphSet glyphs)
    {
        if (samples == null || samples.Count == 0 || width <= 0)
            return "";

        int count = Math.Min(width, samples.Count);
        long max = 0;
        for (int i = samples.Count - count; i < samples.Count; i++)
            if (samples[i] > max) max = samples[i];

        if (max <= 0)
            return "";

        string[] ticks = glyphs.Sparkline;
        var chars = new string[count];
        for (int i = 0; i < count; i++)
        {
            long value = samples[samples.Count - count + i];
            int index = (int)Math.Clamp((value * (ticks.Length - 1)) / max, 0, ticks.Length - 1);
            chars[i] = ticks[index];
        }

        return string.Concat(chars);
    }
}
