namespace VrcResolver;

internal static class TerminalBlocks
{
    public static IReadOnlyList<TerminalFrame> KeyValue(
        string title,
        IEnumerable<(string Left, string Right)> rows,
        int width,
        TerminalGlyphSet glyphs)
    {
        var materialized = rows.ToArray();
        width = Math.Max(24, width);
        int leftWidth = materialized.Length == 0
            ? 0
            : Math.Min(Math.Min(24, materialized.Max(r => r.Left.Length)), Math.Max(8, width / 3));
        int rightWidth = Math.Max(8, width - leftWidth - 7);

        var frames = new List<TerminalFrame>(materialized.Length + 1)
        {
            TerminalFrame.FromRuns(
                new TerminalTextRun(glyphs.Bullet + " ", ConsoleColor.DarkGray),
                new TerminalTextRun(title, ConsoleColor.White)),
        };

        foreach (var (left, right) in materialized)
        {
            var wrapped = Wrap(right, rightWidth);
            int lineCount = Math.Max(1, wrapped.Count);
            for (int i = 0; i < lineCount; i++)
            {
                string leftText = i == 0 ? TrimRight(left, leftWidth).PadRight(leftWidth) : new string(' ', leftWidth);
                string rightText = wrapped.Count == 0 ? "" : wrapped[i];
                frames.Add(TerminalFrame.FromRuns(
                    new TerminalTextRun("  " + glyphs.Detail + " ", ConsoleColor.DarkGray),
                    new TerminalTextRun(leftText, i == 0 ? ConsoleColor.White : ConsoleColor.DarkGray),
                    new TerminalTextRun("  ", ConsoleColor.DarkGray),
                    new TerminalTextRun(rightText, ConsoleColor.Gray)));
            }
        }

        return frames;
    }

    public static IReadOnlyList<TerminalFrame> Table(
        string title,
        IEnumerable<(string Name, string Value, string Description)> rows,
        int width,
        TerminalGlyphSet glyphs)
    {
        var materialized = rows.ToArray();
        if (materialized.Length == 0)
            return KeyValue(title, Array.Empty<(string Left, string Right)>(), width, glyphs);

        width = Math.Max(24, width);
        int nameWidth = Math.Min(Math.Min(22, materialized.Max(r => r.Name.Length)), Math.Max(8, width / 4));
        int valueWidth = Math.Min(Math.Min(18, materialized.Max(r => r.Value.Length)), Math.Max(8, width / 5));
        int descriptionWidth = width - nameWidth - valueWidth - 10;
        if (descriptionWidth < 16)
        {
            return KeyValue(
                title,
                materialized.Select(static r => (r.Name, r.Value + " - " + r.Description)),
                width,
                glyphs);
        }

        var frames = new List<TerminalFrame>(materialized.Length + 2)
        {
            TerminalFrame.FromRuns(
                new TerminalTextRun(glyphs.Bullet + " ", ConsoleColor.DarkGray),
                new TerminalTextRun(title, ConsoleColor.White)),
            TerminalFrame.FromRuns(
                new TerminalTextRun("  " + "setting".PadRight(nameWidth), ConsoleColor.DarkGray),
                new TerminalTextRun("  " + "current".PadRight(valueWidth), ConsoleColor.DarkGray),
                new TerminalTextRun("  about", ConsoleColor.DarkGray)),
        };

        foreach (var row in materialized)
        {
            var descriptions = Wrap(row.Description, descriptionWidth);
            int lineCount = Math.Max(1, descriptions.Count);
            for (int i = 0; i < lineCount; i++)
            {
                string name = i == 0 ? TrimRight(row.Name, nameWidth).PadRight(nameWidth) : new string(' ', nameWidth);
                string value = i == 0 ? TrimRight(row.Value, valueWidth).PadRight(valueWidth) : new string(' ', valueWidth);
                string description = descriptions.Count == 0 ? "" : descriptions[i];

                frames.Add(TerminalFrame.FromRuns(
                    new TerminalTextRun("  " + name, i == 0 ? ConsoleColor.White : ConsoleColor.DarkGray),
                    new TerminalTextRun("  ", ConsoleColor.DarkGray),
                    new TerminalTextRun(value, ValueColor(row.Value)),
                    new TerminalTextRun("  ", ConsoleColor.DarkGray),
                    new TerminalTextRun(description, ConsoleColor.Gray)));
            }
        }

        return frames;
    }

    public static IReadOnlyList<TerminalFrame> Panel(
        string title,
        IEnumerable<(string Name, string State, string Detail)> rows,
        int width,
        TerminalGlyphSet glyphs)
    {
        var materialized = rows.ToArray();
        width = Math.Clamp(width, 30, 120);
        if (materialized.Length == 0)
            return KeyValue(title, Array.Empty<(string Left, string Right)>(), width, glyphs);

        if (width < 48)
        {
            return KeyValue(
                title,
                materialized.Select(static r => (r.Name, r.State + "  " + r.Detail)),
                width,
                glyphs);
        }

        int innerWidth = width - 2;
        int nameWidth = Math.Min(16, Math.Max(8, materialized.Max(r => r.Name.Length)));
        int stateWidth = Math.Min(18, Math.Max(8, materialized.Max(r => r.State.Length)));
        int detailWidth = Math.Max(8, innerWidth - nameWidth - stateWidth - 6);
        string heading = " " + title + " ";
        string topFill = Repeat(glyphs.Horizontal, Math.Max(0, innerWidth - heading.Length));

        var frames = new List<TerminalFrame>(materialized.Length + 2)
        {
            TerminalFrame.FromRuns(
                new TerminalTextRun(glyphs.TopLeft, ConsoleColor.DarkGray),
                new TerminalTextRun(heading, ConsoleColor.White),
                new TerminalTextRun(topFill, ConsoleColor.DarkGray),
                new TerminalTextRun(glyphs.TopRight, ConsoleColor.DarkGray)),
        };

        foreach (var row in materialized)
        {
            string name = TrimRight(row.Name, nameWidth).PadRight(nameWidth);
            string state = TrimRight(row.State, stateWidth).PadRight(stateWidth);
            string detail = TrimRight(row.Detail, detailWidth).PadRight(detailWidth);
            frames.Add(TerminalFrame.FromRuns(
                new TerminalTextRun(glyphs.Vertical + " ", ConsoleColor.DarkGray),
                new TerminalTextRun(name, ConsoleColor.White),
                new TerminalTextRun("  ", ConsoleColor.DarkGray),
                new TerminalTextRun(state, StateColor(row.State)),
                new TerminalTextRun("  ", ConsoleColor.DarkGray),
                new TerminalTextRun(detail, ConsoleColor.Gray),
                new TerminalTextRun(" " + glyphs.Vertical, ConsoleColor.DarkGray)));
        }

        frames.Add(TerminalFrame.FromRuns(
            new TerminalTextRun(glyphs.BottomLeft, ConsoleColor.DarkGray),
            new TerminalTextRun(Repeat(glyphs.Horizontal, innerWidth), ConsoleColor.DarkGray),
            new TerminalTextRun(glyphs.BottomRight, ConsoleColor.DarkGray)));

        return frames;
    }

    public static IReadOnlyList<TerminalFrame> CommandPalette(
        IReadOnlyList<TerminalCommand> commands,
        int width,
        TerminalGlyphSet glyphs)
    {
        width = Math.Max(24, width);
        var frames = new List<TerminalFrame>(commands.Count + 1)
        {
            TerminalFrame.FromRuns(
                new TerminalTextRun(glyphs.Bullet + " ", ConsoleColor.DarkGray),
                new TerminalTextRun("commands", ConsoleColor.White),
                new TerminalTextRun("  tab completes a unique match", ConsoleColor.DarkGray)),
        };

        int commandWidth = Math.Min(18, Math.Max(8, commands.Count == 0 ? 8 : commands.Max(c => c.Name.Length + 1)));
        int descriptionWidth = Math.Max(8, width - commandWidth - 8);
        foreach (var command in commands)
        {
            string alias = command.Aliases.Count == 0 ? "" : "  " + string.Join(", ", command.Aliases.Select(static a => "/" + a));
            string description = TrimRight(command.Description + alias, descriptionWidth);
            frames.Add(TerminalFrame.FromRuns(
                new TerminalTextRun("  " + glyphs.Detail + " ", ConsoleColor.DarkGray),
                new TerminalTextRun(("/" + command.Name).PadRight(commandWidth), ConsoleColor.White),
                new TerminalTextRun("  ", ConsoleColor.DarkGray),
                new TerminalTextRun(description, ConsoleColor.Gray)));
        }

        return frames;
    }

    public static IReadOnlyList<TerminalFrame> CompletionPalette(
        IReadOnlyList<TerminalCompletionItem> items,
        int width,
        TerminalGlyphSet glyphs)
    {
        width = Math.Max(24, width);
        var frames = new List<TerminalFrame>(items.Count + 1)
        {
            TerminalFrame.FromRuns(
                new TerminalTextRun(glyphs.Bullet + " ", ConsoleColor.DarkGray),
                new TerminalTextRun("matches", ConsoleColor.White)),
        };

        int itemWidth = Math.Min(24, Math.Max(8, items.Count == 0 ? 8 : items.Max(c => c.Text.Length + 1)));
        int descriptionWidth = Math.Max(8, width - itemWidth - 8);
        foreach (var item in items)
        {
            frames.Add(TerminalFrame.FromRuns(
                new TerminalTextRun("  " + glyphs.Detail + " ", ConsoleColor.DarkGray),
                new TerminalTextRun(item.Text.PadRight(itemWidth), ConsoleColor.White),
                new TerminalTextRun("  ", ConsoleColor.DarkGray),
                new TerminalTextRun(TrimRight(item.Description, descriptionWidth), ConsoleColor.Gray)));
        }

        return frames;
    }

    public static string TrimRight(string value, int max)
    {
        value ??= "";
        if (max <= 0) return "";
        if (value.Length <= max) return value;
        if (max <= 2) return value[..max];
        return value[..(max - 2)] + "..";
    }

    public static IReadOnlyList<string> Wrap(string value, int width)
    {
        value = (value ?? "").Trim();
        if (value.Length == 0) return Array.Empty<string>();
        width = Math.Max(8, width);

        var lines = new List<string>();
        foreach (string paragraph in value.Split('\n'))
        {
            string remaining = paragraph.Trim();
            while (remaining.Length > width)
            {
                int split = remaining.LastIndexOf(' ', width);
                if (split <= 0) split = width;
                lines.Add(remaining[..split].TrimEnd());
                remaining = remaining[split..].TrimStart();
            }
            if (remaining.Length > 0)
                lines.Add(remaining);
        }

        return lines;
    }

    private static ConsoleColor ValueColor(string value)
    {
        value = (value ?? "").Trim();
        if (value.StartsWith("on", StringComparison.OrdinalIgnoreCase)) return ConsoleColor.Green;
        if (value.StartsWith("off", StringComparison.OrdinalIgnoreCase)) return ConsoleColor.DarkGray;
        if (value.StartsWith("automatic", StringComparison.OrdinalIgnoreCase)) return ConsoleColor.Gray;
        return ConsoleColor.White;
    }

    private static ConsoleColor StateColor(string value)
    {
        value = (value ?? "").Trim().ToLowerInvariant();
        if (value == "on"
            || value == "ready"
            || value == "eligible"
            || value.Contains("online", StringComparison.Ordinal)
            || value.Contains("serving", StringComparison.Ordinal)
            || value.Contains("pulling", StringComparison.Ordinal))
            return ConsoleColor.Green;
        if (value.Contains("reconnecting", StringComparison.Ordinal)
            || value.Contains("fallback", StringComparison.Ordinal)
            || value.Contains("paused", StringComparison.Ordinal)
            || value.Contains("setup", StringComparison.Ordinal)
            || value.Contains("timeout", StringComparison.Ordinal))
            return ConsoleColor.Yellow;
        if (value.Contains("failed", StringComparison.Ordinal)
            || value.Contains("missing", StringComparison.Ordinal)
            || value.Contains("not eligible", StringComparison.Ordinal)
            || value == "off")
            return ConsoleColor.Red;
        return ConsoleColor.Gray;
    }

    private static string Repeat(string text, int count)
    {
        if (count <= 0) return "";
        if (text.Length == 1) return new string(text[0], count);
        return string.Concat(Enumerable.Repeat(text, count));
    }
}
