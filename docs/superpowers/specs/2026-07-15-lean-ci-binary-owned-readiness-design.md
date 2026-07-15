# Lean CI and Binary-Owned Readiness Decision

Status: approved architecture; implementation and live cutover remain blocked by the program ledger.

## Provenance

- Approved decision packet: `/private/tmp/1016-lean-ci-decision-packet-r2.md`, SHA-256 `b2d6a5c9952078c695c2cff54352c1dbec8813974ca3469b6e6515730e3651db`.
- Original approved implementation-plan source: SHA-256 `dda7e936f9070aaa550e4bb5f6f64f0e760947b12046fe47f4e87f4794615ad1`; the durable plan is `docs/superpowers/plans/2026-07-15-lean-ci-binary-owned-readiness.md` and includes the governed reconciliation below.
- Reviewed remote-verification reconciliation input: `/private/tmp/1016-fast-probe-cold-warm-design.md`, SHA-256 `30e30cb9fd8c125597593ff3e677a89761c4a3365c60c7cb4b533a380ac3f974`, inspected against `main` at `37e619b3fbd65fc041a05399ecf1750b8999567a`.
- External resolution review: Claude Code CLI (Anthropic), model `claude-fable-5`, conclusion `APPROVE` on 2026-07-15.
- Architecture issue: #1016. Implementation and deletion ownership is recorded separately in `docs/ci/1016-lean-ci-program-ledger.md`.

## Decision

The selected end state has zero required CI status contexts. CI is visible evidence, not merge authority. Native GitHub human controls remain the merge authority: code-owner approval, stale-review dismissal, last-push approval, and human review-thread resolution.

Compile-heavy Rust evidence is explicit, exact-SHA, workspace-and-target-specific only. It never starts automatically on a pull request, merge group, or `main` push. One public targeted-probe path and one explicit trusted-producer workflow replace the superseded every-`main`-push/full-nextest `trading-binary` contract; neither result can authorize or veto merge, installation, launch, or trading.

The repository explicitly accepts that an approved merge can temporarily leave `main` red or broken. That is repository risk only. It is never deployment or trading permission.

## Remote Rust Evidence Contract

This section is the authoritative contract for the plan, program ledger, governance summary, and issue-body handoffs. Those documents own sequencing and scope; they do not redefine this behavior.

- Root/trading-binary and Backtester are separate workspaces, with separate manifests, locks, target directories, commands, warmth identities, and evidence records. Root-only work does not wake Backtester; Backtester-only and documentation-only work consume zero root Cargo minutes; root binary-only work consumes zero Backtester Cargo minutes. A root-library/package/build/toolchain change that actually affects Backtester's root path dependency selects separately authorized explicit targets in both workspaces.
- Cheap local routing returns a reviewed finite set of explicit `(workspace, build class, Cargo target, selector)` tuples or `UNCLASSIFIED`. It prints one command per finite target and no command for unknown, ambiguous, or unresolved ownership. It never dispatches, broadens to a package/workspace/full-CI run, or treats "run both" as a fallback.
- The targeted probe is `workflow_dispatch`-only and read-only. It binds one clean pushed SHA, one workspace, one exact configured Cargo target, and an optional exact test selector, then executes exactly one configured Cargo/nextest command on an isolated ephemeral disk. Targeted tests remain probes; they are never hidden inside an artifact build.
- The single trusted producer is explicit-only and permits one active writer under a proven finite provider bound. It may seed one exact configured root or Backtester host target. Its `root-artifact` operation performs one locked ARM64 release build, stages and hashes only the produced file, and runs positive and fail-closed evidence against those exact bytes. `root-artifact` runs no locked, full, broad, or targeted Cargo test suite.
- Every probe and producer uses one checksum/version-pinned, preinstalled sccache binary as the mandatory `RUSTC_WRAPPER`. Missing or invalid wrapper/configuration fails before Cargo. A cache miss or backend outage may degrade only inside that wrapper; there is no wrapper removal, direct/uncached retry, target or nextest archive restore, prior/same-SHA result reuse, carry-forward, binary sidecar, aggregate target, scheduler, compile retry, or alternate public path. Probes never receive cache-write authority.
- Root and Backtester test targets are split only when measured clean/warm compile latency and source/module fan-in require it. The Backtester inventory of 75 modules and 40,730 lines, root inventory of 90,894 lines, and combined 131,624 lines are context, not target sizes or acceptance criteria. A split preserves exact test semantics and has no aggregate compatibility harness.
- Completion is behavioral: zero automatic heavy frequency; exact routing; cost and runner minutes; separately labeled cold, dependency-warm/target-cold, target-warm, and degraded runs; bounded parallelism and ephemeral disk; fast first-failure output; one probe path; one producer path; and complete deletion of obsolete callers/owners. File, nonblank-line, branch, and module counts are diagnostic telemetry only.

## Operational Authority

An operator selects one manifest that identifies an exact `main` commit, artifact SHA-256, and config-bundle SHA-256. The single installer verifies those bytes and places the executable at the configured content-addressed immutable path. The systemd unit invokes that exact executable's `ops launch`.

`ops launch` owns one finite in-process pre-arm phase. It verifies executable identity, config-bundle identity, a nonempty enforceable deploy target, SSM-only secrets, storage/prestart state, reference-price health, and shared admission/runtime construction. Only complete success constructs an opaque, non-serializable, non-cloneable, one-use Rust `LiveReadinessPermit`. The sole Start entrypoint consumes that permit by value immediately, with no await, publication, or other fallible authorization step in between.

Installation is inert. A log or persisted receipt is audit-only and is never read as authority. Every systemd start or restart reruns the complete phase and obtains a fresh in-memory permit.

## Single-Path Boundary

The retained design has:

- one public exact-SHA targeted-probe path and one explicit trusted-producer workflow/path;
- one manifest-bound, content-addressed immutable installer;
- one systemd entrypoint invoking the exact installed executable's `ops launch`;
- one in-process permit-consuming Start boundary; and
- one mechanical merge-queue operator path after its CI verdict machinery is retired.

There is no fallback, compatibility adapter, inherited result, alternate installer, mutable-copy route, tag or same-SHA substitution, prior-run artifact selection, cache-as-proof, persisted authority, external readiness publisher, trusted merge publisher, or dual path. Failure or ambiguity pauses merge, deploy, or trading as appropriate and receives a forward fix; it does not restore retired authority.

## Supersession

The trusted-App/protected-base verifier, precursor, activation, freeze, merge-protection ceremony, replay/tombstone control plane, and App-qualified merge authority previously proposed for #1016 are superseded. The historical 36,704–38,043-line rehearsal must not merge. Independently useful binary or runtime work may proceed only as a newly reviewed, issue-owned slice that stands on its own evidence and contains no control-plane authority or compatibility route.

#1016 owns this architecture and its acceptance criteria only. It does not own or automatically complete any implementation slice; every Task 1-9 ledger row requires its own exact assigned issue.

## Migration Boundary

This decision does not change today's merge controls. Until the governed Task 7 cutover is complete and live state is re-verified, the existing required statuses, both Mergify `check-success` predicate sets, exact-head review requirements, queue preflight, and verifier behavior remain authoritative.

In particular, the current required `gate` and `backtester-gate` reporters remain always present on every pull request until Task 7. They must not be path-filtered, renamed, deleted, or weakened early.

Migration order is fixed:

1. land this governance reconciliation and apply the reconciled live issue bodies before an affected implementation branch;
2. under unchanged legacy gates, complete the explicit producer and exact-binary negatives, Tasks 2A, 3, and 4, provider capacity/isolation proof, measured root/Backtester target audits, and non-public caller-migration preparation;
3. complete every applicable Board-B Task 8 runtime-invariant migration under legacy gates before Task 5;
4. complete Task 5's atomic operational replacement, then Task 6B and Task 6A under legacy gates;
5. re-prove native controls and the complete acceptance matrix, then perform the governed Task 7 zero-status cutover;
6. after caller migration, atomically replace the legacy Rust Probe public contract and Debug Test, then delete the complete root and Backtester archive/fingerprint/reuse/sidecar/fallback closure; and
7. complete the remaining issue-owned Task 9 deletion waves and Task 10 behavioral measurement.

Task 5 is the common predecessor of the fixed `6B → 6A → 7` chain. Task 8 cannot depend on, bypass, or be deferred past Task 5.

No migration may expose two accepted debug, artifact, cache, or evidence paths. Pre-Task-7 coexistence is limited to staging and caller migration required by current gates; it is not completion.

No implementation branch starts for a ledger row marked `ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED`.

## Acceptance

The binding acceptance matrix is M1–M3, B1–B3, L1–L3, S1–S3, and X1–X3 in the approved plan. In particular:

- M1–M3 prove zero-status merge governance, retained native human controls, single-PR queues, and the accepted red-`main` risk.
- B1–B3 prove explicit build-only exact-binary evidence, separate exact-target tests, and no authorization edge.
- L1–L3 prove manifest/config/target binding and the one-use in-process permit boundary.
- S1–S3 prove one public probe path, one producer path, one policy owner, and complete obsolete-closure deletion without re-encoding CI debt.
- X1–X3 prove safe sequencing, issue-owned deletion, honest pause semantics, no rehearsal merge, and measurement.

Missing, failed, stale, timed-out, cancelled, skipped, unknown, ambiguous, or substituted operational evidence must leave Start unreachable.
