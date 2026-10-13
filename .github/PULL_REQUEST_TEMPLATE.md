<!-- Linked issue (optional but appreciated): Closes #__ -->

## Summary

<!-- 1-3 sentences on the *why*, not just the what. The diff already shows the what. -->

## Checklist

- [ ] `cargo test --workspace` and `scripts/lint.ps1` pass locally.
- [ ] If behaviour changed, a test was added or updated to cover it.
- [ ] Ran `powershell -File build.ps1` end-to-end at least once on this branch.
- [ ] Commit subjects pass `.githooks/commit-msg` (no duplicate `(YYYY.M.D.N-XXXX)` build-version stamps).

## Notes for relay / strategy / host changes

<!-- Delete this section if irrelevant. Otherwise: -->
<!-- - Did you change a host's relay-wrap behaviour? Mention the host. -->
<!-- - Did you add a host to the AVPro native-UA deny-list? Why? -->
<!-- - Did you add or change a resolution strategy? Note the tier. -->
