# VRCResolver

VRChat plays videos through yt-dlp. Stock yt-dlp is slow, breaks whenever YouTube changes something, and returns URLs that AVPro blocks in public worlds. VRCResolver replaces VRChat's `Tools/yt-dlp.exe` with a patched build that resolves videos through vrcresolver.com and serves them from a local address AVPro trusts. If anything fails, it falls back to VRChat's original yt-dlp, so playback is never worse than stock.

Formerly WKVRCProxy. Old installs update automatically.

**[Report a bug](https://github.com/RealWhyKnot/VRCResolver/issues/new?template=bug_report.yml)**

## Features

- **Works in public worlds.** Streams are served from `localhost.youtube.com:{port}`, which AVPro's trust list accepts.
- **Resolves on a server, not your PC.** Regional blocks and rate limits don't apply, and the server updates its yt-dlp nightly, so YouTube breakage gets fixed without you doing anything.
- **Fast.** 2-3 seconds for a new URL, about 20 ms for a repeat.
- **Better quality.** 1080p HLS instead of 360p mp4.
- **Never breaks playback.** Any failure hands the URL to VRChat's original yt-dlp.

No DRM bypass, no content hosting, no YouTube login.

## Install

1. Launch VRChat once, so `Tools/yt-dlp.exe` exists for the patcher to back up.
2. Download the latest `vrcresolver-*.zip` release and extract it anywhere except `Program Files`.
3. Run `vrcresolver.exe` and accept the one-time UAC prompt. It adds `127.0.0.1 localhost.youtube.com` to your hosts file, which public-world playback needs.
4. Launch VRChat. When the console shows `[mesh] connected`, paste a video URL into any in-world player.

**Update:** type `/update` in the console.
**Uninstall:** run `vrcresolver.Uninstaller.exe`. It restores the original `yt-dlp.exe`, removes the hosts entry, and wipes `%LOCALAPPDATA%Low\vrcresolver\`. There is no confirmation prompt.

Windows 10/11 x64. Self-contained, no installer.

## Troubleshooting

The console prints one line per resolve: green = resolved, yellow = fell back to stock yt-dlp, red = error. Full logs are in `%LOCALAPPDATA%Low\vrcresolver\logs\`. When filing a bug, include the correlation-ID block for the failed resolve.

## How it works

```
VRChat (AVPro)
   v  paste URL
Tools/yt-dlp.exe        patched shim
   v  named pipe
vrcresolver.exe         watchdog
   v  WebSocket
vrcresolver.com         remote resolver
```

The resolved stream comes back through the watchdog's local listener at `http://localhost.youtube.com:{port}/play/<session>/manifest.<ext>?target=...`. If any link breaks, the shim runs the backed-up `yt-dlp-og.exe` instead.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE) for the full text and [NOTICE](NOTICE) for third-party attributions.
