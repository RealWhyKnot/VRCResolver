# Release-body templates

`Generate-ReleaseNotes.ps1` reads each `.md` file in this directory and emits its content as a section of the GitHub release body. The order is fixed: title, auto-changelog slice, file integrity, then `links.md`, `install.md`, `uninstall.md`, `what-you-need-to-do.md`, then optional extras from `.github/release-extras/<tag>.md`.

## Tokens

Each template runs through token substitution before the body composes. Any of these strings in a template gets replaced with the corresponding value at compose time:

| Token | Example value |
|---|---|
| `{tag}` | `v2026.5.5.4` |
| `{version}` | `2026.5.5.4` |
| `{owner}` | `RealWhyKnot` |
| `{repo}` | `VRCResolver` |
| `{full-repo}` | `RealWhyKnot/VRCResolver` |
| `{commit-sha}` | full 40-char hash of the tag's commit |
| `{commit-sha-short}` | first 12 chars of the same hash |
| `{prior-tag}` | `v2026.5.5.3` (empty on first release) |
| `{zip-name}` | `vrcresolver-v2026.5.5.4.zip` |

Tokens that the resolver could not compute render as the literal token string. This is intentional: a missing token is visible to a reader, who can then file an operator fix.

## Adding a new section

1. Create `<name>.md` in this directory.
2. Add the section to the section-order list in `.github/scripts/Generate-ReleaseNotes.ps1` (search for `Add-TemplateSection`).
3. Pick a slot in the composition order; ASCII-scrub gates run on the full composed body so any voice violation in the template fails the workflow at publish time.

## Adding a new token

1. Open `.github/scripts/Generate-ReleaseNotes.ps1`, find the `$tokens` hash table in `New-ReleaseBody`.
2. Add the new key.
3. Update the table above so other operators know the token exists.

## Editing existing templates

Templates are read verbatim and pass through the same scrub gates as commit subjects. Avoid marketing language, internal-tooling vocabulary, and any character outside printable ASCII. The list of forbidden patterns lives in `Generate-ReleaseNotes.ps1` near the bottom.

## Skipping a section

Delete or rename the corresponding `.md` file. The composer emits a `::warning::` to the workflow log when a template is missing but does not fail the build; the section just does not render in the body.

## Optional release-specific extras

Templates here are evergreen content, the same on every release. For one-off prose tied to a single release (a release-specific bug-fix narrative, a coordination note, etc.) put a markdown file at `.github/release-extras/<tag>.md`. The composer appends its content verbatim below the templated sections, separated by `---` and an `## Additional notes` heading.
