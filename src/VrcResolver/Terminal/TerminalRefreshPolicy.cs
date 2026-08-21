namespace VrcResolver;

internal static class TerminalRefreshPolicy
{
    public static readonly TimeSpan ActiveRefreshInterval = TimeSpan.FromMilliseconds(125);
    public static readonly TimeSpan IdleRefreshInterval = TimeSpan.FromSeconds(1);
    public static readonly TimeSpan AnimationWindow = TimeSpan.FromSeconds(2);
    public static readonly TimeSpan RecentActivityWindow = TimeSpan.FromSeconds(5);

    public static bool ShouldUseFastRefresh(
        WatchdogActivitySnapshot snapshot,
        DateTime nowUtc,
        AppSettings settings,
        bool animationsAvailable)
    {
        if (settings == null)
            return false;
        if (!settings.Terminal.StatusLine || !settings.Terminal.Animations || !animationsAvailable)
            return false;

        return snapshot.RelayActive(nowUtc, AnimationWindow)
            || snapshot.UpstreamActive(nowUtc, AnimationWindow);
    }
}
