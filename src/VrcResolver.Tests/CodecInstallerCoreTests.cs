using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.Versioning;
using System.Threading.Tasks;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class CodecInstallerCoreTests
{
    private static readonly CodecInstaller.Codec Hevc = new(
        "HEVC Video Extensions", "Microsoft.HEVCVideoExtension", "h265",
        new[] { "paid-id", "free-id" });

    private static CodecInstaller.Core MakeCore(
        Func<string, bool>? mfProbe = null,
        Func<string, Task<bool>>? appxProbe = null,
        Func<string, Task<CodecInstaller.WingetResult>>? wingetInstall = null,
        Func<DateTime>? utcNow = null)
        => new(
            mfProbe ?? (_ => false),
            appxProbe ?? (_ => Task.FromResult(false)),
            wingetInstall ?? (_ => Task.FromResult(new CodecInstaller.WingetResult(true, false, 1, ""))),
            utcNow ?? (() => new DateTime(2026, 8, 21, 12, 0, 0, DateTimeKind.Utc)));

    private static CodecInstaller.WingetResult Ok() => new(true, false, 0, "");

    [Fact]
    public async Task MfDecoderPresent_ShortCircuits_NoSpawns()
    {
        int appxCalls = 0, wingetCalls = 0;
        var core = MakeCore(
            mfProbe: _ => true,
            appxProbe: _ => { appxCalls++; return Task.FromResult(false); },
            wingetInstall: _ => { wingetCalls++; return Task.FromResult(Ok()); });
        var state = new CodecInstaller.CodecState();

        CodecInstaller.CodecOutcome? seen = null;
        bool dirty = await core.RunAsync(new[] { Hevc }, state, o => seen = o);

        Assert.True(dirty);
        Assert.True(seen!.Installed);
        Assert.Equal("mf", seen.Detail);
        Assert.Equal(0, appxCalls);
        Assert.Equal(0, wingetCalls);
        Assert.Equal("installed", state.Codecs["paid-id"].Status);
    }

    [Fact]
    public async Task AppxPresent_SkipsWinget()
    {
        int wingetCalls = 0;
        var core = MakeCore(
            appxProbe: _ => Task.FromResult(true),
            wingetInstall: _ => { wingetCalls++; return Task.FromResult(Ok()); });
        var state = new CodecInstaller.CodecState();

        CodecInstaller.CodecOutcome? seen = null;
        await core.RunAsync(new[] { Hevc }, state, o => seen = o);

        Assert.True(seen!.Installed);
        Assert.Equal("appx", seen.Detail);
        Assert.Equal(0, wingetCalls);
    }

    [Fact]
    public async Task WingetSuccess_OnlyCountsWhenVerified()
    {
        var core = MakeCore(wingetInstall: _ => Task.FromResult(Ok()));
        var state = new CodecInstaller.CodecState();

        CodecInstaller.CodecOutcome? seen = null;
        await core.RunAsync(new[] { Hevc }, state, o => seen = o);

        Assert.False(seen!.Installed);
        Assert.Equal(CodecInstaller.FailUnverifiedInstall, seen.Detail);
        Assert.Equal("failed", state.Codecs["paid-id"].Status);
        Assert.Equal(CodecInstaller.FailUnverifiedInstall, state.Codecs["paid-id"].FailureClass);
    }

    [Fact]
    public async Task WingetSuccess_VerifiedByAppxProbe_IsInstalled()
    {
        bool installed = false;
        var core = MakeCore(
            appxProbe: _ => Task.FromResult(installed),
            wingetInstall: _ => { installed = true; return Task.FromResult(Ok()); });
        var state = new CodecInstaller.CodecState();

        CodecInstaller.CodecOutcome? seen = null;
        await core.RunAsync(new[] { Hevc }, state, o => seen = o);

        Assert.True(seen!.Installed);
        Assert.Equal("winget+appx", seen.Detail);
    }

    [Fact]
    public async Task SecondStoreId_TriedOnlyAfterFirstFails()
    {
        var tried = new List<string>();
        bool installed = false;
        var core = MakeCore(
            appxProbe: _ => Task.FromResult(installed),
            wingetInstall: id =>
            {
                tried.Add(id);
                if (id == "free-id") { installed = true; return Task.FromResult(Ok()); }
                return Task.FromResult(new CodecInstaller.WingetResult(true, false, 1,
                    "This app requires purchase before install."));
            });
        var state = new CodecInstaller.CodecState();

        CodecInstaller.CodecOutcome? seen = null;
        await core.RunAsync(new[] { Hevc }, state, o => seen = o);

        Assert.Equal(new[] { "paid-id", "free-id" }, tried);
        Assert.True(seen!.Installed);
        Assert.Equal("winget+appx", seen.Detail);
    }

    [Fact]
    public async Task WingetMissing_StopsAfterFirstId()
    {
        var tried = new List<string>();
        var core = MakeCore(wingetInstall: id =>
        {
            tried.Add(id);
            return Task.FromResult(new CodecInstaller.WingetResult(false, false, -1, ""));
        });
        var state = new CodecInstaller.CodecState();

        CodecInstaller.CodecOutcome? seen = null;
        await core.RunAsync(new[] { Hevc }, state, o => seen = o);

        Assert.Single(tried);
        Assert.Equal(CodecInstaller.FailWingetMissing, seen!.Detail);
    }

    [Theory]
    [InlineData("No package found matching input criteria.", CodecInstaller.FailNotFound)]
    [InlineData("This app requires PURCHASE.", CodecInstaller.FailNotEntitled)]
    [InlineData("The msstore source is disabled by policy.", CodecInstaller.FailSourceUnavailable)]
    [InlineData("something new and strange", CodecInstaller.FailUnknown)]
    public void ClassifyWingetFailure_MapsOutputTokens(string output, string expected)
    {
        var result = new CodecInstaller.WingetResult(true, false, 1, output);
        Assert.Equal(expected, CodecInstaller.ClassifyWingetFailure(result));
    }

    [Fact]
    public void ClassifyWingetFailure_TimeoutAndMissing()
    {
        Assert.Equal(CodecInstaller.FailTimeout,
            CodecInstaller.ClassifyWingetFailure(new CodecInstaller.WingetResult(true, true, -1, "")));
        Assert.Equal(CodecInstaller.FailWingetMissing,
            CodecInstaller.ClassifyWingetFailure(new CodecInstaller.WingetResult(false, false, -1, "")));
    }

    [Fact]
    public async Task Backoff_TransientRetriesAfterADay_PermanentAfterAWeek()
    {
        var now = new DateTime(2026, 8, 21, 12, 0, 0, DateTimeKind.Utc);
        var core = MakeCore(utcNow: () => now);

        var transient = new CodecInstaller.CodecState();
        transient.Codecs["paid-id"] = new CodecInstaller.CodecEntry
        {
            Status = "failed",
            FailureClass = CodecInstaller.FailTimeout,
            LastAttemptUtc = now - TimeSpan.FromHours(12),
        };
        Assert.True(core.ShouldSkip(transient, "paid-id"));
        transient.Codecs["paid-id"].LastAttemptUtc = now - TimeSpan.FromHours(25);
        Assert.False(core.ShouldSkip(transient, "paid-id"));

        var permanent = new CodecInstaller.CodecState();
        permanent.Codecs["paid-id"] = new CodecInstaller.CodecEntry
        {
            Status = "failed",
            FailureClass = CodecInstaller.FailNotEntitled,
            LastAttemptUtc = now - TimeSpan.FromDays(3),
        };
        Assert.True(core.ShouldSkip(permanent, "paid-id"));
        permanent.Codecs["paid-id"].LastAttemptUtc = now - TimeSpan.FromDays(8);
        Assert.False(core.ShouldSkip(permanent, "paid-id"));
    }

    [Fact]
    public async Task InstalledState_IsNeverRetried()
    {
        int wingetCalls = 0;
        var core = MakeCore(wingetInstall: _ => { wingetCalls++; return Task.FromResult(Ok()); });
        var state = new CodecInstaller.CodecState();
        state.Codecs["paid-id"] = new CodecInstaller.CodecEntry { Status = "installed" };

        bool dirty = await core.RunAsync(new[] { Hevc }, state);

        Assert.False(dirty);
        Assert.Equal(0, wingetCalls);
    }

    [Fact]
    public void LegacyStateFile_WithoutFailureClass_StillParses()
    {
        string path = Path.Combine(Path.GetTempPath(), "codec-state-" + Guid.NewGuid().ToString("N") + ".json");
        File.WriteAllText(path,
            "{\"codecs\":{\"9NMZLZ57R3T7\":{\"status\":\"failed\",\"last_attempt_utc\":\"2026-08-20T00:00:00Z\",\"package_family_name\":\"Microsoft.HEVCVideoExtension\"}}}");
        try
        {
            var state = CodecInstaller.LoadState(path);
            var entry = state.Codecs["9NMZLZ57R3T7"];
            Assert.Equal("failed", entry.Status);
            Assert.Null(entry.FailureClass);
            Assert.Equal("Microsoft.HEVCVideoExtension", entry.PackageName);
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public void RequiredList_PinsTheHevcFallbackOrder()
    {
        var hevc = Array.Find(CodecInstaller.Required, c => c.ProbeCodec == "h265");
        Assert.NotNull(hevc);
        Assert.Equal(new[] { "9NMZLZ57R3T7", "9N4WGH0Z6VHQ" }, hevc!.StoreIds);
    }
}
