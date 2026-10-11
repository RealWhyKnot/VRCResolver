using System;
using System.IO;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class MeshPlaybackQualityTests
{
    private static async Task<string> DispatchCapturingConsoleAsync(string json)
    {
        var method = typeof(MeshClient).GetMethod("DispatchFrameAsync",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);

        var original = Console.Out;
        var captured = new StringWriter();
        Console.SetOut(captured);
        try
        {
            await using var client = new MeshClient();
            await (Task)method!.Invoke(client, new object[] { Encoding.UTF8.GetBytes(json), CancellationToken.None })!;
        }
        finally
        {
            Console.SetOut(original);
        }
        return captured.ToString();
    }

    private static string Describe(string json)
    {
        using var doc = JsonDocument.Parse(json);
        return MeshClient.FormatPlaybackQualityLine(doc.RootElement);
    }

    [Fact]
    public async Task Playback_quality_update_is_not_reported_as_unknown()
    {
        string output = await DispatchCapturingConsoleAsync(
            "{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"delivered_height\":1080,\"resolved_height\":1080}");

        Assert.DoesNotContain("unknown action", output);
        Assert.Equal("", output);
    }

    [Theory]
    [InlineData("{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"delivered_height\":720}")]
    [InlineData("{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"resolved_height\":1080}")]
    [InlineData("{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"delivered_height\":\"tall\",\"resolved_height\":null}")]
    [InlineData("{\"action\":\"playback_quality_update\",\"correlation_id\":7,\"delivered_height\":1080.5,\"resolved_height\":99999999999}")]
    [InlineData("{\"action\":\"playback_quality_update\"}")]
    public async Task Playback_quality_update_with_missing_or_odd_fields_stays_quiet(string json)
    {
        string output = await DispatchCapturingConsoleAsync(json);

        Assert.Equal("", output);
    }

    [Fact]
    public void Quality_line_reports_the_correlation_id_and_both_heights()
    {
        string line = Describe(
            "{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"delivered_height\":1080,\"resolved_height\":720}");

        Assert.Equal("[mesh] playback quality cid=c1 delivered=1080p resolved=720p", line);
    }

    [Fact]
    public void Quality_line_leaves_out_a_resolved_height_the_server_omitted()
    {
        string line = Describe(
            "{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"delivered_height\":1080}");

        Assert.Equal("[mesh] playback quality cid=c1 delivered=1080p", line);
    }

    [Fact]
    public void Quality_line_leaves_out_heights_that_are_not_numbers()
    {
        string line = Describe(
            "{\"action\":\"playback_quality_update\",\"correlation_id\":\"c1\",\"delivered_height\":\"tall\",\"resolved_height\":null}");

        Assert.Equal("[mesh] playback quality cid=c1", line);
    }

    [Fact]
    public void Quality_line_keeps_only_what_is_present()
    {
        Assert.Equal("[mesh] playback quality", Describe("{\"action\":\"playback_quality_update\"}"));
        Assert.Equal(
            "[mesh] playback quality delivered=480p",
            Describe("{\"action\":\"playback_quality_update\",\"delivered_height\":480}"));
    }

    [Fact]
    public void Quality_line_cleans_control_characters_out_of_the_correlation_id()
    {
        string line = Describe(
            "{\"action\":\"playback_quality_update\",\"correlation_id\":\"c\\r\\n1\",\"delivered_height\":1080}");

        Assert.Equal("[mesh] playback quality cid=c??1 delivered=1080p", line);
    }
}
