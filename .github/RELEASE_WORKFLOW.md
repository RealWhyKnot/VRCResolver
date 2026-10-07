# Release workflow

Releases are tag-driven. Pushing a `v*` tag runs [release.yml](workflows/release.yml), which
builds the dist, publishes a GitHub release and checks that the published body matches what it
sent. The body is generated from `git log` between the previous release base and the new tag, and
there's no way to hand-write it. A stable tag uses the previous stable tag as its base, which pulls
every beta's changes since then into the stable notes. Beta and dev tags use the nearest previous
tag. For anything the generator can't produce, use the [extras file](#extras-file).

The workflow calls `build.ps1 -Package`. Local builds don't create a repo-root `release/` folder.
The zip, build manifest and release integrity TSV go under `dist/` for the workflow to upload.

## Tag shapes

| Form | When | Example |
|---|---|---|
| `vYYYY.M.D.N` | Release. `.N` is the release iteration for that calendar day, starting at 0. | `v2026.5.5.0` |
| `vYYYY.M.D.N-beta` | Prerelease. For beta builds that shouldn't become the latest stable release. | `v2026.5.5.0-beta` |
| `vYYYY.M.D.N-XXXX` | Dev. `.N` is the local build count and `XXXX` a 4-hex ID. Rare on the release stream. | `v2026.5.5.0-A1B2` |

[build.ps1](../build.ps1) checks the tag format against
`^\d{4}\.\d+\.\d+\.\d+(-([A-Fa-f0-9]{4}|beta))?$` and fails straight away on a malformed tag.

## Body composition

The release body is exactly what [Generate-ReleaseNotes.ps1](scripts/Generate-ReleaseNotes.ps1)
prints. It points readers at `vrcresolver-v<version>.integrity.tsv`, and the checksums themselves
are in that TSV asset. Layout:

```
# vrcresolver <tag>

## What's Changed

### Features
- feat(...): subject by [author](https://github.com/author) in <sha>

### Bug Fixes
- fix(...): subject by [author](https://github.com/author) in <sha>

**Full Changelog**: <compare-url>

## File integrity

Full SHA256 hashes are attached as `<integrity TSV>`.

[Additional notes (extras file, if present)]
```

Stable release bodies skip beta tags when they pick the compare base. If `v2026.5.5.0-beta` came
out after `v2026.5.1.0`, then `v2026.5.7.0` compares from `v2026.5.1.0` and the beta's changes
appear in the stable notes too. There's no curated `## Unreleased` excerpt above the generated
section. CHANGELOG.md is the history you browse in the repo.

## Conventional-commit policy

Commits in the tag range are grouped by the prefix on their subject line:

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

A subject starting with `<prefix>!:` (the breaking-change marker) goes in the same section.
Trailing version stamps like ` (2026.5.5.0-A1B2)` are stripped before grouping. Commits with
`[skip changelog]` in the subject are left out, and `--no-merges` leaves out merge commits.

The generator writes a workflow warning for each subject without a known prefix, in case you want
to amend it. Those commits go under `Other Changes` and the build still passes.

## Author credit

Each commit is credited as a link to the author's GitHub profile. The login comes from the commits
API. When the API has no login for a commit, the generator uses the git author name and maps it
through `$AuthorHandleMap`. The local git config uses the name "WhyKnot" and the GitHub login is
"RealWhyKnot", and that map handles the difference. Bot accounts are named in plain text. A new
author with a different git name needs a map entry or their credit links to the wrong profile.

## Scrub gates

After the body is put together, three gates run before the workflow continues:

1. ASCII normalisation. A fixed table of common typographic characters (em dash, en dash, ellipsis,
   smart quotes, NBSP, bullet, multiplication sign, arrows, section sign, pilcrow) is replaced with
   ASCII equivalents. The replacement is one-way and isn't logged.

2. Non-ASCII check. Anything left outside printable ASCII (0x20-0x7E plus tab) fails the script
   with the line, column and Unicode code point. Amend the commit subject (or extras file) to use
   ASCII, or add the character to the substitution table in the generator.

3. Wording check. A list of patterns is matched case-insensitively against the body: marketing
   words like `comprehensive` and `leveraging`, internal process words that don't belong in public
   release notes, and effort claims like "months of effort". A match fails the script with the
   pattern and its position. Amend the commit subject (or extras file) to use plainer words, or
   mark the commit `[skip changelog]` if the term can't be avoided, for example a refactor of an
   internal class whose name trips a pattern.

The patterns are in `$forbiddenPatterns` in
[Generate-ReleaseNotes.ps1](scripts/Generate-ReleaseNotes.ps1). Add to it when a new problem word
turns up.

## Empty ranges and the first release

If the tag range has no commits left to list (everything was `[skip changelog]`, the previous-tag
detection was wrong, or the tag was pushed from an empty branch), the script throws and the
workflow fails.

The first release on a repo has no previous tag. Its notes come from the tag's section of
`CHANGELOG.md`, or from `## Unreleased`. With neither, the script throws unless you pass
`-AllowEmpty`, which writes a one-line stub you can replace later with `gh release edit`.

## Extras file

For content the generator can't produce, like server-side coordination notes, migration
instructions or operational context, create a Markdown file at `.github/release-extras/<tag>.md`
before pushing the tag. Its contents are appended as-is below the generated section, after a `---`
separator and an `## Additional notes` heading.

```
.github/release-extras/v2026.5.5.5.md   <- created before tag push
```

The scrub gates check the extras text too. An em dash or the word `comprehensive` in an extras
file fails the workflow just like it would in a commit subject.

The file is optional and most releases don't need one. If you're writing one for every release,
that content probably belongs in a commit subject.

## Post-publish verification

After `gh release create` succeeds, the workflow fetches the published body with
`gh release view --json body` and compares it byte for byte (after normalising line endings) with
what it sent. If they differ:

1. It logs a warning and runs `gh release edit --notes-file <input>` to correct it.
2. It fetches the body again to confirm the correction took.
3. If they still differ, the workflow fails and keeps both files in the runner temp dir for
   inspection.

GitHub-side normalisation differences are rare. This step stops a malformed body from staying on a
release unnoticed.

## Changelog promotion

Before the build, the workflow promotes `## Unreleased` to the tagged section. The `CHANGELOG.md`
inside the build then matches the release. It also replays the commit range since the previous
stable tag into `## Unreleased` first, for when the tag job starts before the push-triggered
changelog appender has committed back to main.

Once the release is out, the promoted `CHANGELOG.md` goes back to main through GitHub's
`createCommitOnBranch` mutation. `changelog-append.yml` uses the same verified-commit path. It
replaced the old promotion PR.

## Failure modes

| Symptom | Fix |
|---|---|
| `No commits found in range` | Check that the tag's parent is reachable. Either the previous-tag detection failed (push the real previous tag) or every commit is `[skip changelog]` (push a real change before tagging). |
| `Non-ASCII characters in release body after normalisation` | Find the commit subject, amend it to use ASCII and force-push the tag at the new SHA. Or add the character to `$asciiSubs` in the generator. |
| `voice or internal-only-vocabulary patterns in release body` | Amend the commit subject. Or mark it `[skip changelog]` if the term can't be avoided. |
| `Generate-ReleaseNotes.ps1 returned empty output` | The script failed without an error or the slice was empty. Check the workflow log for warnings. An empty slice would already have thrown, so this one is a script bug. |
| `Release body still differs after auto-correct` | A GitHub-side issue. Compare the input file in the runner artifacts with what `gh release view` returns. It's often trailing whitespace or a Unicode normalisation difference. |
| `createCommitOnBranch returned GraphQL errors` | Main moved after the workflow read its head, or GitHub rejected the file update. Re-run the workflow once main stops moving. If it happens again, read the GraphQL error text and push the same CHANGELOG.md promotion with the next source commit. |

## Updating the workflow

The workflow and scripts are versioned with the code, and changes go through the same PR or
direct-to-main flow as anything else. The next real release is the first run of a workflow change.
If the workflow breaks mid-release, the tag is already pushed and gh's partial state may need
cleaning up. In the worst case, run `gh release delete <tag> --cleanup-tag` and re-tag the same SHA
after the fix.

Cosmetic build pipeline fixes, like tweaking the IsDevBuild detection in build.ps1, go out with a
real release. Never tag a release only to test the workflow.
