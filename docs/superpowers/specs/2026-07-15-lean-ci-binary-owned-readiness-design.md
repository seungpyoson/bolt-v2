# Lean CI and Binary-Owned Readiness Decision

Status: approved architecture; implementation and live cutover remain blocked by the program ledger.

## Provenance

- Approved decision packet: `/private/tmp/1016-lean-ci-decision-packet-r2.md`, SHA-256 `b2d6a5c9952078c695c2cff54352c1dbec8813974ca3469b6e6515730e3651db`.
- Approved implementation plan: `docs/superpowers/plans/2026-07-15-lean-ci-binary-owned-readiness.md`, source SHA-256 `dda7e936f9070aaa550e4bb5f6f64f0e760947b12046fe47f4e87f4794615ad1`.
- External resolution review: Claude Code CLI (Anthropic), model `claude-fable-5`, conclusion `APPROVE` on 2026-07-15.
- Architecture issue: #1016. Implementation and deletion ownership is recorded separately in `docs/ci/1016-lean-ci-program-ledger.md`.

## Decision

The selected end state has zero required CI status contexts. CI is visible evidence, not merge authority. Native GitHub human controls remain the merge authority: code-owner approval, stale-review dismissal, last-push approval, and human review-thread resolution.

The single informational Rust workflow is `trading-binary`. It runs only after a push to `main` or by manual dispatch, follows one unconditional locked-test and ARM64-release-build path, and executes the exact produced file for its positive and fail-closed evidence. Its result cannot authorize or veto merge, installation, launch, or trading.

The repository explicitly accepts that an approved merge can temporarily leave `main` red or broken. That is repository risk only. It is never deployment or trading permission.

## Operational Authority

An operator selects one manifest that identifies an exact `main` commit, artifact SHA-256, and config-bundle SHA-256. The single installer verifies those bytes and places the executable at the configured content-addressed immutable path. The systemd unit invokes that exact executable's `ops launch`.

`ops launch` owns one finite in-process pre-arm phase. It verifies executable identity, config-bundle identity, a nonempty enforceable deploy target, SSM-only secrets, storage/prestart state, reference-price health, and shared admission/runtime construction. Only complete success constructs an opaque, non-serializable, non-cloneable, one-use Rust `LiveReadinessPermit`. The sole Start entrypoint consumes that permit by value immediately, with no await, publication, or other fallible authorization step in between.

Installation is inert. A log or persisted receipt is audit-only and is never read as authority. Every systemd start or restart reruns the complete phase and obtains a fresh in-memory permit.

## Single-Path Boundary

The retained design has:

- one static informational `trading-binary` path;
- one manifest-bound, content-addressed immutable installer;
- one systemd entrypoint invoking the exact installed executable's `ops launch`;
- one in-process permit-consuming Start boundary; and
- one mechanical merge-queue operator path after its CI verdict machinery is retired.

There is no fallback, compatibility adapter, inherited result, alternate installer, mutable-copy route, tag or same-SHA substitution, prior-run artifact selection, cache-as-proof, persisted authority, external readiness publisher, trusted merge publisher, or dual path. Failure or ambiguity pauses merge, deploy, or trading as appropriate and receives a forward fix; it does not restore retired authority.

## Supersession

The trusted-App/protected-base verifier, precursor, activation, freeze, merge-protection ceremony, replay/tombstone control plane, and App-qualified merge authority previously proposed for #1016 are superseded. The historical 36,704–38,043-line rehearsal must not merge. Independently useful binary or runtime work may proceed only as a newly reviewed, issue-owned slice that stands on its own evidence and contains no control-plane authority or compatibility route.

#1016 owns this architecture and its acceptance criteria only. It does not own or automatically complete Program-A deletion issues, operational replacement, queue reduction, runtime-invariant migration, or broad-debt deletion.

## Migration Boundary

This decision does not change today's merge controls. Until the governed Task 7 cutover is complete and live state is re-verified, the existing required statuses, both Mergify `check-success` predicate sets, exact-head review requirements, queue preflight, and verifier behavior remain authoritative.

Migration order is fixed:

1. approve governance and assign issue ownership;
2. add exact-binary evidence under legacy gates;
3. migrate genuine runtime invariants and prove the candidate immutable install/pre-arm path under legacy gates;
4. retire legacy deploy authority, queue CI authorization, advisory-review blocking capability, and multi-PR batching while legacy status gates still remain;
5. re-prove native controls and the complete acceptance matrix, then perform the governed zero-status cutover; and
6. delete broad non-authoritative debt in issue-owned waves and measure the result.

No implementation branch starts for a ledger row marked `ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED`.

## Acceptance

The binding acceptance matrix is M1–M3, B1–B3, L1–L3, S1–S3, and X1–X3 in the approved plan. In particular:

- M1–M3 prove zero-status merge governance, retained native human controls, single-PR queues, and the accepted red-`main` risk.
- B1–B3 prove unconditional exact-binary evidence with no authorization edge.
- L1–L3 prove manifest/config/target binding and the one-use in-process permit boundary.
- S1–S3 prove one path, one policy owner, and complete deletion without re-encoding CI debt.
- X1–X3 prove safe sequencing, issue-owned deletion, honest pause semantics, no rehearsal merge, and measurement.

Missing, failed, stale, timed-out, cancelled, skipped, unknown, ambiguous, or substituted operational evidence must leave Start unreachable.
