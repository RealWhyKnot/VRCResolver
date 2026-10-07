# Contributing to VRCResolver

Thanks for your interest.

## Before you start

- The project is alpha, and behaviour changes faster than the docs do. If they disagree, go by the code.
- I maintain it alone. For anything bigger than a small fix, open an issue first so we can agree on the direction before you spend time on it.
- The codebase avoids heavy abstraction on purpose. A new feature is usually a method on an existing class.

## Setting up

You need Windows 10/11 x64, the .NET 11 SDK, PowerShell 5.1+ and Git.

```powershell
git clone https://github.com/RealWhyKnot/VRCResolver.git
cd vrcresolver
powershell -ExecutionPolicy Bypass -File build.ps1
```

The first `build.ps1` run also sets `git config core.hooksPath .githooks`, which turns on the commit-msg hook.

## Branches and PRs

1. Fork and branch off `main`. Branch names are up to you (`fix/...`, `feat/...` and `chore/...` are common).
2. Make your change. Small diffs please. A "while I'm here" cleanup is its own PR.
3. `dotnet build vrcresolver.slnx` has to pass with `-warnaserror`.
4. Run `powershell -File build.ps1 -SkipZip` once on the branch before opening the PR. It catches publish problems that `dotnet build` misses.
5. Open the PR and fill in the template.

## Commit messages

Subjects follow the existing log: `type(scope?): short summary (YYYY.M.D.N-XXXX)`.

- `type` is one of `feat`, `fix`, `build`, `docs`, `refactor`, `test`, `chore`.
- `build.ps1` produces the `(YYYY.M.D.N-XXXX)` build-version stamp at the end. Don't paste the same stamp twice in one subject. The `.githooks/commit-msg` hook rejects duplicates.
- The body explains why. The diff already shows what.

## Code style

C# follows the surrounding file. For decisions that aren't obvious, leave a short comment saying why.

## What to avoid in PRs

- Refactors bundled with feature work. Split them.
- A new class for a one-off case.
- Any code path that can leave VRChat with a broken `yt-dlp.exe`, without either the patched build or the fallback to the original. That fallback always has to work.

## Bugs and questions

- Bugs: use the [bug report template](https://github.com/RealWhyKnot/VRCResolver/issues/new?template=bug_report.yml) and paste the watchdog console output around the failure exactly as it appeared.
- Questions and setup help: open a [Discussion](https://github.com/RealWhyKnot/VRCResolver/discussions). I'll convert it to an issue if it turns out to be a bug.
- Security issues: see [SECURITY.md](.github/SECURITY.md) and use the private GitHub advisory flow, never a public issue.
