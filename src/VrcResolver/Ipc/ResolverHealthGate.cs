namespace VrcResolver;

// Circuit breaker for the mesh resolve path. OgFallbackHint reacts per source
// URL; this gate reacts to the pipeline as a whole. Two independent streaks
// feed it because the failure modes differ:
//   resolve streak  -- consecutive resolves with no real server verdict
//                      (mesh down, IPC budget tripped, resolve threw). A dead
//                      socket otherwise costs every video ~10 s of connect
//                      retries and delays before og runs.
//   playback streak -- consecutive AVPro failures on URLs we served, reset
//                      only by an observed successful playback. Catches the
//                      "server resolves fine but nothing actually plays"
//                      mode that resolve verdicts alone can never see.
//
// While open, LocalIpcServer answers fallback_native/resolver_unhealthy
// before touching cache or mesh; the wrapper treats the reason as
// non-retryable and execs og after one pipe roundtrip. After the cooldown,
// one request is let through as a probe -- but only when the WebSocket is
// actually open, so a dead mesh never wastes probes. A probe that resolves
// waits for playback confirmation (or a verdict timeout) before the gate
// closes: resolving is not proof, playing is.
//
// In-memory only. A restart re-dials the mesh within seconds and gets fresh
// ground truth; persisting an open state could wedge og-mode after the
// cause is long gone.
internal sealed class ResolverHealthGate
{
    internal enum Transition { None, Opened, Closed }
    private enum State { Closed, Open, HalfOpenProbe, HalfOpenVerdict }

    public const int OpenThreshold = 3;
    // Matches the mesh reconnect backoff ceiling: by the time a cooldown
    // lapses, a recovering socket has had at least one reconnect attempt.
    public static readonly TimeSpan DefaultCooldown = TimeSpan.FromSeconds(30);
    // Covers Opening + the 12 s silent_stall window with margin. If no
    // playback observation arrives at all (user never pressed Play again),
    // the verdict lapses into Closed rather than holding og-mode forever.
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

    // Test seam: deterministic cooldown/verdict windows + clock injection.
    internal ResolverHealthGate(TimeSpan cooldown, TimeSpan verdictTimeout, Func<DateTime> nowUtc)
    {
        _cooldown = cooldown;
        _verdictTimeout = verdictTimeout;
        _now = nowUtc;
    }

    // Called once per wrapper resolve. True = answer resolver_unhealthy
    // without touching cache or mesh. The single call that flips Open ->
    // HalfOpenProbe returns false and becomes the probe; concurrent calls
    // keep short-circuiting until the probe's verdict lands.
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

    // healthy = a real server verdict arrived (resolved, or fallback_native
    // with a server-decided reason). Synthesized server_unreachable /
    // internal_error outcomes are unhealthy. resolved = action was
    // `resolved` (drives the probe's wait-for-playback step).
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
                    // Healthy structural fallback: the pipeline works, og
                    // plays this particular video. Good enough to close.
                    CloseLocked();
                    return Transition.Closed;
                default:
                    // Stale in-flight result finishing after the gate
                    // opened; the probe cycle owns recovery.
                    return Transition.None;
            }
        }
    }

    // From VrcLogMonitor, attributed to URLs we served only.
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
                    // The probe's video (or a straggler) failed -- back to
                    // og-mode for another cooldown. Not a new user-visible
                    // pause; the gate never closed.
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
