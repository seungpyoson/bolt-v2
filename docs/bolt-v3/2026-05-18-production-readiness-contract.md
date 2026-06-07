# Bolt-v3 Production Readiness Contract

Date: 2026-05-18

Status: Issue #369 checklist contract. This document defines claim levels and
required evidence. It is not approval to run live capital.

## Scope

One explicitly approved, capped live order attempt requires the current live
submit-admission evidence: reviewed head, root config checksum, approval
window, submit caps, strategy-input evidence, venue state evidence, and
post-decision order-intent/admission evidence. Issue #360 is a closed
historical tracker; issue closure is not evidence that any current live attempt
or production-level claim is satisfied. Production-grade live trading is a
separate claim and stays blocked until the production level below is satisfied
or explicitly waived by the operator in a tracked issue or PR.

Issue #409 tracks PortfolioSnapshot observability. Source-level capture and
verifier coverage can support that issue ledger, but they are not staged-live
or production-live readiness evidence by themselves.

The active readiness authority remains
`docs/bolt-v3/2026-04-28-source-grounded-status-map.md` for source-backed
implementation status and the current live-submit admission contract in source.

## Claim Levels

| Level | Claim Allowed | Required Evidence |
|---|---|---|
| staged live ready | Repeated operator-supervised live runs may be proposed for a configured stage window. This is not unattended production readiness. | Exact reviewed head, clean worktree, exact root TOML checksum, SSM manifest hash, strategy-input safety evidence, financial-envelope evidence, pre-run state evidence, abort-plan evidence, time-bound approval nonce, submit-admission caps, NT submit evidence, venue accept/fill/reject evidence, strategy cancel when an order remains open, NT-backed restart reconciliation, post-run hygiene, order-lifecycle tests, restart-reconciliation tests, single-runner protection tests, approval replay-resistance tests, monitoring/alerting proof, and deploy provenance for each run. |
| production live ready | Production-grade live trading claims may be made for the configured venue, market family, strategy, host, and root TOML. | All staged live evidence plus completed staged-run acceptance criteria, no open blocker in rows 34-48 of the source-grounded status map unless explicitly waived, documented operator runbooks exercised at least once, alert routing verified, deploy provenance tied to the reviewed commit and running binary, and explicit operator approval naming the exact scope. |

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
- strategy input evidence path hash and record hash
- financial envelope evidence path hash and record hash
- pre-run state evidence path hash and record hash
- NT submit, venue order state, optional strategy cancel, restart
  reconciliation, and post-run hygiene evidence hashes when live orders are in
  scope
- monitoring/alerting proof hash for staged live and production live claims
- explicit residual blockers and explicit waivers, if any

Evidence that contains raw secrets, private keys, raw approval ids, or account
balances is invalid for promotion.

The separate claims are staged live readiness and production live readiness.

## Runbooks

These runbooks are required before staged live readiness and must be linked from
the evidence package:

- Repeated-live operation: preflight exact head, root TOML, SSM manifest,
  stage caps, approval window, single-runner lock, launch, monitor, controlled
  stop, and evidence capture.
- Abort: trigger condition, operator action, runner stop, venue order-state
  verification, cancel evidence when needed, position/account reconciliation,
  and incident record.
- Restart recovery: start from the reviewed binary and root TOML, import
  venue-confirmed state through NT, prove no duplicate submit, reconcile
  order/fill/position/account state, and write restart-reconciliation evidence.
- Post-run hygiene: raw-secret residue scan, artifact retention proof, purge
  proof for non-retained artifacts, redaction verification, and issue/PR
  evidence links.

These runbooks define the repeatable operator procedure for staged live runs;
they do not replace submit-admission, venue-state, or kill-switch controls.

## Required Tests And Tooling

Existing local gates that can contribute evidence:

- `cargo test --test bolt_v3_dead_gate_removal -- --nocapture`
- `cargo test --test bolt_v3_submit_admission -- --nocapture`
- `cargo test --test bolt_v3_strategy_registration -- --nocapture`
- `cargo test --test bolt_v3_controlled_connect -- --nocapture`

Missing gates that block staged live and production live claims until implemented
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
execution gate, deploy trust, panic gate, CLOB V2 readiness, first live-order
readiness, and production live trading.

## Claim Block

Production-grade claims are blocked if any of these is true:

- the evidence package is missing, stale, unreviewed, or not exact-head
- a required proof links only to mock/local behavior when the claim needs real
  venue evidence
- a run used a different binary, root TOML, SSM manifest, host, approval id, or
  NT pin than the evidence package records
- the issue or PR says a broader level is satisfied than the evidence supports
- a closed issue, including #360, is cited as proof without the required
  redacted evidence package for the claimed level
- any required source-grounded status-map blocker remains open without an
  explicit operator waiver

The narrowest true claim wins. If the evidence proves only one supervised live
attempt, the allowed claim is that specific attempt completed, not production
readiness.
