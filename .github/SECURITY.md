# Security policy

vrcresolver is alpha software that needs a lot of trust on the user's machine. It edits the Windows hosts file (as admin), patches the `yt-dlp.exe` that comes with VRChat, and runs a local relay HTTP server. I take vulnerability reports seriously even though the project is small.

## Reporting

**Don't open a public issue for security reports.**

Use [GitHub Security Advisories](https://github.com/RealWhyKnot/VRCResolver/security/advisories/new). It's a private channel where we can work out a fix and a disclosure timeline.

I try to acknowledge new reports within **7 days** and give a first assessment within **14 days**. There's no bug bounty.

## In scope

- Local privilege escalation, unsafe operations run as admin, or an unprivileged caller tampering with the hosts file.
- Anything that lets a remote URL or VRChat world make VRCResolver run attacker-controlled code, exfiltrate local files, or persist beyond the running session.
- The local relay server, the IPC servers (HTTP, pipe, WebSocket), or any endpoint reachable from `localhost` while VRCResolver is running, if they expose capabilities they shouldn't.
- The patcher (`PatcherService.cs`) writing or restoring the wrong file, or being tricked into corrupting VRChat's install.
- Update and fetch paths in `build.ps1` that could be tricked into installing a tampered binary.

## Out of scope

- VRChat client behaviour, AVPro behaviour, or the trusted-host allowlist itself.
- Issues that need an attacker to already have admin access on the user's machine.
- "Loading failed" and other playback failures. Those are functional bugs. Use the bug-report issue template.

## Disclosure

I prefer coordinated disclosure: I'll work with the reporter on a fix and a timeline before publishing details. Reporters are credited in the advisory by default unless they ask not to be named.
