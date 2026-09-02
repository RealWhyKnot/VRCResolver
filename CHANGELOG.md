# Changelog

All notable changes to VRCResolver. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses date-driven versioning (`YYYY.M.D.N` for releases, `YYYY.M.D.N-XXXX` for development builds).

Release entries are listed newest first. This changelog starts with the first public GitHub release.

<!-- Entries under "## Unreleased" are appended automatically by the changelog-append GitHub
     workflow on every push to main, then promoted to the versioned section by release.yml when
     a tag is cut. Keep this section public-facing and concise. To override an entry, amend the
     commit subject before merge or mark the commit [skip changelog]. -->

## Unreleased

### Fixed
- **wrapper:** Skip the post-og re-ask when the server already re-raced (cc1393f)

---

## [v2026.9.1.0](https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.9.1.0) - 2026-09-01

### Added
- **terminal:** Complete as you type, suggest on typos, and edit mid-line (ea3d61a)
- **client:** Add a high quality option for the best rung a source offers (d7c4a1b)

### Fixed
- **terminal:** Repaint the prompt while typing into a suggestion (fc13a5f)
- **resolve:** Return to the server after a failed native fallback (7ce6095)
- **wrapper:** Anchor the server retry hint and never exit 0 without a url (e5acf59)
- **wrapper:** Fail og timeouts, cap default avpro height at 1080, split client deadline reason (573184a)

---

## [v2026.8.21.0](https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.8.21.0) - 2026-08-21

### Added
- **watchdog:** Resolver health gate pauses mesh resolving after repeated failures (40b6c52)
- **shared:** Blocked-address policy, resolved-url emit guard, relay liveness probe (8b42d4b)
- **client:** Verified codec claims and the richer server wire contract (3f6e8f6)

### Changed
- Bump the minor-and-patch group with 3 updates (#71) (9b6106d)
- Bump the minor-and-patch group with 1 update (#72) (773dad4)
- style(shared): ascii punctuation in new comments (849afb7)
- **watchdog:** Group sources into subfolders (a2c53ed)
- Bump xunit.runner.visualstudio from 3.1.5 to 4.0.0 (#73) (3eef23c)

### Fixed
- **mesh:** Fail pending resolves on clean close and lost sends, bound ipc fallback write (167d2f5)
- **mesh:** Discovery redirects must land on a first-party dns host (c887d77)
- **relay:** Connect-time address guard, origin rejection, in-flight cap (76503c5)
- **wrapper:** Emit-shape guard, relay probe on direct urls, bounded og wait, stdout write hardening (d0dd3fc)

---

## [v2026.7.14.0](https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.7.14.0) - 2026-07-14

### Breaking
- Rename product to vrcresolver (d8dd29b)

### Changed
- Bump the minor-and-patch group with 1 update (#68) (7da7132)

---

## [v2026.6.30.1](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.6.30.1) - 2026-06-30

### Fixed
- **release:** Use central date for changelog promotion (c7a570d)
- **wrapper:** Fall back to native yt-dlp when og backup is missing (013bae9)

---

## [v2026.6.15.1](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.6.15.1) - 2026-06-16

### Changed
- **proxy:** Remove client gpu helper sharing (aec5a6a)

---

## [v2026.6.15.0](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.6.15.0) - 2026-06-15

### Fixed
- **vrc:** Arm native fallback on playback failure (4c506cd)
- **updater:** Recover release update flow (a70a452)

---

## [v2026.6.12.0](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.6.12.0) - 2026-06-12

### Changed
- Refactor proxy client hotspots (27aa711)
- Bump the minor-and-patch group with 1 update (#63) (cc33fb8)
- Bump MessagePack and MessagePackAnalyzer (#64) (eb1e893)

### Fixed
- **mesh:** Use proxy public endpoint (648c66b)
- **resolve:** Forward wrapper height caps (e5d25a3)
- **build:** Keep BuildInfo generated under obj (3100fd2)

---

## [v2026.5.27.0](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.5.27.0) - 2026-05-27

### Added
- **helper:** Surface helper_eligibility_skipped frames on watchdog console (f402c53)
- **helper:** Hold-and-announce flow for window-pull leases (100e71f)
- **updater:** Opt-in to prereleases for both startup nudge and updater (44d6b81)
- **wrapper:** Reactive og fallback on observed AVPro load_failure (59dc5d7)
- **wrapper:** Surface server public_message via og_fallback_notify (58e2c35)

### Changed
- **helper:** Hardcode GPU throttle; refuse integrated GPUs (79439bd)
- Bump the minor-and-patch group with 2 updates (#61) (b4528b1)

### Fixed
- **helper:** NVENC scale_cuda filter syntax and NVDEC reference-frame pool (0d339d3)
- **helper:** Route seg 0 through software decode + reject empty output (0aca4ac)
- **helper:** Widen NVDEC pool to 32 + retry truncated video via software (ab18327)
- **helper:** Run video-duration check on every decode path (f66790f)
- **helper:** Fall back to container duration when stream tag missing (b39602a)
- **relay:** Scope manifest classification to .m3u8/.mpd extensions (d23943d)
- **vrclog:** Avoid replaying old failures on startup (f2bc3d0)
- **relay:** Retry fresh port after bind failure (07c9ba7)
- **helper:** Drop +discardcorrupt and emit SPS at every IDR (3c0eb3e)

---

## [v2026.5.16.0](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.5.16.0) - 2026-05-16

### Added
- **helper:** Expand helper lease + resolve diagnostics (29d82cf)
- **helper:** Trust key challenge, encoder smoke test, pre-upload validation (9f913f4)
- **wrapper:** Retry resolve on discovery_in_progress with deadline-aware hold (49fb3b7)
- **wrapper:** Bump resolve deadline from 18s to 28s (c89e234)
- **wrapper:** Classify og-fallback content_not_found patterns (c7a4376)

### Fixed
- **wrapper:** Re-establish pipe per retry so resolve retries can actually send (5070472)
- **ipc:** Align watchdog per-request budget with wrapper deadline (c64298e)

---

## [v2026.5.14.0](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.5.14.0) - 2026-05-14

### Added
- **watchdog:** Add interactive terminal and HTTPS relay bootstrap (b0475d0)
- **watchdog:** Add advanced terminal renderer (11749d3)
- **helper:** Ship ffmpeg and handle transcode leases (f150370)
- **helper:** Add hitch diagnostics and benchmarked presets (036692e)
- **mesh:** Playback_feedback emits delivered_height + kind=playing telemetry (f133ffb)
- **helper:** Add ffmpeg hardware decode fallback (2caac69)

### Changed
- Improve helper diagnostics and terminal input (3988dad)
- **patcher:** Identify wrappers by marker; drop bundled yt-dlp (0286d54)

### Fixed
- **relay:** Stream-localize manifests so playback_id tokens dont 502 (fa74463)

---

## [v2026.5.10.4](https://github.com/RealWhyKnot/WKVRCProxy/releases/tag/v2026.5.10.4) - 2026-05-10

### Added
- Local WhyKnot trust gateway for VRChat video players, including `localhost.youtube.com` playback URLs that keep first-party WhyKnot streams inside the allowed playback path.
- Direct handling for pasted `whyknot.dev` playback proxy URLs, so a first-party proxy URL in a video player resolves to a local manifest instead of recursively re-resolving itself.
- Local HLS/DASH manifest localization for first-party WhyKnot proxy URLs, including child manifests and segment URLs with stable local names.
- Mesh client support for WhyKnot backend protocol negotiation, binary response handling, cached welcomes, reconnects, and playback feedback.
- WKVRCProxy updater and uninstaller executables with zip verification, rollback-aware updates, hosts cleanup, state cleanup, and install-folder removal.
- Persistent logs, crash snapshots, startup/runtime context, and redacted error reporting.

### Changed
- Reorganized the client into focused modules for URL policy, relay target validation, manifest rewriting, header forwarding, port tracking, resolve cache, and wrapper behavior.
- Hardened relay shutdown and cleanup so port files, patched binaries, sidecar files, state files, named pipes, and child processes are cleaned when possible.
- Build and release pipeline now signs tagged builds, emits a per-file SHA256 manifest, and gates release notes for public wording and ASCII output.
- Documentation now describes the current watchdog-only architecture, trust gateway behavior, quick start, updater, uninstaller, build pipeline, and release process.

### Fixed
- Prevented non-WhyKnot URLs and local gateway URLs from being wrapped into the trust gateway accidentally.
- Rejected unsafe relay requests with invalid Host headers, unsupported HTTP methods, non-HTTP targets, or non-WhyKnot playback targets.
- Avoided stale localhost gateway cache entries by canonicalizing playback feedback back to first-party WhyKnot target URLs.
- Kept upstream manifest responses uncompressed while they are inspected so headers and body content stay consistent.
- Preserved third-party manifest URLs and data URIs during localization so external media references are not rewritten incorrectly.
- Added regression coverage for direct WhyKnot URLs, Popcorn proxy URLs, manifest localization, local-gateway canonicalization, cache eviction, target validation, and relay host validation.
