using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed class RelayBypass
{
    private readonly RelayPortManager _ports;
    private readonly string _scheme;
    private int _active;

    public RelayBypass(RelayPortManager ports, string scheme)
    {
        _ports = ports;
        _scheme = scheme;
    }

    public bool Active => Volatile.Read(ref _active) == 1;

    public bool Engage(int wedgedPort)
    {
        if (wedgedPort != _ports.CurrentPort) return Active;
        if (Interlocked.Exchange(ref _active, 1) == 0)
        {
            _ports.DeletePortFile();
            ConsoleUx.Warn(LogComponent.Relay, "VRChat can no longer reach the local video relay on port " + wedgedPort
                + "; sending direct links until VRChat restarts");
        }
        return true;
    }

    public void Release()
    {
        if (Interlocked.Exchange(ref _active, 0) == 0) return;
        _ports.Republish(_scheme);
        ConsoleUx.Write(LogComponent.Relay, "new VRChat session; local video relay back on port " + _ports.CurrentPort);
    }
}
