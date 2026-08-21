namespace VrcResolver;

internal readonly record struct TerminalCommandLine(string Verb, string Arguments)
{
    public static TerminalCommandLine Parse(string text)
    {
        text = (text ?? "").Trim();
        if (text.Length == 0) return new TerminalCommandLine("", "");

        int split = text.IndexOfAny(new[] { ' ', '\t' });
        string verb = split < 0 ? text : text.Substring(0, split);
        string args = split < 0 ? "" : text.Substring(split + 1).Trim();
        return new TerminalCommandLine(NormalizeVerb(verb), args);
    }

    public static string NormalizeVerb(string verb)
    {
        verb = (verb ?? "").Trim();
        while (verb.StartsWith("/", StringComparison.Ordinal))
            verb = verb.Substring(1);
        return verb.ToLowerInvariant();
    }
}
