namespace VrcResolver;

internal static class TerminalStatusFormatter
{
    internal static readonly TimeSpan ActivityWindow = TerminalRefreshPolicy.AnimationWindow;

    public static string FormatLine(
        WatchdogActivitySnapshot snapshot,
        WatchdogBandwidthSnapshot bandwidth,
        DateTime nowUtc,
        bool meshConnected,
        int spinnerIndex,
        int width,
        string input,
        bool statusLineEnabled = true,
        bool animationsEnabled = true,
        bool unicodeSymbols = true)
    {
        return Format(
            snapshot,
            bandwidth,
            nowUtc,
            meshConnected,
            spinnerIndex,
            width,
            input,
            statusLineEnabled,
            animationsEnabled,
            unicodeSymbols).PlainText;
    }

    public static TerminalFrame Format(
        WatchdogActivitySnapshot snapshot,
        WatchdogBandwidthSnapshot bandwidth,
        DateTime nowUtc,
        bool meshConnected,
        int spinnerIndex,
        int width,
        string input,
        bool statusLineEnabled = true,
        bool animationsEnabled = true,
        bool unicodeSymbols = true)
    {
        width = NormalizeWidth(width);
        input ??= "";

        string prompt = "vrcr> ";
        if (!statusLineEnabled)
            return PromptOnly(prompt, input, width);

        TerminalGlyphSet glyphs = TerminalGlyphSet.For(unicodeSymbols);
        bool relayAnimating = snapshot.RelayActive(nowUtc, TerminalRefreshPolicy.AnimationWindow);
        bool upstreamAnimating = snapshot.UpstreamActive(nowUtc, TerminalRefreshPolicy.AnimationWindow);
        bool relayRecent = snapshot.RelayActive(nowUtc, TerminalRefreshPolicy.RecentActivityWindow);
        bool upstreamRecent = snapshot.UpstreamActive(nowUtc, TerminalRefreshPolicy.RecentActivityWindow);

        var statusRuns = BuildStatusRuns(
            snapshot,
            bandwidth,
            meshConnected,
            spinnerIndex,
            animationsEnabled,
            glyphs,
            relayAnimating,
            upstreamAnimating,
            relayRecent,
            upstreamRecent,
            includeTotals: width >= 100,
            includeSparkline: width >= 116);

        var runs = new List<TerminalTextRun>(statusRuns.Count + 3);
        runs.AddRange(statusRuns);
        runs.Add(new TerminalTextRun("  ", ConsoleColor.DarkGray));
        runs.Add(new TerminalTextRun(prompt, ConsoleColor.White));
        if (input.Length > 0)
            runs.Add(new TerminalTextRun(input, ConsoleColor.Gray));

        var full = new TerminalFrame(runs);
        if (full.PlainText.Length <= width)
            return full;

        statusRuns = BuildStatusRuns(
            snapshot,
            bandwidth,
            meshConnected,
            spinnerIndex,
            animationsEnabled,
            glyphs,
            relayAnimating,
            upstreamAnimating,
            relayRecent,
            upstreamRecent,
            includeTotals: false,
            includeSparkline: false);

        string fitted = Fit(PlainText(statusRuns), prompt, input, width);
        return TerminalFrame.Plain(fitted, ConsoleColor.DarkGray);
    }

    private static List<TerminalTextRun> BuildStatusRuns(
        WatchdogActivitySnapshot snapshot,
        WatchdogBandwidthSnapshot bandwidth,
        bool meshConnected,
        int spinnerIndex,
        bool animationsEnabled,
        TerminalGlyphSet glyphs,
        bool relayAnimating,
        bool upstreamAnimating,
        bool relayRecent,
        bool upstreamRecent,
        bool includeTotals,
        bool includeSparkline)
    {
        var runs = new List<TerminalTextRun>(24);

        runs.Add(new TerminalTextRun("VRChat", ConsoleColor.Gray));
        string relayGlyph = TerminalEffectEngine.ActivityGlyph(glyphs, relayAnimating, spinnerIndex, animationsEnabled);
        if (relayGlyph.Length > 0)
        {
            runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
            runs.Add(new TerminalTextRun(relayGlyph, TerminalEffectEngine.PulseColor(spinnerIndex, 0, animationsEnabled)));
        }
        runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
        if (relayRecent)
        {
            runs.Add(new TerminalTextRun("serving", TerminalEffectEngine.PulseColor(spinnerIndex, 1, relayAnimating && animationsEnabled)));
            runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
            runs.Add(new TerminalTextRun(WatchdogDisplay.FormatBytesPerSecond(bandwidth.CurrentBytesPerSecond), ConsoleColor.White));
            runs.Add(new TerminalTextRun(" now", ConsoleColor.DarkGray));
        }
        else
        {
            runs.Add(new TerminalTextRun("waiting", ConsoleColor.DarkGray));
            runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
            runs.Add(new TerminalTextRun(WatchdogDisplay.FormatBytes(snapshot.RelayBytesTotal), ConsoleColor.Gray));
            runs.Add(new TerminalTextRun(" served", ConsoleColor.DarkGray));
        }

        if (includeTotals && relayRecent)
        {
            runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
            runs.Add(new TerminalTextRun(WatchdogDisplay.FormatBytes(snapshot.RelayBytesTotal) + " served", ConsoleColor.Gray));
        }

        if (includeSparkline && bandwidth.HasTraffic)
        {
            string sparkline = TerminalEffectEngine.Sparkline(bandwidth.HistoryBytesPerSecond, 8, glyphs);
            if (sparkline.Length > 0)
            {
                runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
                runs.Add(new TerminalTextRun(sparkline, ConsoleColor.DarkCyan));
            }
        }

        runs.Add(new TerminalTextRun("  resolver", ConsoleColor.Gray));
        string upstreamGlyph = TerminalEffectEngine.ActivityGlyph(glyphs, upstreamAnimating, spinnerIndex + 1, animationsEnabled);
        if (upstreamGlyph.Length > 0)
        {
            runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
            runs.Add(new TerminalTextRun(upstreamGlyph, TerminalEffectEngine.PulseColor(spinnerIndex, 2, animationsEnabled)));
        }
        runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
        if (upstreamRecent)
        {
            runs.Add(new TerminalTextRun("pulling", TerminalEffectEngine.PulseColor(spinnerIndex, 3, upstreamAnimating && animationsEnabled)));
            if (includeTotals && snapshot.UpstreamRelayBytesTotal > 0)
            {
                runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
                runs.Add(new TerminalTextRun(WatchdogDisplay.FormatBytes(snapshot.UpstreamRelayBytesTotal) + " received", ConsoleColor.Gray));
            }
        }
        else
        {
            runs.Add(new TerminalTextRun("idle", ConsoleColor.DarkGray));
        }

        runs.Add(new TerminalTextRun(" ", ConsoleColor.DarkGray));
        runs.Add(new TerminalTextRun(meshConnected ? "online" : "reconnecting", meshConnected ? ConsoleColor.Green : ConsoleColor.Yellow));

        return runs;
    }

    private static TerminalFrame PromptOnly(string prompt, string input, int width)
    {
        string line = Fit("", prompt, input, width);
        return TerminalFrame.Plain(line, ConsoleColor.Gray);
    }

    private static int NormalizeWidth(int width)
    {
        if (width <= 0) return 119;
        return Math.Clamp(width, 20, 180);
    }

    private static string Fit(string status, string prompt, string input, int width)
    {
        int separatorWidth = string.IsNullOrEmpty(status) ? 0 : 2;
        int inputBudget = Math.Max(0, width - status.Length - separatorWidth - prompt.Length);
        string shownInput = TrimLeft(input, inputBudget);
        string line = string.IsNullOrEmpty(status)
            ? prompt + shownInput
            : status + "  " + prompt + shownInput;
        if (line.Length <= width) return line;

        int statusBudget = Math.Max(0, width - 2 - prompt.Length - shownInput.Length);
        status = TrimRight(status, statusBudget);
        line = status.Length == 0
            ? prompt + shownInput
            : status + "  " + prompt + shownInput;
        if (line.Length <= width) return line;

        return TrimLeft(prompt + input, width);
    }

    private static string PlainText(IReadOnlyList<TerminalTextRun> runs)
    {
        int length = 0;
        for (int i = 0; i < runs.Count; i++)
            length += runs[i].Text.Length;

        return string.Create(length, runs, static (span, source) =>
        {
            int offset = 0;
            for (int i = 0; i < source.Count; i++)
            {
                string text = source[i].Text;
                text.AsSpan().CopyTo(span[offset..]);
                offset += text.Length;
            }
        });
    }

    private static string TrimRight(string value, int max)
    {
        if (max <= 0) return "";
        if (value.Length <= max) return value;
        if (max <= 2) return value.Substring(0, max);
        return value.Substring(0, max - 2) + "..";
    }

    private static string TrimLeft(string value, int max)
    {
        if (max <= 0) return "";
        if (value.Length <= max) return value;
        if (max <= 3) return value.Substring(value.Length - max, max);
        return "..." + value.Substring(value.Length - (max - 3), max - 3);
    }
}
