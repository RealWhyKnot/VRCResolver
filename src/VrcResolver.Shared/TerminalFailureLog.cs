namespace VrcResolver.Shared;

public static class TerminalFailureLog
{
    public static string Build(
        string requestId, string domain, string player,
        string serverReason, string bundledReason, long elapsedMs)
        => "terminal_failure request_id=" + requestId
            + " domain=" + domain
            + " player=" + player
            + " server_reason=" + serverReason
            + " bundled_ytdlp_reason=" + bundledReason
            + " elapsed_ms=" + elapsedMs;
}
