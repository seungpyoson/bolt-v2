# Implementation Plan: Manipulated Pump Research Contract

**Branch**: `974-pump-research-contract` | **Date**: 2026-08-20 | **Spec**: `specs/974-pump-research-contract/spec.md`
**Input**: Feature specification from `specs/974-pump-research-contract/spec.md`

## Summary

Add a typed, versioned research-experiment contract to the isolated Rust
Backtesting Engine workspace. Bolt remains the sole authority for experiment
meaning; NautilusTrader remains the sole replay/backtest engine. The contract
admits retained source artifacts, freezes G/D/C/E/P commitments, produces
deterministic episode/control manifests, gates sealed confirmation and selective
enrichment, and publishes atomic evidence-scoped claims through the existing
artifact root and Artifact Index. The current implementation program begins
with zero-spend Stage 0/1 slices and contains no provider adapter, purchase,
strategy, or universal granular-history archive.

## Technical Context

**Language/Version**: Rust 1.97.1, edition 2024
**Primary Dependencies**: Existing `serde`, `toml`, `serde_json`, `sha2`,
`chrono`, `object_store`, `aws-sdk-ssm`, the already locked `aws-sdk-sts`
`1.107.0` promoted to a direct dependency for authenticated caller identity,
and pinned NautilusTrader crates in `crates/backtesting-vertical-slice`
**Storage**: Existing configured S3-compatible `artifact_root`, Artifact Index,
and immutable content-addressed objects; local files are bounded ephemeral work
products only
**Testing**: Rust behavior/integration fixtures in the isolated backtesting
workspace, fail-closed negative-path tests, deterministic fresh-environment
reproduction, advisory remote CI, targeted static checks, and internal
adversarial review for planning/policy artifacts
**Target Platform**: Linux research worker plus local macOS/Linux development;
the production live binary is outside this feature
**Project Type**: Isolated Rust library plus one research CLI with subcommands
**Performance Goals**: Stream or batch over configured roster/observation
partitions without loading a universal granular archive; reproduce identical
semantic hashes in two fresh executions; favor auditability over interactive
latency
**Constraints**: Zero incremental provider spend in the current phase; no
provider calls; no secret display; TOML is the only experiment-config format;
SSM is the only runtime secret source; no second replay engine; no ticker-only
identity joins; no manipulation, queue-position, fill, or exact-PnL overclaim
**Scale/Scope**: A user-approved multi-venue, multi-year coarse discovery frame;
granular storage is limited to later authorized event, control, near-threshold,
negative-control, and warm-up windows

## Governance Check (AGENTS.md)

**Pre-design: PASS**

- One typed Bolt TOML definition owns experiment semantics; NT-native run input
  is compiled from it and cannot redefine the experiment.
- Existing `source_proof`, Artifact Index, artifact store, research analytics,
  run manifest, result contract, and NT catalog surfaces are extended or reused.
  No second data, replay, secret, or build path is introduced.
- Acquisition remains outside Bolt runtime. Token-screener output is candidate
  evidence until Bolt admission, hashing, and registration succeed.
- All provider selection, paid/free access, quotes, and purchases remain blocked
  behind a later explicit user authorization and provider-admission evidence.
- The implementation program is divided into explicitly named vertical slices
  of this spec; no PR may claim the complete feature until every accepted slice
  is implemented and evidenced.
- Production strategy and live-trading behavior are explicitly out of scope.

**Post-design: PASS**

- The data model makes the NT/Bolt boundary, SSM-only secret boundary,
  append-only lineage, and fail-closed state transitions explicit.
- The contracts expose one CLI and one artifact family extension rather than
  parallel workflows.
- Each implementation slice names behavior, fail-closed, deterministic, and
  remote-CI evidence. No source-scanning test is proposed.
- Concrete providers, venues, dates, thresholds, cost caps, role assignments,
  and timestamp authority remain user-approved TOML/registry values. Missing
  registered values block execution; they do not create fallback behavior.

## Architecture

```text
candidate observations / official inventory / retained source artifacts
                              |
                              v
             typed Bolt ExperimentDefinition (TOML)
                              |
             canonical bytes + SHA-256 + Artifact Index
                              |
          +-------------------+-------------------+
          |                                       |
  roster/source admission                 G/D/C/E/P commitments
          |                                       |
          +-------------------+-------------------+
                              |
               deterministic Stage-1 program
                              |
               EpisodeManifest + controls
                              |
             sealed verification/release gate
                              |
                  AtomicClaim registry
                              |
       later separately authorized Stage-2 source packet
                              |
        accepted bounded inputs -> NT catalog/run manifest
                              |
             NT replay/results/snapshots/reports
```

Bolt owns the typed definition, admission decisions, commitments, detector and
control meaning, custody evidence, and claims. NT owns catalog projection,
replay, execution simulation, events, fills, snapshots, and reports. Provider
acquisition workers may produce candidate bytes, but cannot make those bytes
canonical or change experiment meaning.

## Project Structure

### Documentation (this feature)

```text
specs/974-pump-research-contract/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── artifact-contracts.md
│   ├── command-interface.md
│   └── experiment-definition.md
└── tasks.md                       # generated in the next SpecKit phase
```

### Source Code (repository root)

```text
crates/backtesting-vertical-slice/
├── src/
│   ├── research_experiment.rs     # typed TOML, canonicalization, version state
│   ├── research_commitment.rs     # G/D/C/E/P and timestamp verification gates
│   ├── research_custody.rs        # access/disclosure chain and checkpoints
│   ├── pump_episode.rs            # detector, censoring, controls, semantic ids
│   ├── research_claim.rs          # atomic claims and invalidation
│   ├── research_analytics.rs      # result publication integration
│   ├── artifact_index.rs          # one experiment-contracts subfamily addition
│   ├── run_manifest.rs            # immutable NT input compilation only
│   └── bin/pump_research.rs       # one CLI, typed subcommands
└── tests/
    ├── backtesting_vertical_slice_research_experiment.rs
    ├── backtesting_vertical_slice_research_commitment.rs
    ├── backtesting_vertical_slice_research_custody.rs
    ├── backtesting_vertical_slice_pump_episode.rs
    └── backtesting_vertical_slice_research_claim.rs

config/
└── research/                      # reviewed fixture/example TOML, no credentials
```

**Structure Decision**: Implement inside the existing isolated Backtesting
Engine crate because it already owns source proof, content-addressed artifacts,
research result contracts, and NT run materialization. A new service, notebook
runtime, database, or vendor-specific adapter would create a second research
truth and is rejected.

## Implementation Slices

Each slice requires its own branch and PR naming this spec, the slice, remaining
accepted scope, exact-head review request, and acceptance evidence. A separate
GitHub issue is optional rather than a prerequisite.

1. **Typed definition and registry envelope**: parse strict TOML, validate
   roles/scope/versions, produce canonical bytes and hashes, add the single
   `experiment-contracts` Artifact Index subfamily, and reject unknown or
   incomplete values.
2. **Target frame and source admission**: implement roster status accounting,
   time-bounded identities, append-only source assertions, lifecycle states,
   retained-input requirements, and zero-spend/provider-call guards. Evidence
   validity remains separate from Artifact Index hot/inactive storage lifecycle.
3. **Prospective discovery custody foundation**: implement G/D schemas, registered
   timestamp-receipt verification, hash-chained access/disclosure events,
   authenticated role bindings, custodian-controlled fenced access,
   conditional checkpoint closure, single-use authorization, and contamination
   propagation. Test-only authorities and roles cannot satisfy non-test runs.
4. **Stage-1 episode/control manifest**: implement exact observation clocks,
   detector rules, censoring, deduplication, same-time controls, attrition, and
   stable semantic identifiers from synthetic and admitted retained inputs.
5. **Sealed confirmatory execution and primary report**: implement C, compile its
   one locked program, run
   canonical and verification executions in separate environments, compare
   semantic outputs, enforce retry/quarantine rules, and transactionally release
   the required primary report with statement-level `episode_detected` and
   `not_proven` atomic claims or fail closed.
6. **Stage-2 authorization and selection gate**: implement E/P schemas,
   pre-provider sampling, content-neutral acceptance packets, deterministic
   ranking, explicit user-authorization receipt, bounded storage plans, and no
   acquisition adapter.
7. **Claim-tier extensions, mechanism reports, and lifecycle invalidation**:
   extend atomic claims with allegation, sanction, mechanism, and proof tiers;
   extend the primary report contract for mechanism and
   authority evidence, preserve alternatives/falsifiers, propagate source
   quarantine/revocation/expiry through replay artifacts and claims, and retain
   immutable historical lineage.

Slices 1–5 deliver Stage 1 including its primary report. Slice 6 only makes later data acquisition admissible;
it does not select or call a provider. Slice 7 completes auditable publication.

## Requirement and Verification Matrix

| Requirements | Slice | Required evidence |
|---|---|---|
| FR-001–FR-006; SC-001 | 1 | Strict TOML parse/validation behavior tests; canonical byte/hash fixtures; duplicate, unknown-field, role, stale-parent, provider-call, alternate-config, and alternate-replay rejection |
| FR-007–FR-017; SC-002, SC-003 | 2 | Complete-denominator fixtures; delist/rebrand/migration histories; availability clocks; rights/fidelity/retention/correction/expiry failures; zero-spend and zero-provider-call witness |
| FR-018–FR-028 | 3 | G-before-E0 tests; pending/invalid/unknown timestamp failures; role-separation and prior-exposure fixtures; deterministic narrowing and contamination propagation |
| FR-029–FR-035 | 3 | Authenticated-principal mismatch; active/stale lease; stale-head CAS; reused authorization; shared/rotated credential event; exhausted disclosure budget; anchoring-interval and omitted-tail failures |
| FR-036–FR-052; SC-005, SC-007, SC-015 | 3–5 | D/C prerequisite and full-span partition tests; later-access/new-C enforcement; one-primary-cell and robustness-primary separation; two fresh-environment executions; conformance fixtures; mismatch, partial-output, human-exposure, retry-cap, overlap/reuse, unanchored-tail failures, and complete primary-report evidence |
| FR-053–FR-060; SC-004, SC-006, SC-008 | 4 | Golden synthetic observations; explicit clocks; boundary censoring; full-span purge; deduplication; no-lookahead same-time controls; repeat-run semantic equality; unmatched and null-result publication |
| FR-061–FR-074; SC-009–SC-012 | 6 | No Stage 2 before sealed Stage 1; no quote/query/purchase without distinct authorization; frozen draw and provider attrition; mechanical ranking/ties; content-neutral acceptance; case-only and L3 claim limitations; storage-budget failures |
| FR-075–FR-084; SC-013–SC-015 | 7 | Mechanism-report and claim-tier/scope/falsifier fixtures; conflict-of-interest and authority-scope enforcement; no automatic manipulation or non-manipulated label; dependence-aware uncertainty, prior-overlap, positive-unlabeled and generalization disclosures; replay/result/claim invalidation with preserved history |
| FR-002–FR-004; SC-004 | 1, 4, 5 | Integration proof that accepted bounded artifacts compile to the existing NT catalog/run manifest, identify every version/environment/seed, reproduce outputs, and execute no alternate replay path |
| All planning artifacts | Plan gate | Targeted placeholder, path, JSON, whitespace, and contract-consistency checks plus internal adversarial review before completion claims |

Compile-heavy Rust verification remains remote-first through the repository's
advisory CI. Cheap formatting and schema checks may run locally. No result grants
deployment or trading authority.

## Complexity Tracking

| Added complexity | Why required | Simpler alternative rejected |
|---|---|---|
| One `experiment-contracts` Artifact Index subfamily | Commitments and custody gates must be registered before an experiment result exists | Hiding commitments under `experiment-results` cannot prove pre-result ordering |
| Hash-chained custody events with conditional checkpoints | Confirmatory validity depends on proving no unanchored access or disclosure tail | A mutable log or repository timestamp alone cannot establish the required boundary |
| Registered timestamp-verifier boundary | Formal commitments require an independent verified timestamp without selecting a vendor in this feature | Hardcoding a timestamp service violates deferred provider selection and TOML registry rules |
| One staged state machine across G/D/C/E/P | Prevents post-hoc tuning, premature enrichment, and reuse of exposed evaluation information | Independent scripts would create dual authority and unverifiable transitions |

## Residual Risks and Deferred Decisions

- A concrete independent timestamp authority, role identities, venues, dates,
  thresholds, and cost caps are intentionally absent until the user approves
  them in a later experiment version. Canonical execution remains blocked until
  every required registered value is present.
- Non-test role authentication uses AWS STS `GetCallerIdentity`; TOML binds the
  accepted principal/role identity. Missing AWS identity or a binding mismatch
  fails closed. This authenticates the caller but does not eliminate collusion,
  compromised AWS identity, or access outside the committed workflow.
- Existing analyst exposure may make historical ranges exploratory. The exposure
  inventory and contamination rule cannot prove absence of undisclosed knowledge.
- Reported volume, delisted inventory, authoritative labels, and cross-venue
  clocks remain imperfect evidence and must stay visible in reports.
- L2 cannot prove exact queue position; no reviewed historical source currently
  establishes L3. The claim validator therefore rejects those claims without a
  later admitted market-by-order dataset and venue semantics.
- The plan minimizes stored granular bytes but does not estimate a future paid
  provider's cost. Stage 1 must produce exact window counts and storage bounds
  before any separate authorization request.
