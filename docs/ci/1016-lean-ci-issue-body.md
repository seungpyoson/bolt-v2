# Exact Replacement Body for Live Issue 1016

Prepared from the approved and reconciled 2026-07-15 architecture. This repository artifact is intended as the exact live #1016 body once an operator applies it; committing the artifact does not mutate the live issue. The exact replacement body is the content between the markers, excluding the markers themselves.

<!-- BEGIN EXACT ISSUE BODY -->
# Lean CI architecture: visible binary evidence and binary-owned live readiness

## Status

Architecture approved. Implementation, GitHub/Mergify mutation, operational cutover, deployment, launch, and trading remain blocked by their own issue-owned evidence and authorization.

Durable sources:

- authoritative decision and remote Rust evidence contract: `docs/superpowers/specs/2026-07-15-lean-ci-binary-owned-readiness-design.md`
- implementation plan: `docs/superpowers/plans/2026-07-15-lean-ci-binary-owned-readiness.md`
- two-board ownership ledger: `docs/ci/1016-lean-ci-program-ledger.md`

## Decision

The selected end state has zero required CI status contexts. CI is visible evidence, not merge authority. Native code-owner approval, stale-review dismissal, last-push approval, and human review-thread resolution remain mandatory.

Heavy Rust verification is explicit, exact-SHA, workspace-and-target-specific only. No heavy Rust run starts automatically on a pull request, merge group, or `main` push. One public targeted-probe path and one explicit trusted-producer path replace the superseded every-`main`-push/full-nextest `trading-binary` contract. Neither can authorize or veto merge, install, launch, or trading.

An approved merge may temporarily leave `main` red or broken. That is explicitly accepted repository risk. It is never deployment or trading permission.

## Remote Rust evidence

Root/trading-binary and Backtester are separate workspaces. Root-only work does not wake Backtester; Backtester-only and documentation-only work consume zero root Cargo minutes; root binary-only work consumes zero Backtester Cargo minutes. A proven root path-dependency change selects separately authorized explicit targets in both workspaces. Routing returns a reviewed finite target set or `UNCLASSIFIED` with no command.

The read-only targeted probe executes one exact configured target/test command. The producer may seed one exact configured root or Backtester host target. Its `root-artifact` operation performs one locked ARM64 release build and positive/fail-closed execution of those exact bytes; it contains no locked/full or targeted Cargo test suite.

One pinned sccache-wrapped compiler path is mandatory and cache is acceleration only. There is no direct/uncached retry, archive restore, prior-result reuse, carry-forward, sidecar, aggregate fallback, scheduler, or alternate public path. Test-target splitting is driven by measured compile latency and fan-in in both workspaces; file, line, module, and branch counts are diagnostic only.

## Live authority

An operator selects one manifest identifying the exact `main` commit, artifact SHA-256, and config-bundle SHA-256. The single installer verifies those bytes and places them at the configured content-addressed immutable path. The systemd unit invokes that exact executable's `ops launch`.

The actual Rust process owns one finite in-process pre-arm phase. It verifies executable and config identity, a nonempty enforceable deploy target, SSM-only secrets, storage/prestart state, reference-price health, and shared admission/runtime construction. Only complete success constructs an opaque, non-serializable, non-cloneable, one-use `LiveReadinessPermit`; the sole Start entrypoint consumes it by value immediately.

Installation and persisted receipts are inert. Every systemd start or restart reruns the complete phase and obtains a fresh in-memory permit. Missing, failed, stale, timed-out, cancelled, skipped, unknown, ambiguous, or substituted inputs leave Start unreachable.

## Supersession and single path

This architecture supersedes the trusted-App/protected-base verifier, precursor, activation, freeze, merge-protection ceremony, replay/tombstone control plane, App-qualified merge authority, and automatic full-suite `trading-binary` contract previously proposed here. The historical trusted-control-plane rehearsal must not merge. Independently useful binary/runtime work requires a new bounded review with no retained control-plane authority.

There is no fallback, compatibility adapter, inherited result, alternate installer, mutable-copy route, tag or same-SHA substitution, prior-run artifact selection, cache-as-proof, persisted authority, external readiness publisher, or dual path. Rollback is a pause or forward fix, never restoration of retired authority.

## Ownership boundary

This issue owns the architecture, the 15 acceptance criteria (M1–M3, B1–B3, L1–L3, S1–S3, X1–X3), and their cross-program traceability. B1–B3 require explicit build-only exact-byte evidence and separate targeted tests; S1–S3 require one public probe path, one producer path, and complete obsolete-owner deletion.

This issue owns no implementation slice. Every Task 1-9 ledger row requires its own exact assigned issue, implementer, evidence, internal adversarial review, and native human review. Applying this body does not complete or auto-complete any implementation row.

## Migration boundary

This issue-body amendment does not relax current controls. Until the governed Task 7 cutover is complete and live state is re-verified, the existing required `gate` and `backtester-gate` reporters remain always present on every pull request, and the required statuses, both Mergify `check-success` predicate sets, exact-head review requirements, queue preflight, and verifier behavior remain authoritative. They must not be path-filtered, renamed, deleted, or weakened early.

Migration order is fixed: land this reconciliation and apply the live issue bodies; prepare the explicit producer, exact-binary negatives, provider/target evidence, caller migration, and Tasks 2A, 3, and 4 under legacy gates; complete every applicable Board-B Task 8 migration; Task 5 operational replacement; Task 6B then Task 6A; governed Task 7 zero-status cutover; then atomically replace legacy Rust Probe plus Debug Test and delete the complete root/Backtester archive/fallback closure before the remaining Task 9 waves and Task 10 measurement. Task 5 is the common predecessor of `6B → 6A → 7`; Task 8 cannot depend on, bypass, or be deferred past Task 5.

No migration or final state exposes two accepted debug, artifact, cache, or evidence paths.

No implementation branch starts for a ledger row marked `ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED`.
<!-- END EXACT ISSUE BODY -->
