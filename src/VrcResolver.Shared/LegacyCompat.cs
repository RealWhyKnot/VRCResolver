namespace VrcResolver.Shared;

public static class LegacyCompat
{
    public const string LegacyWatchdogMutexName = "Global\\WKVRCProxy.Watchdog";
    public const string LegacyWatchdogMutexNameLocal = "Local\\WKVRCProxy.Watchdog";

    public const string LegacyWrapperMarker =
        "WKVRCPROXY_WRAPPER_MARKER_v1:9b3e7c8a-7f23-4e6b-9c1d-a4f8e0d2c5b6";

    public static ReadOnlySpan<byte> LegacyWrapperMarkerUtf8 =>
        "WKVRCPROXY_WRAPPER_MARKER_v1:9b3e7c8a-7f23-4e6b-9c1d-a4f8e0d2c5b6"u8;

    public const string LegacyProductName = "WKVRCProxy";

    public const string LegacyStateDirName = "WKVRCProxy";

    public const string LegacyPipeName = "WKVRCProxy.resolve";

    private const string EnvPrefix = "VRCRESOLVER_";
    private const string LegacyEnvPrefix = "WKVRCPROXY_";

    public static string? GetEnvWithLegacyFallback(string suffix)
    {
        return Environment.GetEnvironmentVariable(EnvPrefix + suffix)
            ?? Environment.GetEnvironmentVariable(LegacyEnvPrefix + suffix);
    }
}
