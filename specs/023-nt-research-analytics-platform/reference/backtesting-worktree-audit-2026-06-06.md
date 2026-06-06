# Backtesting Worktree Audit - 2026-06-06

This is a read-only audit of local/remote backtesting, converter, and backfill
branches after `main` reached `44655776`. It records disposition guidance only;
it does not delete worktrees or branches.

## Evidence Commands

- `git worktree list`
- `git branch -a --list '*438*' '*439*' '*541*' '*496*' '*backtesting*' '*backfill*' '*converter*' '*bte*'`
- For each listed branch: `git merge-base --is-ancestor <ref> main`,
  `git rev-list --count main..<ref>`, and `git diff --name-only main...<ref>`.
- Targeted `git status -sb` on the relevant local worktrees.
- GitHub PR state/comments for PR #541, #571, #578, #582, #588, #589, and #592.

## Disposition Summary

| Branch or worktree | Evidence | Disposition |
|---|---|---|
| `codex/backtesting-vertical-slice` / `541-bte-fixes` | Tip is contained in `main`; unique commits: 0; changed files: 0; remote is gone. | Superseded by merged PR #541. Delete local worktree/branch after this audit is reviewed. |
| `chore/bte-runtime-hardening` / `bte-runtime-hardening` | Tip is contained in `main`; unique commits: 0; changed files: 0; remote is gone. | Superseded by merged PR #588. Delete local worktree/branch after this audit is reviewed. |
| `feat/439-nt-venue-converters` / `backtesting-vertical-slice` | Not contained in `main`; unique commits: 25; changed files: 58; PR #571 is closed unmerged. Worktree has uncommitted changes in `Cargo.lock`, `Cargo.toml`, `canonical_book.rs`, and `canonical_book_catalog.rs`. | Preserve as reference only. Do not merge as-is. Replacement converter PR should start from current `main` with streaming, bounded writes, and idempotent per-object/member completion markers. |
| `feat/438-bte-ingest-loader` / `438-bte-gate1-proof` | Not contained in `main`; unique commits: 20; changed files: 25; worktree is dirty with many renames, root-crate BTE files, Python proof script, source-proof docs, and untracked acquisition/source-proof files. | Do not merge as-is. #541 supersedes the accepted minimal engine path; separately review only unique source-proof fixture/acquisition material before deleting. |
| `feat/438-catalog-projector-phase0` / `438-catalog-projector` | Not contained in `main`; unique commits: 1; changed files: 13 under `tools/catalog-projector/`; worktree is clean. | Unique small tool branch. Either open a fresh narrow PR from current `main` if still wanted, or delete after explicit decision. |
| `feat/438-bte-gate4-run-proof` / `496-bte-gate4-run-proof` | Not contained in `main`; unique commits: 16; changed files: 15; worktree is clean. | Likely superseded by #541 for engine proof concepts, but compare docs/tests before deletion. Do not merge as-is. |
| `feat/438-bte-gate1-backtest-proof` | Not contained in `main`; unique commits: 3; changed files: 5. | Likely superseded by #541 gate tests. Audit briefly before deleting. |
| `docs/438-normalization-catalog-plan` / `docs-438-normalization-catalog-plan` | Not contained in `main`; unique commits: 2; changed files: 11, all normalization/catalog planning docs and review syntheses; worktree is clean. | Preserve as planning reference for the replacement converter-boundary PR; do not silently merge all docs. |
| `feat/023-venue-data-backfill` / `oneoff-backfill` | Not contained in `main`; unique commits: 1; changed files: 46, mostly one-off `scripts/backfill_*`. Worktree is clean. | Do not merge. PR #582 intentionally kept reference docs and excluded one-off scripts. Delete after confirming no reviewed reference material is missing from #582. |
| `codex/oneoff-seven-token-backfill` | Not contained in `main`; unique commits: 2; changed files: 22, mostly one-off `scripts/backfill_*`. | Same disposition as `feat/023-venue-data-backfill`: reference only, not maintained tooling. |
| `origin/docs/backfill-source-proof-handoff` / PR #578 | Not contained in `main`; unique commits: 1; changed files: 17; PR #578 is closed as superseded by #582. | Leave closed. Do not reopen. Port nothing except deliberately reviewed reference material already represented in #582. |

## Immediate Next Actions

1. Delete only the contained merged worktrees/branches after review:
   `541-bte-fixes` / `codex/backtesting-vertical-slice` and
   `bte-runtime-hardening` / `chore/bte-runtime-hardening`.
2. Keep `feat/439-nt-venue-converters` as reference until a replacement
   converter-boundary PR ports only reviewed/proven pieces from fresh `main`.
3. Do a focused read-only pass over the old #438 branches for unique
   source-proof fixture/acquisition material; do not merge root-crate BTE/Python
   proof code as-is.
4. Decide separately whether `tools/catalog-projector/` deserves a fresh small
   PR or should be deleted.
5. Delete one-off backfill script branches only after confirming #582 contains
   all desired reference material.
