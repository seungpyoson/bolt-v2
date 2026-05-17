# Bolt-v3 Production Readiness Contract

Date: 2026-05-18

Status: Issue #369 checklist contract. This document defines claim levels and
evidence gates. It is not approval to run live capital.

## Scope

Issue #360 tiny-canary readiness is the proof for one explicitly approved,
capped live order attempt. Production-grade live trading is a separate claim and
stays blocked until the production level below is satisfied or explicitly
waived by the operator in a tracked issue or PR.

The active readiness authority remains:

- `specs/001-thin-live-canary-path/contracts/live-canary-gates.md` for the
  tiny-canary gate order and live proof inputs.
- `specs/002-phase7-no-submit-readiness/contracts/no-submit-readiness.md` for
  authenticated zero-order readiness.
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` for source-backed
  implementation status.

## Claim Levels

| Level | Claim Allowed | Required Evidence |
|---|---|---|
| Tiny-canary ready | One approved capped canary attempt may enter the live runner. This is not repeated-live or production readiness. | Exact reviewed head, clean worktree, exact root TOML checksum, SSM manifest hash, satisfied no-submit readiness report accepted by the live-canary gate, strategy-input safety evidence, financial-envelope evidence, pre-run state evidence, abort-plan evidence, time-bound approval nonce, submit-admission caps, and local gate tests. |
| Staged live ready | Repeated operator-supervised live runs may be proposed for a configured stage window. This is not unattended production readiness. | All tiny-canary evidence plus completed canary evidence for NT submit, venue accept/fill/reject, strategy cancel when an order remains open, NT-backed restart reconciliation, post-run hygiene, order-lifecycle tests, restart-reconciliation tests, single-runner protection tests, approval replay-resistance tests, monitoring/alerting proof, and deploy provenance for each run. |
| Production live ready | Production-grade live trading claims may be made for the configured venue, market family, strategy, host, and root TOML. | All staged-live evidence plus completed staged-run acceptance criteria, no open blocker in rows 34-48 of the source-grounded status map unless explicitly waived, documented operator runbooks exercised at least once, alert routing verified, deploy provenance tied to the reviewed commit and running binary, and explicit operator approval naming the exact scope. |

Any claim must name the level. A PR, issue, or runbook must not say "production
ready", "live ready", or "ready for trading" without naming the level and
linking the evidence package.

## Evidence Package

Every promotion between levels requires a redacted evidence package with these
fields or links:

- reviewed commit SHA and PR or issue reference
- root TOML path and SHA-256 checksum
- binary build provenance, including source commit and local or CI build record
- host identity proof and service policy proof
- SSM manifest path hash and manifest record hash, with no secret values
- operator approval id hash, approval time window, nonce path hash, and
  consumption record hash
- no-submit readiness report path hash and report record hash
- strategy input evidence path hash and record hash
- financial envelope evidence path hash and record hash
- pre-run state evidence path hash and record hash
- NT submit, venue order state, optional strategy cancel, restart
  reconciliation, and post-run hygiene evidence hashes when live orders are in
  scope
- monitoring/alerting proof hash for staged-live and production-live claims
- explicit residual blockers and explicit waivers, if any

Evidence that contains raw secrets, private keys, raw approval ids, or account
balances is invalid for promotion.

## Runbooks

These runbooks are required before staged-live readiness and must be linked from
the evidence package:

- Repeated-live operation: preflight exact head, root TOML, SSM manifest,
  no-submit report, stage caps, approval window, single-runner lock, launch,
  monitor, controlled stop, and evidence capture.
- Abort: trigger condition, operator action, runner stop, venue order-state
  verification, cancel evidence when needed, position/account reconciliation,
  and incident record.
- Restart recovery: start from the reviewed binary and root TOML, import
  venue-confirmed state through NT, prove no duplicate submit, reconcile
  order/fill/position/account state, and write restart-reconciliation evidence.
- Post-run hygiene: raw-secret residue scan, artifact retention proof, purge
  proof for non-retained artifacts, redaction verification, and issue/PR
  evidence links.

The Phase 8 quickstart already names the tiny-canary artifacts. These runbooks
extend that one-canary path into repeatable operator procedure; they do not
replace the live-canary gate.

## Required Tests And Tooling

Existing local gates that can contribute evidence:

- `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`
- `cargo test --test bolt_v3_live_canary_gate -- --nocapture`
- `cargo test --test bolt_v3_submit_admission -- --nocapture`
- `cargo test --test bolt_v3_controlled_connect -- --nocapture`
- `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`
- `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`

Missing gates that block staged-live and production-live claims until implemented
or explicitly waived:

- order lifecycle proof from NT order/execution events for submit, accept,
  fill, reject, cancel, and terminal state
- restart reconciliation proof from NT and venue-confirmed state, including
  no-duplicate-submit behavior after restart
- single-runner protection proof covering concurrent starts and stale locks
- approval replay-resistance proof covering nonce reuse, time-window rejection,
  and mismatched head/config/manifest rejection
- monitoring and alerting proof for runner liveness, venue connectivity,
  order-state stalls, reconciliation mismatch, rejected/cancelled orders,
  error budget breach, and evidence-writer failure
- deploy provenance proof tying reviewed commit, built binary, host, root TOML,
  SSM manifest, approval artifact, NT pin, and CI run

Rows 34-48 of `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` remain the
source-backed blocker list for order lifecycle, reconciliation, observability,
execution gate, deploy trust, panic gate, CLOB V2 readiness, tiny canary, and
production live trading.

## Claim Block

Production-grade claims are blocked if any of these is true:

- the evidence package is missing, stale, unreviewed, or not exact-head
- a required proof links only to mock/local behavior when the claim needs real
  venue evidence
- a run used a different binary, root TOML, SSM manifest, host, approval id, or
  NT pin than the evidence package records
- the issue or PR says a broader level is satisfied than the evidence supports
- any required source-grounded status-map blocker remains open without an
  explicit operator waiver

The narrowest true claim wins. If the evidence proves one tiny canary, the
allowed claim is "tiny-canary ready" or "tiny canary completed", not production
readiness.
