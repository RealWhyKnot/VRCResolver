using System.Net.Http;
using System.Reflection;
using System.Text.Json;
using VrcResolver.Shared;

namespace VrcResolver;

internal static class UpdateCheck
{
    private const string Repo = "RealWhyKnot/VRCResolver";
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
                if (cmp == 0 && (pre || !best!.Value.isPrerelease)) continue;
            }

            bestVersion = parsed;
            best = (tag, url, pre);
        }
        return best;
    }
}
