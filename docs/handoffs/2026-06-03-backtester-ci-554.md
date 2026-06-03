# Handoff — Backtester CI always-present gate (issue #554)

- **Date:** 2026-06-03
- **Branch:** `chore/554-backtester-ci-gate` (stacked on `codex/backtesting-vertical-slice`, i.e. PR #541 head `f92fe0139d071afec36adadf7ed68b0db2df294d`)
- **Tracking issue:** #554
- **Why this branch exists:** to keep the CI redesign OUT of PR #541 and parked with a record, per the operator's instruction. PR #541's branch is untouched.

## What happened (read this first — it's a scope mistake)

The operator asked me to "use a workflow to complete the incomplete job for the **backtesting engine**." I read "the engine" as the **CI plumbing** (this issue, #554) and ran a workflow that produced the CI edit on this branch. **The operator meant the functional engine** — converting raw S3 venue data into NautilusTrader-backtestable format. I had even flagged that ambiguity earlier in the session and then resolved it myself instead of confirming. Wrong target.

So: this CI work is correct and validated, but it was **not** what was asked for at that moment. It is parked here, not committed into #541. **The real next task is the converter (see bottom).**

## What this branch contains

One edit to `.github/workflows/backtester-ci.yml`: replaces the **advisory + path-filtered** gate with an **always-present** gate.

- Removed `on.pull_request.paths` / `on.push.paths` — the workflow now triggers on every PR/push to `main`.
- Added a `detect` job: computes `bvs_changed` from `git diff --name-only <base>...HEAD` over the crate-relevant paths (the crate dir, the **root `justfile`**, `scripts/rust_verification.py`, `scripts/command_understanding.py`, `rust-toolchain.toml`, the `setup-environment` action, and the workflow file itself).
- `fmt` / `clippy` / `test` lanes `if: bvs_changed == 'true'` — they skip on PRs that don't touch the crate. `fmt` stays the fail-early gate; `clippy`/`test` still `needs: fmt`.
- `backtester-gate` is `if: always()`, `needs: [detect, fmt, clippy, test]`: **no-op success** when the crate is untouched, and **fails** if any lane `!= success` when the crate changed. So the `backtester-gate` context **always reports** — safe for the operator to mark required later without GitHub's path-filtered "frozen PR" trap.
- Root `justfile` added to the `detect` path-list **and** both `managed-target-bvs-v1-*` cache keys (it owns the `bte-*` recipes the lanes call).

### Validated — but LOCALLY only (not yet on a real GitHub run)

| Check | Result |
|---|---|
| `just ci-lint-workflow` | exit 0 |
| `actionlint .github/workflows/backtester-ci.yml` | exit 0 |
| `just bte-fmt-check` | exit 0 |
| `cargo tree -p bolt-v2 \| grep nautilus-backtest` | 0 matches (live binary isolation holds) |

The previous green Backtester CI run was the OLD path-filtered version. The new gate logic has **not** been exercised on a real GitHub run yet.

## Known caveat in the gate logic (fail-open detector)

If the `detect` job itself fails or is cancelled, `bvs_changed` is empty, the gate's guard sees `!= "true"` and exits 0 (green no-op). If the operator later marks **only** `backtester-gate` required (not `bvs-detect`), a `detect` failure would be masked. **Mitigation:** mark `bvs-detect` required alongside `backtester-gate`, or accept that a `detect` failure shows up as its own red check on the PR.

## Open decisions (operator's call — do NOT pre-decide)

1. Whether `backtester-gate` becomes a **required** merge check, and via classic branch protection vs a ruleset. **No ruleset / branch-protection change has been made.** (Both Codex and Gemini confirmed GitHub ruleset "required workflows" ignore workflow `paths:` filters — which is why the gate was made always-present instead.)
2. Whether the CI ships as its **own PR** or folds back into #541. The workflow file originates in #541, and the gate depends on #541's crate + root `justfile` `bte-*` recipes + `setup-environment` action — so a truly `main`-based standalone CI PR would be a non-functional fragment until #541 lands. That is why this branch is stacked on #541, not branched off `main`.
3. Root `ci.yml` nextest cache still hashes `crates/**` (fix 3 in #554, deferred). `scripts/verify_ci_workflow_hygiene.py` enforces `crates/**` as a **required** substring in the test-archive cache key, so removing it trips the hygiene lint. Needs a verifier change first — do not just delete it.

## External review trail

Codex adversarial review + Gemini custom-review (against PR #541 head `f92fe013`) both flagged: root `justfile` missing from triggers/cache (fixed here) and the advisory gate being too weak (fixed here). GLM review could not be obtained (relay tooling bug — `seungpyoson/relay#208`, footgun `#209`).

## The REAL next task — the converter (NOT started)

Convert the raw S3 venue files into NT-backtestable format. The data goal the operator locked is the **full L2 order book**, not just trades (NT quote-driven strategies place ~0 orders on trade-only data).

- Conversion code lives in `crates/backtesting-vertical-slice/src/` — `catalog_projection.rs`, `canonical_trades.rs`, `source_proof.rs`, `runner.rs`. It looks **trade-focused** today (the known "trade-only" gap).
- Raw data is fed to S3 by `scripts/backfill_*_to_s3.py` (multiple venue families).
- **Investigate before implementing:** map what the converter does today (input format → output NT type), confirm whether it emits `OrderBookDelta`/`QuoteTick` or only `TradeTick`, then plan the L2 path. Do not start writing conversion until that map exists and the operator has confirmed scope.
