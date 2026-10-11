using System.Diagnostics;
using System.Runtime.Versioning;
using System.Text.RegularExpressions;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class ParentProcessTests
{
    [Fact]
    public void Describe_names_the_process_that_started_the_test_host()
    {
        string? parent = ParentProcess.Describe();

        Assert.NotNull(parent);
        Match match = Regex.Match(parent!, @"^(?<name>\S+) pid=(?<pid>\d+)$");
        Assert.True(match.Success, parent);
        int pid = int.Parse(match.Groups["pid"].Value);
        Assert.NotEqual(Environment.ProcessId, pid);

        using Process self = Process.GetCurrentProcess();
        using Process found = Process.GetProcessById(pid);
        Assert.Equal(match.Groups["name"].Value, found.ProcessName);
        Assert.True(found.StartTime <= self.StartTime);
    }
}
