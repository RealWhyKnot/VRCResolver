namespace VrcResolver;

internal sealed class ResolverHealthGate
{
    internal enum Transition { None, Opened, Closed }
    private enum State { Closed, Open, HalfOpenProbe, HalfOpenVerdict }

    public const int OpenThreshold = 3;
    public static readonly TimeSpan DefaultCooldown = TimeSpan.FromSeconds(30);
    public static readonly TimeSpan DefaultVerdictTimeout = TimeSpan.FromSeconds(25);

    private readonly object _lock = new();
    private readonly Func<DateTime> _now;
    private readonly TimeSpan _cooldown;
    private readonly TimeSpan _verdictTimeout;
    private State _state = State.Closed;
    private DateTime _openUntil;
    private DateTime _verdictSince;
    private int _resolveStreak;
    private int _playbackStreak;

    public ResolverHealthGate() : this(DefaultCooldown, DefaultVerdictTimeout, () => DateTime.UtcNow) { }

    internal ResolverHealthGate(TimeSpan cooldown, TimeSpan verdictTimeout, Func<DateTime> nowUtc)
    {
        _cooldown = cooldown;
        _verdictTimeout = verdictTimeout;
        _now = nowUtc;
    }

    public bool ShouldShortCircuit(bool meshConnected, out Transition transition)
    {
        lock (_lock)
        {
            transition = Transition.None;
            switch (_state)
            {
                case State.Open:
                    if (meshConnected && _now() >= _openUntil)
                    {
                        _state = State.HalfOpenProbe;
                        return false;
                    }
                    return true;
                case State.HalfOpenProbe:
                    return true;
                case State.HalfOpenVerdict:
                    if (_now() - _verdictSince >= _verdictTimeout)
                    {
                        CloseLocked();
                        transition = Transition.Closed;
                        return false;
                    }
                    return true;
                default:
                    return false;
            }
        }
    }

    public Transition RecordResolveOutcome(bool healthy, bool resolved)
    {
        lock (_lock)
        {
            switch (_state)
            {
                case State.Closed:
                    if (healthy)
                    {
                        _resolveStreak = 0;
                        return Transition.None;
                    }
                    if (++_resolveStreak >= OpenThreshold)
                    {
                        OpenLocked();
                        return Transition.Opened;
                    }
                    return Transition.None;
                case State.HalfOpenProbe:
                    if (!healthy)
                    {
                        OpenLocked();
                        return Transition.None;
                    }
                    if (resolved)
                    {
                        _state = State.HalfOpenVerdict;
                        _verdictSince = _now();
                        return Transition.None;
                    }
                    CloseLocked();
                    return Transition.Closed;
                default:
                    return Transition.None;
            }
        }
    }

    public Transition RecordPlaybackFailure()
    {
        lock (_lock)
        {
            switch (_state)
            {
                case State.Closed:
                    if (++_playbackStreak >= OpenThreshold)
                    {
                        OpenLocked();
                        return Transition.Opened;
                    }
                    return Transition.None;
                case State.HalfOpenProbe:
                case State.HalfOpenVerdict:
                    OpenLocked();
                    return Transition.None;
                default:
                    return Transition.None;
            }
        }
    }

    public Transition RecordPlaybackConfirmed()
    {
        lock (_lock)
        {
            _playbackStreak = 0;
            if (_state == State.HalfOpenProbe || _state == State.HalfOpenVerdict)
            {
                CloseLocked();
                return Transition.Closed;
            }
            return Transition.None;
        }
    }

    private void OpenLocked()
    {
        _state = State.Open;
        _openUntil = _now() + _cooldown;
        _resolveStreak = 0;
        _playbackStreak = 0;
    }

    private void CloseLocked()
    {
        _state = State.Closed;
        _resolveStreak = 0;
        _playbackStreak = 0;
    }
}
