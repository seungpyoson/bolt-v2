# Handoff — Backtester CI always-present gate (issue #554)

- **Date:** 2026-06-03
- **Updated:** 2026-06-06
- **Original branch:** `chore/554-backtester-ci-gate` (stacked on `codex/backtesting-vertical-slice`, i.e. PR #541 head `f92fe0139d071afec36adadf7ed68b0db2df294d`)
- **Shipping branch:** `codex/backtesting-vertical-slice` / PR #541
- **Tracking issue:** #554
- **Final scope decision:** the backtester CI gate now ships in PR #541 because the workflow gates the new crate introduced by PR #541 and depends on that crate's root `justfile` recipes. This document records the original parked-branch context and the final fold-in decision.

## What happened

The operator asked me to "use a workflow to complete the incomplete job for the **backtesting engine**." I read "the engine" as the **CI plumbing** (this issue, #554) and ran a workflow that produced the CI edit on this branch. **The operator meant the functional engine** — converting raw S3 venue data into NautilusTrader-backtestable format. I had even flagged that ambiguity earlier in the session and then resolved it myself instead of confirming. Wrong target.

The original plan was to park this CI work outside #541. That is no longer the final state: the CI gate is part of #541's declared review surface, and the PR body must continue to name it explicitly. The converter remains separate follow-on work.

## What this branch contains

One edit to `.github/workflows/backtester-ci.yml`: replaces the **advisory + path-filtered** gate with an **always-present** gate.

- Removed `on.pull_request.paths` / `on.push.paths` — the workflow now triggers on every PR/push to `main`.
- Added a `detect` job: computes `bvs_changed` from `git diff --name-only <base>...HEAD` over the crate-relevant paths (the crate dir, the **root `justfile`**, `scripts/rust_verification.py`, `scripts/command_understanding.py`, `rust-toolchain.toml`, the `setup-environment` action, and the workflow file itself).
- `fmt` / `clippy` / `test` lanes `if: bvs_changed == 'true'` — they skip on PRs that don't touch the crate. `fmt` stays the fail-early gate; `clippy`/`test` still `needs: fmt`.
- `backtester-gate` is `if: always()`, `needs: [detect, fmt, clippy, test]`: **no-op success** when the crate is untouched, and **fails** if any lane `!= success` when the crate changed. So the `backtester-gate` context **always reports** — safe for the operator to mark required later without GitHub's path-filtered "frozen PR" trap.
- Root `justfile` added to the `detect` path-list **and** both `managed-target-bvs-v1-*` cache keys (it owns the `bte-*` recipes the lanes call).

### Validated

| Check | Result |
|---|---|
| `just ci-lint-workflow` | exit 0 |
| `actionlint .github/workflows/backtester-ci.yml` | exit 0 |
| `just bte-fmt-check` | exit 0 |
| `cargo tree -p bolt-v2 \| grep nautilus-backtest` | 0 matches (live binary isolation holds) |

The PR now also has GitHub Backtester CI green on the folded workflow.

## Resolved caveat in the gate logic

Earlier versions let `backtester-gate` no-op if `detect` failed before setting `bvs_changed`. PR #541 now makes `backtester-gate` fail closed when `needs.detect.result != success`, so marking only `backtester-gate` required no longer masks a detector failure.

## Remaining decisions

1. Whether `backtester-gate` becomes a **required** merge check, and via classic branch protection vs a ruleset. **No ruleset / branch-protection change has been made.** (Both Codex and Gemini confirmed GitHub ruleset "required workflows" ignore workflow `paths:` filters — which is why the gate was made always-present instead.)
2. Root `ci.yml` nextest cache still hashes `crates/**` (fix 3 in #554, deferred). `scripts/verify_ci_workflow_hygiene.py` enforces `crates/**` as a **required** substring in the test-archive cache key, so removing it trips the hygiene lint. Needs a verifier change first — do not just delete it.

## External review trail

Codex adversarial review + Gemini custom-review (against PR #541 head `f92fe013`) both flagged: root `justfile` missing from triggers/cache (fixed here) and the advisory gate being too weak (fixed here). GLM review could not be obtained (relay tooling bug — `seungpyoson/relay#208`, footgun `#209`).

## The REAL next task — the converter (NOT started)

Convert the raw S3 venue files into NT-backtestable format. The data goal the operator locked is the **full L2 order book**, not just trades (NT quote-driven strategies place ~0 orders on trade-only data).

- Conversion code lives in `crates/backtesting-vertical-slice/src/` — `catalog_projection.rs`, `canonical_trades.rs`, `source_proof.rs`, `runner.rs`. It looks **trade-focused** today (the known "trade-only" gap).
- Raw data is fed to S3 by `scripts/backfill_*_to_s3.py` (multiple venue families).
- **Investigate before implementing:** map what the converter does today (input format → output NT type), confirm whether it emits `OrderBookDelta`/`QuoteTick` or only `TradeTick`, then plan the L2 path. Do not start writing conversion until that map exists and the operator has confirmed scope.
