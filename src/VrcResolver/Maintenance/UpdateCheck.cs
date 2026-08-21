using System.Net.Http;
using System.Reflection;
using System.Text.Json;
using VrcResolver.Shared;

namespace VrcResolver;

// Best-effort, fire-and-forget startup check against the GitHub
// releases-latest endpoint. If a newer version exists, print one line so
// users running the watchdog see the prompt to type /update.
//
// The dedicated vrcresolver.Updater.exe still owns the actual upgrade flow
// (download / SHA-verify / atomic swap / relaunch). This is just the
// "by the way, X is out" line the legacy AppUpdateChecker used to surface
// in the UI; restored here as console output.
//
// Failure modes are silenced — a network outage, GitHub rate-limit, or
// parser drift must never delay the watchdog's startup or scroll a
// stack trace over the operator's banner.
internal static class UpdateCheck
{
    private const string Repo = "RealWhyKnot/VRCResolver";
    // /releases/latest skips prereleases by GitHub convention; the list
    // endpoint includes them and we filter ourselves when the user has
    // not opted in. Page size 10 covers any realistic run of consecutive
    // prereleases between two stable tags without paging.
    private const string StableLatestUrl = "https://api.github.com/repos/" + Repo + "/releases/latest";
    private const string AnyReleasesUrl = "https://api.github.com/repos/" + Repo + "/releases?per_page=10";
    private static readonly TimeSpan RequestTimeout = TimeSpan.FromSeconds(8);

    public static void StartBackgroundCheck()
    {
        if (!AppSettingsStore.Shared.Snapshot().Maintenance.UpdateCheck)
        {
            Logger.WriteFileOnly("[update] startup update check disabled by settings");
            return;
        }

        _ = Task.Run(RunAsync);
    }

    private static async Task RunAsync()
    {
        try
        {
            bool includePrereleases = AppSettingsStore.Shared.Snapshot().Maintenance.IncludePrereleases;

            using var http = new HttpClient { Timeout = RequestTimeout };
            var asmVer = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "0.0.0.0";
            http.DefaultRequestHeaders.UserAgent.ParseAdd("VRCResolver-Watchdog/" + asmVer);
            // GitHub returns 403 without an Accept header in some scenarios.
            http.DefaultRequestHeaders.Accept.ParseAdd("application/vnd.github+json");

            string apiUrl = includePrereleases ? AnyReleasesUrl : StableLatestUrl;
            using var resp = await http.GetAsync(apiUrl).ConfigureAwait(false);
            if (!resp.IsSuccessStatusCode) return;
            var json = await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
            using var doc = JsonDocument.Parse(json);

            (string tag, string htmlUrl, bool isPrerelease)? best = includePrereleases
                ? PickHighestFromList(doc.RootElement)
                : PickSingleObject(doc.RootElement);
            if (best == null || string.IsNullOrEmpty(best.Value.tag)) return;

            // Tag is like "v2026.5.3.1-A930"; the AssemblyVersion is just
            // the numeric portion. Strip the leading 'v' and any "-XXXX"
            // dev suffix so we compare apples to apples.
            string tagNumeric = best.Value.tag.TrimStart('v', 'V');
            int dash = tagNumeric.IndexOf('-');
            if (dash >= 0) tagNumeric = tagNumeric[..dash];

            if (!Version.TryParse(tagNumeric, out var remote)) return;
            var local = Assembly.GetEntryAssembly()?.GetName().Version;
            if (local == null) return;
            if (remote <= local) return;

            string channelTag = best.Value.isPrerelease ? " (prerelease)" : "";
            ConsoleUx.Success(
                LogComponent.Update,
                "version " + best.Value.tag + channelTag
                    + " is available; type /update to install" +
                (string.IsNullOrEmpty(best.Value.htmlUrl) ? "" : " (" + best.Value.htmlUrl + ")"));
        }
        catch
        {
            // Network outage / parse drift / rate-limit. Silent on purpose
            // — this is observability, not load-bearing.
        }
    }

    private static (string tag, string htmlUrl, bool isPrerelease)? PickSingleObject(JsonElement element)
    {
        if (element.ValueKind != JsonValueKind.Object) return null;
        string tag = element.TryGetProperty("tag_name", out var tagEl) ? (tagEl.GetString() ?? "") : "";
        string url = element.TryGetProperty("html_url", out var urlEl) ? (urlEl.GetString() ?? "") : "";
        bool pre = element.TryGetProperty("prerelease", out var preEl)
            && preEl.ValueKind == JsonValueKind.True;
        return (tag, url, pre);
    }

    // List endpoint returns releases newest-first by publish date, but the
    // newest-published isn't necessarily the highest version (a stable
    // patch can land after a prerelease for the next major). Pick the
    // highest comparable version we can parse so the prompt always points
    // at the actual upgrade rather than the most-recent tag. Ties (same
    // numeric version, different prerelease flag) resolve to the stable
    // entry so an opted-in user is never told to install a prerelease
    // when an equivalent-version stable exists.
    internal static (string tag, string htmlUrl, bool isPrerelease)? PickHighestFromList(JsonElement list)
    {
        if (list.ValueKind != JsonValueKind.Array) return null;
        Version? bestVersion = null;
        (string tag, string htmlUrl, bool isPrerelease)? best = null;
        foreach (JsonElement entry in list.EnumerateArray())
        {
            if (entry.ValueKind != JsonValueKind.Object) continue;
            string tag = entry.TryGetProperty("tag_name", out var tagEl) ? (tagEl.GetString() ?? "") : "";
            if (string.IsNullOrEmpty(tag)) continue;

            string numeric = tag.TrimStart('v', 'V');
            int dash = numeric.IndexOf('-');
            if (dash >= 0) numeric = numeric[..dash];
            if (!Version.TryParse(numeric, out Version? parsed)) continue;

            string url = entry.TryGetProperty("html_url", out var urlEl) ? (urlEl.GetString() ?? "") : "";
            bool pre = entry.TryGetProperty("prerelease", out var preEl)
                && preEl.ValueKind == JsonValueKind.True;

            if (bestVersion != null)
            {
                int cmp = parsed.CompareTo(bestVersion);
                if (cmp < 0) continue;
                // Same numeric version: prefer the stable entry.
                if (cmp == 0 && (pre || !best!.Value.isPrerelease)) continue;
            }

            bestVersion = parsed;
            best = (tag, url, pre);
        }
        return best;
    }
}
