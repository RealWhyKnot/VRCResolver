namespace VrcResolver;

internal sealed class TerminalFrameGate
{
    private bool _hasLast;
    private WatchdogActivitySnapshot _snapshot;
    private long _current;
    private long[] _history = Array.Empty<long>();
    private bool _mesh;
    private bool _relayAnimating;
    private bool _upstreamAnimating;
    private bool _relayRecent;
    private bool _upstreamRecent;
    private int _spinner;
    private int _width;
    private string _input = "";
    private string _ghost = "";
    private int _cursor;
    private bool _statusLine;
    private bool _animations;
    private bool _unicode;
    private TerminalFrame? _frame;

    public int LastWidth => _width;

    public bool TryGetCached(
        WatchdogActivitySnapshot snapshot,
        WatchdogBandwidthSnapshot bandwidth,
        DateTime nowUtc,
        bool meshConnected,
        int spinnerIndex,
        int width,
        string input,
        bool statusLine,
        bool animations,
        bool unicode,
        string ghost,
        int cursor,
        out TerminalFrame? frame)
    {
        frame = null;
        bool relayAnimating = snapshot.RelayActive(nowUtc, TerminalRefreshPolicy.AnimationWindow);
        bool upstreamAnimating = snapshot.UpstreamActive(nowUtc, TerminalRefreshPolicy.AnimationWindow);
        bool relayRecent = snapshot.RelayActive(nowUtc, TerminalRefreshPolicy.RecentActivityWindow);
        bool upstreamRecent = snapshot.UpstreamActive(nowUtc, TerminalRefreshPolicy.RecentActivityWindow);
        int spinner = animations && (relayAnimating || upstreamAnimating) ? spinnerIndex : 0;

        bool same = _hasLast
            && _frame != null
            && _snapshot == snapshot
            && _current == bandwidth.CurrentBytesPerSecond
            && HistoryEquals(bandwidth.HistoryBytesPerSecond)
            && _mesh == meshConnected
            && _relayAnimating == relayAnimating
            && _upstreamAnimating == upstreamAnimating
            && _relayRecent == relayRecent
            && _upstreamRecent == upstreamRecent
            && _spinner == spinner
            && _width == width
            && _cursor == cursor
            && _statusLine == statusLine
            && _animations == animations
            && _unicode == unicode
            && string.Equals(_input, input, StringComparison.Ordinal)
            && string.Equals(_ghost, ghost, StringComparison.Ordinal);

        if (same)
        {
            frame = _frame;
            return true;
        }

        _snapshot = snapshot;
        _current = bandwidth.CurrentBytesPerSecond;
        CopyHistory(bandwidth.HistoryBytesPerSecond);
        _mesh = meshConnected;
        _relayAnimating = relayAnimating;
        _upstreamAnimating = upstreamAnimating;
        _relayRecent = relayRecent;
        _upstreamRecent = upstreamRecent;
        _spinner = spinner;
        _width = width;
        _input = input ?? "";
        _ghost = ghost ?? "";
        _cursor = cursor;
        _statusLine = statusLine;
        _animations = animations;
        _unicode = unicode;
        _frame = null;
        _hasLast = true;
        return false;
    }

    public void Store(TerminalFrame frame)
    {
        _frame = frame;
    }

    private bool HistoryEquals(IReadOnlyList<long>? history)
    {
        int count = history?.Count ?? 0;
        if (count != _history.Length) return false;
        for (int i = 0; i < count; i++)
            if (_history[i] != history![i]) return false;
        return true;
    }

    private void CopyHistory(IReadOnlyList<long>? history)
    {
        int count = history?.Count ?? 0;
        if (_history.Length != count) _history = new long[count];
        for (int i = 0; i < count; i++)
            _history[i] = history![i];
    }
}
