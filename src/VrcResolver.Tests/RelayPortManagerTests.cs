using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Runtime.Versioning;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class RelayPortManagerTests
{
    [Fact]
    public void Initialize_ReplacesBusyPreviousPort()
    {
        string stateRoot = CreateTempStateRoot();
        try
        {
            using var busy = new TcpListener(IPAddress.Loopback, 0);
            busy.Start();
            int busyPort = ((IPEndPoint)busy.LocalEndpoint).Port;

            File.WriteAllText(
                Path.Combine(stateRoot, "relay_last_port.txt"),
                busyPort.ToString(CultureInfo.InvariantCulture));

            var manager = new RelayPortManager(stateRoot);

            Assert.True(manager.Initialize());
            Assert.NotEqual(busyPort, manager.CurrentPort);
            Assert.Equal(
                manager.CurrentPort.ToString(CultureInfo.InvariantCulture),
                File.ReadAllText(Path.Combine(stateRoot, "relay_port.txt")).Trim());
            Assert.Equal(
                manager.CurrentPort.ToString(CultureInfo.InvariantCulture),
                File.ReadAllText(Path.Combine(stateRoot, "relay_last_port.txt")).Trim());
        }
        finally
        {
            DeleteTempStateRoot(stateRoot);
        }
    }

    [Fact]
    public void TryReserveFreshPort_RewritesRelayFiles()
    {
        string stateRoot = CreateTempStateRoot();
        try
        {
            var manager = new RelayPortManager(stateRoot);
            Assert.True(manager.Initialize());
            int firstPort = manager.CurrentPort;
            using var busy = new TcpListener(IPAddress.Loopback, firstPort);
            busy.Start();

            Assert.True(manager.TryReserveFreshPort("test bind failure"));

            Assert.NotEqual(0, manager.CurrentPort);
            Assert.NotEqual(firstPort, manager.CurrentPort);
            Assert.Equal(
                manager.CurrentPort.ToString(CultureInfo.InvariantCulture),
                File.ReadAllText(Path.Combine(stateRoot, "relay_port.txt")).Trim());
            Assert.Equal(
                manager.CurrentPort.ToString(CultureInfo.InvariantCulture),
                File.ReadAllText(Path.Combine(stateRoot, "relay_last_port.txt")).Trim());
        }
        finally
        {
            DeleteTempStateRoot(stateRoot);
        }
    }

    private static string CreateTempStateRoot()
    {
        string path = Path.Combine(Path.GetTempPath(), "VrcResolver.Tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }

    private static void DeleteTempStateRoot(string path)
    {
        try { Directory.Delete(path, recursive: true); }
        catch { }
    }
}
