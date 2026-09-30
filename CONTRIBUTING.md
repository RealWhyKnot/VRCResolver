# Contributing to VRCResolver

Thanks for your interest.

## Before you start

- The project is alpha. Behaviour changes faster than docs do; trust the code over old commentary.
- This is a single-maintainer repo. For anything beyond a small fix, open an issue first so we can agree on direction before you spend time.
- The codebase deliberately avoids heavy abstraction. A new feature is usually a method on an existing class, not a new class.

## Setting up

You need: Windows 10/11 x64, .NET 10 SDK, PowerShell 5.1+, Git.

```powershell
git clone https://github.com/RealWhyKnot/VRCResolver.git
cd vrcresolver
powershell -ExecutionPolicy Bypass -File build.ps1
```

The first `build.ps1` run also configures `git config core.hooksPath = .githooks`, which activates the commit-msg hook.

## Branches and PRs

1. Fork, branch off `main`. Branch name is up to you (`fix/...`, `feat/...`, `chore/...` are common).
2. Make your change. Keep diffs small and focused. A "while I'm here" cleanup is its own PR.
3. `dotnet build vrcresolver.slnx` must pass with `-warnaserror`.
4. Run `powershell -File build.ps1 -SkipZip` once on the branch before opening the PR. It catches publish issues that `dotnet build` won't.
5. Open the PR; the template tells you what to fill in.

## Commit messages

Subject style follows the existing log: `type(scope?): short summary (YYYY.M.D.N-XXXX)`.

- `type` is one of `feat`, `fix`, `build`, `docs`, `refactor`, `test`, `chore`.
- The `(YYYY.M.D.N-XXXX)` build-version stamp at the end is automatic: `build.ps1` produces it. **Do not paste the same stamp twice in one subject.** The `.githooks/commit-msg` hook will reject duplicates.
- Body explains the *why*, not the *what*. The diff is the what.

## Code style

- C# follows the surrounding file.
- For non-obvious decisions, leave a short comment on why.

## What to avoid in PRs

- Refactors bundled with feature work. Split them.
- Introducing a new class for a one-off case.
- Code paths that can leave VRChat with a broken `yt-dlp.exe` (no patched build, no fallback). The og-fallback invariant is non-negotiable.

## Bugs and questions

- Bug: use the [bug report template](https://github.com/RealWhyKnot/VRCResolver/issues/new?template=bug_report.yml). Include the watchdog console output around the failure verbatim.
- Question or setup help: open a [Discussion](https://github.com/RealWhyKnot/VRCResolver/discussions). We'll convert to an issue if it turns out to be a bug.
- Security issue: see [SECURITY.md](.github/SECURITY.md). Use the private GitHub advisory flow, never a public issue.
