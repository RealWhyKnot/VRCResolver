using System;
using System.IO;
using System.Linq;
using System.Net.WebSockets;
using System.Reflection;
using System.Threading.Tasks;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class MeshConnectTimingTests
{
    private static MeshConnectProbe ProbeOf(MeshClient client)
    {
        var field = typeof(MeshClient).GetField("_connectProbe", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        return (MeshConnectProbe)field!.GetValue(client)!;
    }

    [Fact]
    public void Default_connect_deadline_is_eight_seconds()
    {
        var field = typeof(MeshClient).GetField("_connectDeadline", BindingFlags.Instance | BindingFlags.NonPublic)!;
        Assert.Equal(TimeSpan.FromSeconds(8), (TimeSpan)field.GetValue(new MeshClient())!);
    }

    [Theory]
    [InlineData("tcp", 12, 0, true, "connect to vrcresolver.com timed out after 8000 ms during tcp (dns 12 ms)")]
    [InlineData("upgrade", 3, 40, true, "connect to vrcresolver.com timed out after 8000 ms during upgrade (dns 3 ms, tcp 40 ms)")]
    [InlineData("upgrade", 3, 40, false, "connect to vrcresolver.com failed after 8000 ms during upgrade (dns 3 ms, tcp 40 ms): WebSocketException")]
    public void DescribeConnectFailure_names_the_phase(string phase, long dns, long tcp, bool timeout, string expectedPrefix)
    {
        var probe = new MeshConnectProbe();
        probe.RecordDns(dns);
        if (tcp > 0) probe.RecordTcp(tcp);
        probe.Enter(phase);
        Exception ex = timeout ? new TaskCanceledException() : new WebSocketException("x");

        string text = MeshClient.DescribeConnectFailure("vrcresolver.com", 8000, probe, ex);

        Assert.StartsWith(expectedPrefix, text);
    }

    [Fact]
    public void DescribeConnectFailure_omits_timings_that_were_never_recorded()
    {
        var probe = new MeshConnectProbe();
        probe.Enter("dns");

        string text = MeshClient.DescribeConnectFailure("vrcresolver.com", 8000, probe, new TaskCanceledException());

        Assert.Equal("connect to vrcresolver.com timed out after 8000 ms during dns", text);
    }

    [Fact]
    public void DescribeConnectFailure_reports_the_root_cause_of_a_failed_connect()
    {
        var probe = new MeshConnectProbe();
        probe.RecordDns(1);
        probe.RecordTcp(2);
        probe.Enter("upgrade");
        var ex = new WebSocketException("Unable to connect", new InvalidOperationException("server said 503"));

        string text = MeshClient.DescribeConnectFailure("vrcresolver.com", 15, probe, ex);

        Assert.Equal(
            "connect to vrcresolver.com failed after 15 ms during upgrade (dns 1 ms, tcp 2 ms): WebSocketException (InvalidOperationException: server said 503)",
            text);
    }

    [Fact]
    public async Task Successful_connect_records_dns_and_tcp_timings()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server);

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 10_000, "mesh connection");
        var probe = ProbeOf(client);
        Assert.Equal("upgrade", probe.Phase);
        Assert.True(probe.DnsMs >= 0);
        Assert.True(probe.TcpMs >= 0);
    }

    [Fact]
    public async Task Each_connect_attempt_gets_its_own_probe()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Hang : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server, connectDeadline: TimeSpan.FromMilliseconds(500));

        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 1, 10_000, "first connect attempt");
        var first = ProbeOf(client);
        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 10_000, "mesh connection after a hung first connect");

        var second = ProbeOf(client);
        Assert.NotSame(first, second);
        Assert.True(second.DnsMs >= 0);
        Assert.True(second.TcpMs >= 0);
    }

    [Fact]
    public async Task Hung_upgrade_is_reported_with_its_phase()
    {
        var original = Console.Out;
        var captured = new StringWriter();
        Console.SetOut(captured);
        try
        {
            await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Hang : MeshAttempt.Accept);
            await using var client = MeshTestClient.For(server, connectDeadline: TimeSpan.FromMilliseconds(500));

            await client.StartAsync();

            await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 10_000, "mesh connection after a hung first connect");
        }
        finally
        {
            Console.SetOut(original);
        }

        string output = captured.ToString();
        Assert.Contains("timed out after", output);
        Assert.Contains("during upgrade", output);
        Assert.DoesNotContain("/mesh", output);
    }

    [Fact]
    public async Task Only_the_first_failed_connect_reaches_the_console()
    {
        var original = Console.Out;
        var captured = new StringWriter();
        Console.SetOut(captured);
        try
        {
            await using var server = new ScriptedMeshServer(attempt => attempt <= 2 ? MeshAttempt.Refuse : MeshAttempt.Accept);
            await using var client = MeshTestClient.For(server);

            await client.StartAsync();

            await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 15_000, "mesh connection after two refused connects");
        }
        finally
        {
            Console.SetOut(original);
        }

        var failures = captured.ToString()
            .Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Where(l => l.Contains("failed after", StringComparison.Ordinal))
            .ToList();
        Assert.Single(failures);
        Assert.Contains("during upgrade", failures[0]);
    }
}
