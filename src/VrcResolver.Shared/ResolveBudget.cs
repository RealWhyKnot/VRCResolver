namespace VrcResolver.Shared;

public static class ResolveBudget
{
    public static readonly TimeSpan Total = TimeSpan.FromSeconds(28);

    public static readonly TimeSpan OgReserve = TimeSpan.FromSeconds(5);

    public static readonly TimeSpan ReAskReserve = TimeSpan.FromSeconds(5);

    public static readonly TimeSpan OgHardCap = TimeSpan.FromMinutes(5);

    public static TimeSpan OgWindow(TimeSpan elapsed, bool reAskAvailable)
    {
        if (!reAskAvailable) return OgHardCap;
        var remaining = Total - elapsed - ReAskReserve;
        return remaining < OgReserve ? OgReserve : remaining;
    }
}
