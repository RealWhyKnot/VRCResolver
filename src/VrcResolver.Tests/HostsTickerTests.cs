using System.Runtime.Versioning;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public sealed class HostsTickerTests
{
    private const string GiveUpMarker = "won't ask again";

    [Fact]
    public void Ticker_does_not_reprompt_after_a_decline()
    {
        var h = new Harness();
        var ticker = h.Create();

        string output = Capture(() =>
        {
            ticker.Tick();
            h.Now += TimeSpan.FromHours(1);
            ticker.Tick();
            h.Now += TimeSpan.FromHours(1);
            ticker.Tick();
        });

        Assert.Equal(1, h.EnsureCalls);
        string line = Assert.Single(Lines(output), l => l.Contains(GiveUpMarker));
        string message = line[line.IndexOf("tick:", StringComparison.Ordinal)..];
        Assert.All(message, c => Assert.True(c < 128));
        Assert.DoesNotContain("next attempt in", output);
    }

    [Fact]
    public void Ticker_waits_while_the_startup_prompt_is_open()
    {
        var h = new Harness { PromptInFlight = true };
        var ticker = h.Create();

        string whileOpen = Capture(() =>
        {
            ticker.Tick();
            h.Now += TimeSpan.FromMinutes(1);
            ticker.Tick();
        });

        Assert.Equal(0, h.EnsureCalls);
        Assert.DoesNotContain("failed", whileOpen);
        Assert.DoesNotContain(GiveUpMarker, whileOpen);

        h.PromptInFlight = false;
        h.OnEnsure = () => { h.Active = true; h.Present = true; };
        string afterClose = Capture(ticker.Tick);

        Assert.Equal(1, h.EnsureCalls);
        Assert.Contains("re-add succeeded", afterClose);
    }

    [Fact]
    public void A_declined_startup_prompt_is_not_followed_by_another_one()
    {
        var h = new Harness { PromptInFlight = true };
        var ticker = h.Create();

        string whileOpen = Capture(ticker.Tick);

        Assert.DoesNotContain(GiveUpMarker, whileOpen);

        h.PromptInFlight = false;
        h.PromptDeclined = true;
        string afterDecline = Capture(() =>
        {
            ticker.Tick();
            h.Now += TimeSpan.FromHours(1);
            ticker.Tick();
            ticker.Tick();
        });

        Assert.Equal(0, h.EnsureCalls);
        string line = Assert.Single(Lines(afterDecline), l => l.Contains(GiveUpMarker));
        string message = line[line.IndexOf("tick:", StringComparison.Ordinal)..];
        Assert.All(message, c => Assert.True(c < 128));
        Assert.DoesNotContain("re-adding", afterDecline);
    }

    [Fact]
    public void A_prompt_taken_over_by_another_caller_is_not_a_failed_attempt()
    {
        var h = new Harness();
        var ticker = h.Create();
        h.OnEnsure = () => h.PromptInFlight = true;

        string output = Capture(ticker.Tick);

        Assert.Equal(1, h.EnsureCalls);
        Assert.DoesNotContain(GiveUpMarker, output);

        h.PromptInFlight = false;
        h.OnEnsure = null;
        string retry = Capture(ticker.Tick);

        Assert.Equal(2, h.EnsureCalls);
        Assert.Contains(GiveUpMarker, retry);
    }

    [Fact]
    public void A_successful_readd_is_not_repeated()
    {
        var h = new Harness();
        var ticker = h.Create();
        h.OnEnsure = () => { h.Active = true; h.Present = true; };

        string output = Capture(() =>
        {
            ticker.Tick();
            h.Now += TimeSpan.FromHours(1);
            ticker.Tick();
        });

        Assert.Equal(1, h.EnsureCalls);
        Assert.Contains("re-add succeeded", output);
        Assert.DoesNotContain(GiveUpMarker, output);
    }

    [Fact]
    public void A_readd_that_throws_counts_as_the_failed_attempt()
    {
        var h = new Harness();
        var ticker = h.Create();
        h.OnEnsure = () => throw new InvalidOperationException("boom");

        string output = Capture(() =>
        {
            ticker.Tick();
            h.Now += TimeSpan.FromHours(1);
            ticker.Tick();
        });

        Assert.Equal(1, h.EnsureCalls);
        Assert.Single(Lines(output), l => l.Contains(GiveUpMarker));
    }

    private static IEnumerable<string> Lines(string output) =>
        output.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

    private static string Capture(Action action)
    {
        var original = Console.Out;
        var captured = new StringWriter();
        Console.SetOut(captured);
        try
        {
            action();
        }
        finally
        {
            Console.SetOut(original);
        }
        return captured.ToString();
    }

    private sealed class Harness
    {
        public bool Present;
        public bool Active;
        public bool PromptInFlight;
        public bool PromptDeclined;
        public DateTime Now = new(2026, 10, 11, 12, 0, 0, DateTimeKind.Utc);
        public int EnsureCalls;
        public Action? OnEnsure;

        public HostsTicker Create() => new(
            () => (true, Present, null),
            () =>
            {
                EnsureCalls++;
                OnEnsure?.Invoke();
            },
            () => Active,
            () => PromptInFlight,
            () => PromptDeclined,
            () => Now);
    }
}
