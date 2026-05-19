# Implementation Plan: Phase 9 Current-main Audit

**Branch**: `022-bolt-v3-phase9-current-main-audit`
**Audit source anchor**: `23acab30b73990302765ea441550fabcbf03f570`
**Refreshed base**: `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`
**Mode**: audit plus approved remediation, no live-capital action

## Source Provenance

- `git fetch origin --prune` completed before worktree creation.
- `git rev-parse HEAD main origin/main` returned `23acab30b73990302765ea441550fabcbf03f570` for all three refs.
- `git log -1 --oneline --decorate` returned `23acab3 ... Merge pull request #328 from seungpyoson/020-bolt-v3-phase8-implementation`.
- Worktree: `.worktrees/022-bolt-v3-phase9-current-main-audit`.
- Final refresh merged current `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`; the only main deltas from the original anchor were `.github/workflows/stale.yml` and `.github/workflows/summary.yml`.

## Scope

### Included Surfaces

- `src/bolt_v3_*.rs`
- `src/bolt_v3_*/**/*.rs`
- `src/strategies/binary_oracle_edge_taker.rs`
- `src/clients/chainlink.rs`
- `src/clients/polymarket.rs`
- `src/platform/polymarket_catalog.rs`
- `src/live_config.rs`
- `src/config.rs`
- `src/validate.rs`
- `tests/bolt_v3*.rs`
- `tests/fixtures/bolt_v3/**`
- `scripts/verify_bolt_v3_*.py`
- `scripts/test_verify_bolt_v3_*.py`
- `docs/bolt-v3/**`
- `specs/001-thin-live-canary-path/**`
- `specs/002-phase7-no-submit-readiness/**`

### Excluded Actions

- No live capital.
- No soak.
- No merge.
- No runtime cleanup.
- No source-bearing external review without explicit approval-request evidence.
- No unapproved runtime code edit.

## Required Proof Commands

```bash
rg -n '"[^"]+"|[0-9]+' src/bolt_v3_*.rs src/bolt_v3_* src/strategies/binary_oracle_edge_taker.rs src/clients/chainlink.rs src/clients/polymarket.rs src/platform/polymarket_catalog.rs src/live_config.rs src/config.rs src/validate.rs > /private/tmp/bolt-v3-phase9-literal-coverage.txt
rg -n "polymarket|chainlink|venue|strategy|provider|market_family|admission|risk|default|fallback|bypass|hardcoded|TODO|FIXME|fix later|As an AI|language model|I'm sorry|apologize|unfortunate" src tests config docs specs > /private/tmp/bolt-v3-phase9-policy-coverage.txt
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_core_boundary.py
python3 scripts/verify_bolt_v3_naming.py
```

## Evidence Collected

- Literal coverage output: `/private/tmp/bolt-v3-phase9-literal-coverage.txt`, 7,525 lines.
- Policy coverage output: `/private/tmp/bolt-v3-phase9-policy-coverage.txt`, 7,186 lines after rerun with AI-slop markers included.
- Verifier inspection line counts:
  - `scripts/verify_bolt_v3_runtime_literals.py`: 499 lines.
  - `scripts/verify_bolt_v3_provider_leaks.py`: 900 lines.
  - `scripts/verify_bolt_v3_core_boundary.py`: 78 lines.
  - `scripts/verify_bolt_v3_naming.py`: 121 lines.
- Verifier results:
  - Runtime literal verifier: passed.
  - Provider-leak verifier: passed.
  - Core-boundary verifier: passed.
  - Canonical naming verifier: passed.
- Debt marker scan: `rg -n "TODO|FIXME|fix later|As an AI|language model|I'm sorry|apologize|unfortunate" src tests docs specs` found no active source/test TODO, FIXME, or AI-slop markers; remaining hits are recorded command lines plus one historical doc statement.

## Audit Method

1. Anchor current main and worktree.
2. Generate literal and policy scan outputs.
3. Inspect verifier scripts and run verifiers.
4. Inspect current roadmap/status docs against current source.
5. Classify runtime values by ownership category.
6. Severity-rank findings with line evidence.
7. Classify runtime-capture concern.
8. Produce cleanup candidates with behavior locks only.
9. Decide current live-action state.

## Decision Rule

The audit can recommend "ready tiny live order approval" only if all of these are true:

- Current-main docs and spec state agree with current source.
- No production runtime value remains as an unexplained code-owned residual.
- Runtime-capture failure behavior has an integrated regression test or is proven irrelevant.
- No-submit evidence and strategy/feed safety evidence are current-head and approval-backed.
- Live order gate, reconciliation, execution, deploy trust, and panic gate are proven.

Current main does not meet that rule. The Phase 9 decision is **blocked for tiny live order approval**.
