# External Review

**Feature**: `specs/026-nt-backed-iv-engine/`
**Review date**: 2026-06-10

## Status

External review and PR review comments were requested/reviewed on PR #611.

Relay model approvals from earlier heads are stale after the 2026-06-10 local fix delta. Fresh relay reviews must be requested only after that delta is committed, pushed, and exact-head GitHub CI is green.

Current PR review state:

- PR: `https://github.com/seungpyoson/bolt-v2/pull/611`
- CodeQL hard-coded cryptographic value review threads: resolved/outdated; CodeQL checks are green.
- Gemini review threads: replied to and resolved for strict-positive IV validation and optional operator expiration metadata.
- Owner replies are present on both Gemini threads.
- No unresolved review threads were found through the PR review-thread query.

## CI Status

PR #611 GitHub CI is the current verification source for the branch head. Passing checks to require are CI `gate`, `test`, all four nextest shards, `nextest archive`, `clippy`, `deny`, `build`, `check-aarch64`, `source-fence`, `fmt-check`, `detector`, CodeQL, actionlint, and Backtester CI. `deploy` and `same-sha-main-evidence` are expected skips.

## Rerun Status

External/PR review comments were resolved. Because any evidence-file commit changes the head SHA, final status must be confirmed with `gh pr view 611 --json headRefOid` and `gh pr checks 611` after the final push.
