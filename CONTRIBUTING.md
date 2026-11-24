# Contributing to VRCResolver

VRCResolver is built for my own use first and I maintain it solo. I'm picky about how the codebase is structured, how it's styled, and how features fit together. If you hit a bug or want something changed, please [open an issue](https://github.com/RealWhyKnot/VRCResolver/issues/new) instead of sending a surprise pull request. A description of the problem, steps to reproduce, or what you're trying to do helps me far more than an unannounced PR.

If you'd like to help maintain the project over time, or you have a substantial change in mind, open an issue or reach out first. I'm happy to talk through the approach and agree on code style and commit conventions before you spend time writing code.

The project is alpha and behavior changes faster than the docs do. If they disagree, go by the code. Please follow the [code of conduct](CODE_OF_CONDUCT.md).

## Bugs and questions

If you hit a bug or have a question, open an issue and paste the console output around the failure exactly as it appeared. For security problems, see [SECURITY.md](.github/SECURITY.md) and use the private GitHub advisory flow, never a public issue.

## Building and checks

You need Windows 10 or 11 x64, a stable Rust toolchain from rustup with the Visual Studio C++ build tools for the MSVC linker, PowerShell 5.1 or newer, and Git.

```powershell
git clone https://github.com/RealWhyKnot/VRCResolver.git
cd VRCResolver
powershell -ExecutionPolicy Bypass -File build.ps1
```

The first `build.ps1` run also sets `git config core.hooksPath .githooks`, which turns on the commit hooks.

Before anything goes up for review:

- `.github/lint/lint.ps1` and `cargo test --workspace` pass. The lint script runs `cargo fmt --check` and `cargo clippy` with warnings treated as errors.
- `powershell -File build.ps1 -SkipZip` succeeds. It builds the release exes and checks their version stamps, which a plain `cargo build` skips. CI runs the same command.

## If we agree on a PR

- Link the issue where we settled the approach.
- Keep the diff small. A "while I'm here" cleanup is its own PR.
- Don't bundle refactors with feature work, and don't add a new type or trait for a one-off case. The codebase avoids heavy abstraction on purpose, so a new feature usually goes into an existing module.
- Rust code follows the surrounding file and `rustfmt.toml`. Don't add code comments unless we agreed on one.
- Don't add any code path that can leave VRChat with a broken `yt-dlp.exe`. The fallback to the original has to keep working.
- If behavior changed, add or update a test.

## Commit messages

Subjects follow the existing log: `type(scope?): short summary (YYYY.M.D.N-XXXX)`.

- `type` is one of `feat`, `fix`, `chore`, `ci`, `docs`, `refactor`, `revert`, `test`, `perf`, `diag`, `style`. The `commit-msg` hook enforces this, with an optional lowercase `(scope)`.
- The `(YYYY.M.D.N-XXXX)` part is a build-version stamp. A `prepare-commit-msg` hook appends it from `version.txt`, which `build.ps1` updates (the hook makes one for the day if none exists). Don't type it yourself.
- The `commit-msg` hook rejects a subject that has more than one stamp.
- The body, if there is one, explains why. The diff already shows what.
