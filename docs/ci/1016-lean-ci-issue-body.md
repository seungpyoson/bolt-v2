# Proposed Replacement Body for Issue 1016

Prepared from the approved 2026-07-15 architecture. This repository artifact does not mutate the live issue. The exact replacement body is the content between the markers, excluding the markers themselves.

<!-- BEGIN EXACT ISSUE BODY -->
# Lean CI architecture: visible binary evidence and binary-owned live readiness

## Status

Architecture approved. Implementation, GitHub/Mergify mutation, operational cutover, deployment, launch, and trading remain blocked by their own issue-owned evidence and authorization.

Durable sources:

- decision: `docs/superpowers/specs/2026-07-15-lean-ci-binary-owned-readiness-design.md`
- implementation plan: `docs/superpowers/plans/2026-07-15-lean-ci-binary-owned-readiness.md`
- two-board ownership ledger: `docs/ci/1016-lean-ci-program-ledger.md`

## Decision

The selected end state has zero required CI status contexts. CI is visible evidence, not merge authority. Native code-owner approval, stale-review dismissal, last-push approval, and human review-thread resolution remain mandatory.

One informational workflow, `trading-binary`, runs only after a push to `main` or by manual dispatch. Every invocation follows one unconditional locked-test and ARM64-release-build path and executes the exact produced file for positive and fail-closed evidence. It cannot authorize or veto merge, install, launch, or trading.

An approved merge may temporarily leave `main` red or broken. That is explicitly accepted repository risk. It is never deployment or trading permission.

## Live authority

An operator selects one manifest identifying the exact `main` commit, artifact SHA-256, and config-bundle SHA-256. The single installer verifies those bytes and places them at the configured content-addressed immutable path. The systemd unit invokes that exact executable's `ops launch`.

The actual Rust process owns one finite in-process pre-arm phase. It verifies executable and config identity, a nonempty enforceable deploy target, SSM-only secrets, storage/prestart state, reference-price health, and shared admission/runtime construction. Only complete success constructs an opaque, non-serializable, non-cloneable, one-use `LiveReadinessPermit`; the sole Start entrypoint consumes it by value immediately.

Installation and persisted receipts are inert. Every systemd start or restart reruns the complete phase and obtains a fresh in-memory permit. Missing, failed, stale, timed-out, cancelled, skipped, unknown, ambiguous, or substituted inputs leave Start unreachable.

## Supersession and single path

This architecture supersedes the trusted-App/protected-base verifier, precursor, activation, freeze, merge-protection ceremony, replay/tombstone control plane, and App-qualified merge authority previously proposed here. The historical trusted-control-plane rehearsal must not merge. Independently useful binary/runtime work requires a new bounded review with no retained control-plane authority.

There is no fallback, compatibility adapter, inherited result, alternate installer, mutable-copy route, tag or same-SHA substitution, prior-run artifact selection, cache-as-proof, persisted authority, external readiness publisher, or dual path. Rollback is a pause or forward fix, never restoration of retired authority.

## Ownership boundary

This issue owns the architecture, the 15 acceptance criteria (M1–M3, B1–B3, L1–L3, S1–S3, X1–X3), and their cross-program traceability.

This issue does not own operational replacement, queue reduction, advisory-review changes, live ruleset or Mergify changes, runtime-invariant migrations, broad CI/Python deletion waves, deployment, launch, or trading. Those rows require their own current issue body, implementer, evidence, internal adversarial review, and native human review. Applying this body does not complete or auto-complete any Program-A deletion issue.

## Migration boundary

This issue-body amendment does not relax current controls. Until the governed Task 7 cutover is complete and live state is re-verified, the existing required statuses, both Mergify `check-success` predicate sets, exact-head review requirements, queue preflight, and verifier behavior remain authoritative.

Migration order is governance first; exact-binary and candidate live-readiness proof under legacy gates; operational and queue-authority retirement while legacy gates remain; governed zero-status cutover; then issue-owned broad-debt deletion and measurement.

No implementation branch starts for a ledger row marked `ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED`.
<!-- END EXACT ISSUE BODY -->
