# Feature Specification: Rename `position_sizer` → `capital_admission` gate

**Feature Branch**: `docs/711-capital-admission-rename`
**Created**: 2026-06-25
**Status**: Draft
**Input**: GitHub issue #711 ("Rename position_sizer → capital_admission gate (frees naming for #712)")

## Context

The component merged via #658 and specified in `specs/506-nt-position-sizer-submit/spec.md` is
named `position_sizer`, but it does not size positions. Verified at HEAD `eeb52cf0b`:

- `evaluate_position_sizing` takes the requested quantity as input — `let original_quantity = inputs.request.quantity;` (`src/bolt_v3_position_sizer.rs:427`).
- `worst_case_liability` returns `sized_quantity: request.quantity` (`src/bolt_v3_position_sizer.rs:697`), identical to the input — it never reduces or computes a quantity; it only rejects with `InsufficientAllowance` / `InsufficientInventory`.
- The 506 spec itself states the code "is a submit-admission and live-state-feed slice, **not a complete production-grade positional sizer**" (`specs/506-nt-position-sizer-submit/spec.md:50`).

What the component actually does: for an order whose quantity the strategy already chose, it is
the last checkpoint before NT submit. It computes the order's worst-case capital liability,
checks affordability against the configured capital pool (minimum-balance floor + fee/slippage
liability caps), checks the loss governor and that the NT-derived account state is fresh, then
**reserves** that capital against the pool (via the existing `bolt_v3_capital_reservation.rs`
ledger) and **admits or rejects** the order — releasing/revaluing/rebuilding the reservation on
fills and terminal order events. It is a capital-based admission gate: a debit-card-style
authorization-and-hold, not a sizer.

The real size-chooser already exists separately and is correctly named:
`src/bolt_v3_sizing.rs::choose_robust_size` ("the ONE shared 'how much do I want' primitive";
its doc comment states capital enforcement/reservation/admission "is a separate concern").

The misleading name (a) makes reviewers/operators expect Kelly/edge sizing that is not there, and
(b) occupies the `position_sizer` / `position_sizing_engine` name that #712 needs for the real
positional-sizing engine.

This feature is a **rename only**. It changes no trading, admission, reservation, or
loss-governor behavior. It is the prerequisite cleanup for #712; #712 (the real engine) is out of
scope.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Names accurately describe the component (Priority: P1)

A reviewer or operator reading the code, config, persisted evidence, or audit allowlist sees
names that describe *capital admission* (authorize-and-hold), not *position sizing*.

**Why this priority**: This is the entire point of the issue. Inaccurate names caused #712 to be
filed twice (#711 + #712) and risk an operator mis-trusting the component as a risk sizer.

**Independent Test**: After Slice 1, `git grep -nE 'position_sizer|PositionSizing|PositionSizer'`
returns hits only in (a) git history, (b) the kept serialized strings pending Slice 2, and (c)
this spec / issue text — never in renamed module identifiers. After Slice 2, it returns nothing
in `src/`, `tests/`, `config/`, `scripts/`, and the runtime-literal audit except deliberately
documented historical references.

**Acceptance Scenarios**:

1. **Given** the renamed code, **When** a reviewer opens the gate module, **Then** the module is
   `src/bolt_v3_capital_admission.rs`, the core type is `CapitalAdmissionGate`, and the entry
   point is `evaluate_capital_admission`.
2. **Given** the renamed code, **When** a reviewer inspects the embedding state, **Then**
   `BoltV3SubmitAdmissionState` exposes the gate as a `capital_admission` field (not
   `position_sizer`).
3. **Given** the renamed internal liability fields, **When** a reviewer reads `LiabilityQuote`,
   **Then** the fields are `accepted_quantity` / `calculated_liability` / `reserved_liability`
   (not `sized_quantity` / `liability_before_sizing` / `liability_after_sizing`).

### User Story 2 - An operator upgrades without losing reservation recovery (Priority: P1)

An operator who has existing persisted decision-evidence (schema v13) and an existing config using
`[risk.capital_pools.sizing_policy]` upgrades to the renamed binary and still recovers open-order
reservations correctly, or fails closed safely if they skip migration.

**Why this priority**: The serialized renames + `SCHEMA_VERSION` bump cross a persistence/operator
contract. Getting this wrong could fail live startup with open orders or, worse, recover the wrong
reservation. (Currently theoretical — see Assumptions — but the contract must be correct.)

**Independent Test**: Run the migration scripts against a representative v13 evidence directory and
a v13 config, start the renamed binary, and confirm reservations rebuild identically to pre-rename
behavior; separately confirm an un-migrated v13 input fails closed (never silently mis-recovers).

**Acceptance Scenarios**:

1. **Given** a v13 evidence file migrated by the JSONL migration script, **When** the renamed
   binary reads it at startup, **Then** reservation recovery produces the same reserved liability
   per pool as before the rename.
2. **Given** an un-migrated v13 evidence file with a reservation-bearing record
   (`submit_reservation_metadata` / `submit_reservation_fill`), **When** the renamed binary reads
   it, **Then** it fails closed at header validation and the gate starts unreconciled (it does not
   silently ignore a possibly-open reservation).
3. **Given** an un-migrated config still using `sizing_policy`, **When** the renamed binary loads
   it, **Then** parsing fails fast (via `deny_unknown_fields`) rather than silently ignoring the
   block.
4. **Given** a config migrated to `capital_admission_policy`, **When** the renamed binary loads it,
   **Then** the pool's minimum-balance floor and fee/slippage caps parse to the same values as
   before.

### User Story 3 - #712 can claim the freed name (Priority: P2)

The real positional-sizing engine (#712) can introduce `position_sizer` / `position_sizing_engine`
identifiers and a `position_sizer`-flavored evidence record without colliding with this component.

**Why this priority**: This rename is explicitly the prerequisite for #712; the freed namespace is
the deliverable's downstream value.

**Independent Test**: After Slice 2, no code, config, evidence record kind, gate-id, or audit
classification uses `position_sizer`/`position_sizing`, so #712 may add them fresh.

**Acceptance Scenarios**:

1. **Given** the completed rename, **When** #712 adds a `position_sizing_engine` module, **Then**
   there is no module, type, record kind, gate-id, or TOML key already using that name.

### Edge Cases

- **Below-schema audit-only record present** (old `position_sizer_rebuild`): must be skipped on
  read, not fail the whole recovery (it carries no reservation state). Verified design at
  `src/bolt_v3_decision_evidence.rs:1987-1994`.
- **Below-schema reservation-bearing record present**: must fail closed at
  `DecisionEvidenceEnvelopeHeader::validate` (`src/bolt_v3_decision_evidence.rs:2284`), degrading
  to the unreconciled gate. This behavior must be preserved by the rename.
- **Mixed-version evidence directory** (some v13, some v14): the strict `!=` schema check means a
  partially migrated directory fails closed on the v13 reservation records; the migration script
  must rewrite the entire directory, not a subset.
- **Slice 1 merged without Slice 2**: leaves code saying `capital_admission` while serialized
  strings still say `position_sizer`. This is an accepted *intermediate* state between the two PRs
  of one issue only; Slice 2 must land to avoid a half-rename on `main`.

## Requirements *(mandatory)*

### Functional Requirements

**Naming — internal (Slice 1, behavior-neutral)**

- **FR-001**: The gate module MUST be renamed `src/bolt_v3_position_sizer.rs` →
  `src/bolt_v3_capital_admission.rs`; the runtime feed `src/bolt_v3_position_sizer_runtime_feed.rs`
  → `src/bolt_v3_capital_admission_runtime_feed.rs`; the input-state module
  `src/bolt_v3_sizing_state.rs` → `src/bolt_v3_capital_admission_state.rs`; with matching
  `pub mod` updates in `src/lib.rs`.
- **FR-002**: All Rust identifiers carrying the gate-context misnomer MUST be renamed under the
  scheme `PositionSizing*`/`PositionSizer*`/`Sizing*`/`SizedAdmission*` → `CapitalAdmission*`.
  Anchor examples: `PositionSizingAdmissionGate` → `CapitalAdmissionGate`;
  `evaluate_position_sizing` → `evaluate_capital_admission`; `SizingPolicy` →
  `CapitalAdmissionPolicy`; `NtDerivedSizingState` → `NtDerivedCapitalAdmissionState`;
  `PositionSizerRuntimeFeed` → `CapitalAdmissionRuntimeFeed`;
  `BoltV3SubmitPositionSizerState`/`Config`/`...NtComponents` →
  `BoltV3SubmitCapitalAdmissionState`/`Config`/`...NtComponents`;
  `BoltV3AdmissionOutcome::RejectedPositionSizing` → `RejectedCapitalAdmission`.
- **FR-003**: The embedded gate field on `BoltV3SubmitAdmissionState` MUST be renamed
  `position_sizer` → `capital_admission`.
- **FR-004**: Internal (non-serialized) liability/decision fields MUST be renamed for accuracy:
  `sized_quantity` → `accepted_quantity`; `liability_before_sizing` → `calculated_liability`;
  `liability_after_sizing` → `reserved_liability` (in `LiabilityQuote`, `SizedAdmissionDecision`,
  `SizedAdmissionEvidence`). These structs carry no `Serialize` derive (verified — no `Serialize`
  in `src/bolt_v3_position_sizer.rs`), so this is internal-only.
- **FR-005**: Test function names, fixtures, and `tests/support/mod.rs` helpers carrying the
  misnomer MUST be renamed to match.

**Naming — serialized / contract (Slice 2)**

- **FR-006**: The decision-evidence record kind MUST be renamed `position_sizer_rebuild` →
  `capital_admission_rebuild` (`BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND`,
  `src/bolt_v3_decision_evidence.rs:42`, plus its read-dispatch arms).
- **FR-007**: The audit gate-id MUST be renamed `bolt_v3.position_sizer_rebuild` →
  `bolt_v3.capital_admission_rebuild` (`BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID`,
  `src/bolt_v3_decision_evidence.rs:26`).
- **FR-008**: The admission outcome string MUST be renamed `rejected_position_sizing` →
  `rejected_capital_admission`.
- **FR-009**: The evidence source label MUST be renamed `nt_position_sizer_runtime_components` →
  `nt_capital_admission_runtime_components`.
- **FR-010**: The decision-evidence schema version MUST be bumped
  `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` 13 → 14 (`src/bolt_v3_decision_evidence.rs:23`).
- **FR-011**: The operator TOML key MUST be renamed `sizing_policy` → `capital_admission_policy`
  under `[[risk.capital_pools]]` (Rust field on `CapitalPoolBlock`, `src/bolt_v3_config.rs`), and
  every fixture/production config under `config/` and `tests/fixtures/` updated.
- **FR-012**: The runtime-literal audit allowlist
  (`docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`) MUST be updated:
  `path` entries for the renamed module files, all `position_sizer_*` `classification` values, and
  every verbatim `context`/`literal` line that changed.
- **FR-013**: The schema doc (`docs/bolt-v3/2026-04-25-bolt-v3-schema.md`) and the Python schema
  verifier (`scripts/verify_bolt_v3_schema_current.py`, `scripts/test_verify_bolt_v3_schema_current.py`)
  MUST reflect the renamed record kind and the new `schema_version`.

**Migration (Slice 2)**

- **FR-014**: A one-time JSONL migration tool MUST rewrite existing decision-evidence files from
  v13 to v14: rename the record kind / gate-id / outcome strings above and set `schema_version`
  14 on every envelope (including the un-renamed but version-bearing
  `submit_reservation_metadata` / `submit_reservation_fill` records). It MUST preserve all other
  field values byte-for-byte semantically.
- **FR-015**: A one-time config migration tool MUST rewrite `sizing_policy` →
  `capital_admission_policy` in operator TOML.
- **FR-016**: The running binary MUST accept only the new schema/names (no dual-path runtime
  reader, no accept-both TOML key) — migration is the single one-time bridge (NO DUAL PATHS).

**Invariants that MUST NOT change (both slices)**

- **FR-017**: No trading, admission, reservation, loss-governor, or NT-feed behavior changes. The
  worst-case-liability calculation, reservation arithmetic, freshness checks, lifecycle handling,
  and admit/reject outcomes are identical pre- and post-rename.
- **FR-018**: The following names MUST be kept (already accurate): the `bolt_v3_capital_reservation.rs`
  ledger and its types; `bolt_v3_sizing.rs` / `choose_robust_size`; all loss-governor /
  loss-protection names; `enforce_submit_admission`; `min_remaining_pool_balance`, `fee_slippage`,
  `max_fee_liability`, `max_slippage_liability`; the serialized record kinds
  `submit_reservation_metadata` / `submit_reservation_fill` and the field `submit_reservation_id`;
  and all decision-evidence JSON payload field names (they do not embed the misnomer).
- **FR-019**: A scoped naming guard MUST be added so that `position_sizer` / `PositionSizing` /
  `PositionSizer` identifiers cannot reappear in the renamed modules in a future change.
- **FR-020**: The `gated_source_roots.manifest` MUST NOT require changes — the renamed files are
  not in any gated source root (only `src/bolt_v3_submit_admission.rs` is). The plan must verify
  this rather than assume it.

### Key Entities

- **Capital admission gate** — the renamed component (`bolt_v3_capital_admission.rs`): decides
  admit/reject for one order based on capital, using the reservation ledger.
- **Capital reservation ledger** — unchanged (`bolt_v3_capital_reservation.rs`): the bookkeeping
  the gate drives (`reserve` / `release` / `revalue`).
- **Decision-evidence records** — persisted JSONL; `position_sizer_rebuild` (audit-only) renamed;
  `submit_reservation_metadata` / `submit_reservation_fill` (recovery-critical) kept, version-bumped.
- **Capital pool config block** — `[[risk.capital_pools]]` with its renamed `capital_admission_policy`
  sub-block.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After Slice 2, `git grep -nE 'position_sizer|PositionSizing|PositionSizer|position_sizing'`
  over `src/`, `tests/`, `config/`, `scripts/`, and the runtime-literal audit returns zero matches
  except explicitly documented historical references.
- **SC-002**: Exact-head remote CI is green on each slice's PR (full `CI` workflow + Backtester CI
  + actionlint), per AGENTS.md remote-first Rust verification.
- **SC-003**: The runtime-literal audit and `scripts/verify_bolt_v3_schema_current.py` pass with
  the renamed identifiers.
- **SC-004**: A v13 evidence directory migrated by the migration tool yields byte-identical
  reservation recovery (same reserved liability per pool) versus the pre-rename binary on the
  pre-migration data; an un-migrated v13 reservation record fails closed.
- **SC-005**: A config migrated to `capital_admission_policy` parses to the same pool policy values
  as the pre-rename `sizing_policy` config; an un-migrated config fails fast.
- **SC-006**: No behavior test that existed before the rename changes its asserted outcome (only
  identifiers/strings change), demonstrating FR-017.

## Assumptions

- **No active deploy.** There is currently no running production node and #529 restart-with-open-
  orders recovery has not been live-exercised, so there is likely no production v13 evidence/config
  requiring migration today. The migration tools ship for correctness/completeness (NO DEBTS); the
  operator-restart exposure in User Story 2 is currently theoretical but the contract must be right.
- **#658 is merged** (2026-06-15), so the `position_sizer` naming is on `main` and this rename is
  unblocked.
- **Evidence-driven verification** per `AGENTS.md` governs (not mandatory TDD red-green). New code
  (migration tools) gets tests; the mechanical rename is proven by the compiler + naming guard +
  grep + exact-head CI.
- **Golden source-digest gate is removed** — no digest re-derivation is needed (corrects the
  stale #658 note).
- **Root-config schema versioning** is unconfirmed: there is a `schema_version: u32` field on the
  root config and a test asserting `== 1`, but no `SUPPORTED_ROOT_SCHEMA_VERSION` constant was
  found. `deny_unknown_fields` already makes an un-migrated `sizing_policy` config fail fast; the
  plan must confirm at implementation whether an explicit root-config version bump is additionally
  required.
