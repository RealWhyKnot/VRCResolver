# Security policy

vrcresolver is alpha-stage software with elevated trust requirements on the user's machine: it modifies the Windows hosts file (admin), patches a binary VRChat ships (`yt-dlp.exe`), and runs a local relay HTTP server. Vulnerability reports are taken seriously even though the project is small.

## Reporting

**Do not open a public issue for security reports.**

Use [GitHub Security Advisories](https://github.com/RealWhyKnot/VRCResolver/security/advisories/new). It's a private channel where we can coordinate a fix and disclosure timeline.

We try to acknowledge new reports within **7 days** and aim for an initial assessment within **14 days**. There is no bug bounty.

## In scope

- Local privilege escalation, unsafe admin-elevated operations, or hosts-file tampering by an unprivileged caller.
- Anything that lets a remote URL or VRChat world cause VRCResolver to execute attacker-controlled code, exfiltrate local files, or persist beyond the running session.
- The local relay server, IPC servers (HTTP / pipe / WebSocket), or any endpoint reachable from `localhost` while VRCResolver is running, if they expose unintended capabilities.
- Patcher behaviour (`PatcherService.cs`) writing or restoring the wrong file, or being induced to corrupt VRChat's install.
- Update / fetch paths in `build.ps1` that could be tricked into installing a tampered binary.

## Out of scope

- VRChat client behaviour, AVPro behaviour, or the trusted-host allowlist itself.
- Issues that require an attacker to already have admin access on the user's machine.
- "Loading failed" / playback failures are functional bugs, not security issues. Use the bug-report issue template.

## Disclosure

We prefer coordinated disclosure: we'll work with the reporter on a fix and a timeline before publishing details. Credit in the advisory is given by default unless the reporter prefers anonymity.
