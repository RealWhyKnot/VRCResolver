# Release-body templates

`Generate-ReleaseNotes.ps1` turns these `.md` files into sections of the GitHub release body. The order is fixed: title, the changelog slice, file integrity, then `links.md`, `install.md`, `uninstall.md` and `what-you-need-to-do.md`, then any extras from `.github/release-extras/<tag>.md`.

## Tokens

These strings in a template are replaced with their values before the body is put together:

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

A token the generator can't work out becomes an empty string. Misspell one and it's not in the table, which leaves it in the release exactly as typed.

## Adding a new section

1. Create `<name>.md` in this folder.
2. Add the name to `$templateOrder` in `.github/scripts/Generate-ReleaseNotes.ps1`, in the position you want it.
3. The ASCII and wording checks run over the full body. A template that fails them fails the release workflow.

## Adding a new token

1. In `.github/scripts/Generate-ReleaseNotes.ps1`, find the `$tokens` hash table.
2. Add the new key.
3. Add it to the table above.

## Editing existing templates

Templates are read as-is and go through the same checks as commit subjects. Avoid marketing words, internal tooling terms and anything outside printable ASCII. The rejected patterns are in `$forbiddenPatterns` near the bottom of `Generate-ReleaseNotes.ps1`.

## Skipping a section

Delete or rename its `.md` file. The generator writes a `::warning::` to the workflow log and leaves that section out of the body. The build still passes.

## Release-specific extras

Templates here are the same on every release. For prose about one release, like the story behind a particular fix, put a Markdown file at `.github/release-extras/<tag>.md`. Its content is appended below the template sections, after `---` and an `## Additional notes` heading.
