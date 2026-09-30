# Release workflow

Releases are tag-driven and fully automated. Pushing a `v*` tag triggers
[release.yml](workflows/release.yml), which builds the dist, publishes a
GitHub release, and verifies the published body matches the input. The
release body is generated from `git log` between the previous release base
and the current tag. There is no hand-written narrative path. Stable
release tags use the previous stable tag as the base, so beta release notes
since the last stable are included when the stable release is published.
Beta and dev tags use the nearest previous tag. If a release needs content
that the auto-generator can't produce, the only supported path is the
[extras file](#extras-file).

The workflow calls `build.ps1 -Package`; local builds do not create a
repo-root `release/` directory. The zip, build manifest, and release
integrity TSV are emitted under `dist/` for the workflow to upload.

## Tag shapes

| Form | When | Example |
|---|---|---|
| `vYYYY.M.D.N` | Release. `.N` is the release iteration for that calendar day, starting at 0. | `v2026.5.5.0` |
| `vYYYY.M.D.N-beta` | Prerelease. Used for beta builds that should not become the latest stable release. | `v2026.5.5.0-beta` |
| `vYYYY.M.D.N-XXXX` | Dev. `.N` is local build count; `XXXX` is a 4-hex UID. Rare on the release stream. | `v2026.5.5.0-A1B2` |

[build.ps1](../build.ps1) validates the shape via the regex
`^\d{4}\.\d+\.\d+\.\d+(-([A-Fa-f0-9]{4}|beta))?$` and fails fast on malformed tags.

## Body composition

The release body is the verbatim output of
[Generate-ReleaseNotes.ps1](scripts/Generate-ReleaseNotes.ps1). The body points
readers at `vrcresolver-v<version>.integrity.tsv`; the checksum values live in
that TSV asset. Layout:

```
# vrcresolver <tag>

## What's Changed

### Features
- feat(...): subject by @author in <sha>

### Bug Fixes
- fix(...): subject by @author in <sha>

**Full Changelog**: <compare-url>

## File integrity

Full SHA256 hashes are attached as `<integrity TSV>`.

[Additional notes (extras file, if present)]
```

Stable release bodies deliberately skip beta tags when picking the compare
base. For example, if `v2026.5.5.0-beta` ships after `v2026.5.1.0`, then
`v2026.5.7.0` compares from `v2026.5.1.0` so the beta changes appear in the
stable release notes too. There is no curated `## Unreleased` excerpt above
the auto section; CHANGELOG.md remains the browsable in-repo history.

## Conventional-commit policy

Commits in the tag range get bucketed by the prefix on their subject line:

| Prefix | Section |
|---|---|
| `feat(...)?:` | Features |
| `fix(...)?:` | Bug Fixes |
| `perf(...)?:` | Performance |
| `refactor(...)?:` | Refactors |
| `revert(...)?:` | Reverts |
| `docs(...)?:` | Documentation |
| `style(...)?:` | Style |
| `test(...)?:` | Tests |
| `ci(...)?:` | CI |
| `build(...)?:` | Build |
| `chore(...)?:` | Chores |
| anything else | Other Changes |

Subjects starting with `<prefix>!:` (the breaking-change marker) match the
same section. Trailing version-stamps like ` (2026.5.5.0-A1B2)` are stripped
before grouping. Commits with `[skip changelog]` in the subject are excluded
entirely; merge commits are excluded by `--no-merges`.

The generator emits a workflow warning for each non-conforming subject so
the operator can amend if desired. Non-conforming subjects ship under
`Other Changes` and the build doesn't fail.

## Author handle remap

The generator emits `by @<author>` from the commit's `%an` field. GitHub
@-mentions only resolve when the handle matches an actual login. The repo's
local git config uses the brand "WhyKnot"; the GitHub login is "RealWhyKnot".
The generator carries an `$AuthorHandleMap` hashtable that remaps known git
authors to the right login before emit. If a new author lands on the repo,
add a mapping or expect their @-mention to render as plain text.

## Scrub gates

After the body is composed, two gates run before the workflow proceeds:

1. **ASCII normalisation.** A fixed table of common typographic patterns
   (em-dash, en-dash, ellipsis, smart quotes, NBSP, bullet, multiplication
   sign, arrows, section sign, pilcrow) is substituted to ASCII equivalents.
   The substitution is one-way and silent.

2. **Non-ASCII fail.** Anything left outside the printable-ASCII range
   (0x20-0x7E plus tab) after normalisation fails the script with the
   offending line, column, and Unicode code point. Fix by amending the
   offending commit subject (or extras file) to use ASCII, OR add the
   character to the substitution table in the generator.

3. **Voice + internal-only-vocabulary check.** Four pattern groups match
   case-insensitively against the composed body: marketing puffery (words
   like `comprehensive` and `leveraging`), internal-only vocabulary
   (process-tracking nouns that don't belong in public release notes),
   future-tense rhetoric, and unverified time-of-effort claims. Any match
   fails the script with the offending pattern and match position. Fix by
   amending the offending commit subject (or extras file) to use plainer
   language, OR mark the commit `[skip changelog]` if the term is
   unavoidable in-repo (e.g. a refactor of an internal class whose name
   trips a pattern).

The scrub list lives in
[Generate-ReleaseNotes.ps1](scripts/Generate-ReleaseNotes.ps1). Add patterns
there if a new tell shows up in the wild.

## Empty-slice guard

If the tag range yields zero qualifying commits (everything was `[skip
changelog]`, the prev-tag detection is wrong, or the tag was pushed from
an empty branch), the script throws and the workflow fails. There is one
escape hatch: pass `-AllowEmpty` for the very first release on a repo
where there is no prior tag and the range trick produces nothing
meaningful. The release body in that case is a stub the operator can
replace via `gh release edit`.

## Extras file

For content that the auto-generator can't capture, server-side coordination
notes, migration instructions, operational context, etc., create
a markdown file at `.github/release-extras/<tag>.md` BEFORE pushing the tag.
The file's contents get appended verbatim below the auto section with a
`---` separator and an `## Additional notes` heading.

```
.github/release-extras/v2026.5.5.5.md   <- created before tag push
```

The same scrub gates run on the composed body, so an em-dash or "comprehensive
solution" in an extras file fails the workflow just like it would in a commit
subject.

The file is optional. Most releases don't need one. If you find yourself
reaching for an extras file every release, consider whether the content
should be a commit subject instead.

## Post-publish verification

After `gh release create` succeeds, the workflow re-fetches the published
body via `gh release view --json body` and compares it byte-for-byte
(line-ending-normalised) to the input. If they differ:

1. Workflow logs a warning and runs `gh release edit --notes-file <input>`
   to auto-correct.
2. Re-fetches the body to confirm the correction landed.
3. If they still differ, fails the workflow loud with both files persisted
   to the runner temp dir for inspection.

This catches GitHub-side normalisation surprises (which are rare but real)
without letting a malformed body sit on the release indefinitely.

## Changelog promotion

The workflow promotes `## Unreleased` to the tagged section before the build
so the embedded `CHANGELOG.md` matches the shipped release. Before promotion,
it also replays the commit range since the previous stable tag into
`## Unreleased`; that keeps the release notes correct when the tag job starts
before the push-triggered changelog appender has committed back to main.

After the release publishes, the stashed promoted `CHANGELOG.md` is committed
back to main with GitHub's `createCommitOnBranch` mutation. This is the same
verified-commit path used by `changelog-append.yml` and avoids the old
promotion-branch PR flow.

## Failure modes + remediations

| Symptom | Fix |
|---|---|
| `No commits found in range` | Check the tag's parent reachability. Either the prev-tag detection failed (push the actual prev tag) or every commit is `[skip changelog]` (push a real change before tagging). |
| `Non-ASCII characters in release body after normalisation` | Find the offending commit subject, amend it to use ASCII, force-push the tag at the new SHA. Or add the char to `$asciiSubs` in the generator. |
| `Voice or internal-only-vocabulary patterns in release body` | Amend the offending commit subject. Or `[skip changelog]` it if the term is genuinely unavoidable. |
| `Generate-ReleaseNotes.ps1 returned empty output` | The script failed silently or the slice was empty. Check the workflow log for warnings; if the slice really is empty, the empty-slice guard would have already thrown, so this is a script bug. |
| `Release body still differs after auto-correct` | A GitHub-side issue. Compare the input file in the runner artifacts against what `gh release view` returns. Often a trailing-whitespace or unicode-form difference. |
| `createCommitOnBranch returned GraphQL errors` | Main moved after the workflow read its head, or GitHub rejected the file update. Re-run the workflow after main settles; if it repeats, inspect the GraphQL error text and push the same CHANGELOG.md promotion in the next source commit. |

## Updating the workflow

The workflow + scripts are versioned alongside the code. Changes go through
the same PR-or-direct-to-main flow as anything else. After landing a
workflow change, the next genuine release exercises it. If the workflow
breaks mid-release, the tag is already pushed and gh's partial state may
need cleanup. In extreme cases, `gh release delete <tag> --cleanup-tag`
and re-tag at the same SHA after the fix.

The build pipeline cosmetic fixes (e.g. tweaking the IsDevBuild detection
in build.ps1) MUST ride along with a genuine customer release; never tag
a release purely to test workflow plumbing.
