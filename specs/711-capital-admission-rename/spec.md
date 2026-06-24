# Feature Specification: Rename `position_sizer` → `capital_admission` gate

**Feature Branch**: `docs/711-capital-admission-rename`
**Created**: 2026-06-25
**Status**: Draft (revised after external review — GPT/Kimi/GLM, adjudicated vs code at HEAD `07db9cb04`)
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

This feature is a **rename only**, shipped as **one PR**. It changes no trading, admission,
reservation, or loss-governor behavior. It is the prerequisite cleanup for #712; #712 (the real
engine) is out of scope.

### Why one PR (not two slices)

An earlier draft split this into an internal-rename slice and a serialized-contract slice. Review
disproved the split's premise on two counts, both verified in code:

1. **The internal slice is not byte-neutral.** `BoltV3AdmissionOutcome` derives `Serialize` with
   `#[serde(rename_all = "snake_case")]` (`src/bolt_v3_decision_evidence.rs:960-961`) and is
   persisted through `serde_json::to_vec` (`src/bolt_v3_decision_evidence.rs:2648`). Renaming the
   variant `RejectedPositionSizing` → `RejectedCapitalAdmission` therefore changes the persisted
   byte `rejected_position_sizing` → `rejected_capital_admission` automatically — there is no
   identifier-only rename for that variant. Keeping it byte-neutral would require throwaway
   `#[serde(rename = "...")]` scaffolding deleted by the second slice, i.e. a dual path / debt.
2. **A half-rename on `main` is a debt.** Per `AGENTS.md` (NO DEBTS / "80% done is 0% done"), code
   reading `capital_admission` while emitting `position_sizer_*` strings is not an acceptable
   resting state.

Doing the whole rename — code, serialized values, schema bump, config key, root-config version,
migration tools, audit/docs — in one PR is the class-correct, debt-free unit.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Names accurately describe the component (Priority: P1)

A reviewer or operator reading the code, config, persisted evidence, or audit allowlist sees
names that describe *capital admission* (authorize-and-hold), not *position sizing*.

**Why this priority**: This is the entire point of the issue. Inaccurate names caused #712 to be
filed twice (#711 + #712) and risk an operator mis-trusting the component as a risk sizer.

**Independent Test**: After the PR, the repo-wide misnomer fence (FR-019) returns zero matches
over `src/`, `tests/`, `config/`, `scripts/`, and `docs/` except the lines named in its explicit
allowlist file (legacy skip literal, this spec, the 506 spec, git history).

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
`[risk.capital_pools.sizing_policy]` (root `schema_version = 1`) upgrades to the renamed binary and
still recovers open-order reservations correctly, or fails closed safely if they skip migration.

**Why this priority**: The serialized renames + decision-evidence `SCHEMA_VERSION` 13→14 + root
config `schema_version` 1→2 cross a persistence/operator contract. Getting this wrong could fail
live startup with open orders or, worse, recover the wrong reservation. (Currently theoretical —
see Assumptions — but the contract must be correct.)

**Independent Test**: Run the migration tools against a representative v13 evidence directory and a
v13 config, start the renamed binary, and confirm reservations rebuild with **identical reserved
liability per pool** to pre-rename behavior; separately confirm an un-migrated v13 input fails
closed (never silently mis-recovers).

**Acceptance Scenarios**:

1. **Given** a v13 evidence directory migrated by the JSONL migration tool, **When** the renamed
   binary reads it at startup, **Then** reservation recovery produces the **same reserved liability
   per pool** as the pre-rename binary on the pre-migration data (equivalence proof, SC-004).
2. **Given** an un-migrated v13 evidence file with a reservation-bearing record
   (`submit_reservation_metadata` / `submit_reservation_fill`), **When** the renamed binary reads
   it, **Then** it fails closed at header validation (`src/bolt_v3_decision_evidence.rs:2284`) and
   the gate starts unreconciled (it does not silently ignore a possibly-open reservation).
3. **Given** an un-migrated v13 evidence file with an audit-only record (legacy
   `position_sizer_rebuild`), **When** the renamed binary reads it, **Then** it is skipped (not
   failed), because the below-schema non-recovery skip set retains the legacy literal (FR-017).
4. **Given** an un-migrated config still using `sizing_policy` (or root `schema_version = 1`),
   **When** the renamed binary loads it, **Then** parsing/validation fails fast (via
   `deny_unknown_fields` and the `SUPPORTED_ROOT_SCHEMA_VERSION` check) rather than silently
   ignoring the block.
5. **Given** a config migrated to `capital_admission_policy` with `schema_version = 2`, **When** the
   renamed binary loads it, **Then** the pool's minimum-balance floor and fee/slippage caps parse to
   the same values as before.

### User Story 3 - #712 can claim the freed name (Priority: P2)

The real positional-sizing engine (#712) can introduce `position_sizer` / `position_sizing_engine`
identifiers and a `position_sizer`-flavored evidence record without colliding with this component.

**Why this priority**: This rename is explicitly the prerequisite for #712; the freed namespace is
the deliverable's downstream value.

**Independent Test**: After the PR, no code, config, evidence record kind, gate-id, audit
classification, or TOML key uses `position_sizer`/`position_sizing` (outside the allowlist), so #712
may add them fresh.

**Acceptance Scenarios**:

1. **Given** the completed rename, **When** #712 adds a `position_sizing_engine` module, **Then**
   there is no module, type, record kind, gate-id, or TOML key already using that name.

### Edge Cases

- **Below-schema audit-only record present** (legacy `position_sizer_rebuild`): must be skipped on
  read, not fail the whole recovery (it carries no reservation state). The skip set
  (`decision_evidence_header_is_below_current_schema_non_recovery_record`,
  `src/bolt_v3_decision_evidence.rs:1995-2006`) currently matches on the constant
  `BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND`. After the rename flips that constant's value, the
  skip set MUST also retain the **legacy literal** `"position_sizer_rebuild"` (alongside the new
  `"capital_admission_rebuild"`), or pre-rename audit records would stop being skipped and fail
  closed. (FR-017.)
- **Below-schema reservation-bearing record present**: must fail closed at
  `DecisionEvidenceEnvelopeHeader::validate` (`src/bolt_v3_decision_evidence.rs:2284`), degrading to
  the unreconciled gate. This behavior is preserved by the rename.
- **Mixed-version evidence directory** (some v13, some v14): the strict `!=` schema check means a
  partially migrated directory fails closed on the v13 reservation records; the migration tool must
  rewrite the **entire** directory and be **idempotent/resumable** (re-running after an interrupted
  run completes it, rather than refusing).
- **Migration interrupted mid-directory**: the tool must write each file atomically (temp +
  fsync + rename) so an interrupted run never leaves a truncated/half-written evidence file; a
  resumed run skips already-v14 records and finishes the rest.
- **Future-schema evidence present** (schema > 14): the migration tool must refuse rather than
  downgrade.

## Requirements *(mandatory)*

### Functional Requirements

**Naming — code identifiers (rename by rule, not enumeration)**

- **FR-001**: The three gate modules MUST be renamed (via `git mv`): `src/bolt_v3_position_sizer.rs`
  → `src/bolt_v3_capital_admission.rs`; `src/bolt_v3_position_sizer_runtime_feed.rs` →
  `src/bolt_v3_capital_admission_runtime_feed.rs`; `src/bolt_v3_sizing_state.rs` →
  `src/bolt_v3_capital_admission_state.rs`; with matching `pub mod` updates in `src/lib.rs`. The
  test file `tests/bolt_v3_position_sizer_runtime_feed.rs` MUST be renamed
  `tests/bolt_v3_capital_admission_runtime_feed.rs`.
- **FR-002**: Every gate-context identifier carrying the misnomer MUST be renamed under the rule
  `PositionSizing*`/`PositionSizer*`/`Sizing*`(gate-context)/`SizedAdmission*` → `CapitalAdmission*`,
  and `sized_quantity`/`*_sizing` snake_case identifiers to their `capital_admission`/accuracy
  equivalents. The rename is **repo-wide** across `src/`, `tests/`, and `scripts/`, not scoped to the
  three modules. Verified surfaces that MUST be covered (non-exhaustive anchors — the fence in
  FR-019 is the completeness authority):
  - Gate core (`bolt_v3_capital_admission.rs`): `PositionSizingAdmissionGate` →
    `CapitalAdmissionGate`; `evaluate_position_sizing` → `evaluate_capital_admission`; the request /
    inputs / lifecycle / policy / snapshot / evidence-kind types per the rule.
  - Internal liability/decision fields (no `Serialize` derive — internal-only): `sized_quantity` →
    `accepted_quantity`; `liability_before_sizing` → `calculated_liability`; `liability_after_sizing`
    → `reserved_liability`.
  - `src/bolt_v3_submit_admission.rs` (267 hits): the embedding field (FR-003); request/claim field
    `position_sizing: Option<BoltV3CompiledOrderSizingEvidence>` (`:2706`, `:2829` — **internal,
    not serialized**; verified no `Serialize` derive, zero `"position_sizing"` JSON keys) → e.g.
    `admission_evidence: Option<BoltV3CompiledOrderAdmissionEvidence>`;
    `BoltV3SubmitAdmissionError::PositionSizingRejected`; `BoltV3PositionSizerRejectReason` + its
    variants (`SizingRejected`, `MissingSizingEvidence`, `SizedQuantityMismatch`);
    `BoltV3PositionSizerReservationRollback` / `BoltV3PositionSizerSubmitDecision`.
  - `src/bolt_v3_live_node.rs` (85 hits): `BoltV3PositionSizerVenueSpendabilitySourceConfig`; error
    variant `StartupPositionSizerRebuild` (and its `Display`/`anyhow!` text — see FR-005);
    `sizing_policy_from_pool`; the `position_sizer_*` accessor methods, builders, and
    open-order-reservation rebuild helpers.
  - `src/bolt_v3_config.rs`: type `CapitalPoolSizingPolicyBlock` (`:256`) → `CapitalAdmissionPolicyBlock`
    (the field rename is FR-011).
  - `src/bolt_v3_validate.rs` (8 hits): `validate_position_sizer_recovery_evidence` and the
    validation label strings.
  - `src/strategies/binary_oracle_maker/mod.rs`, `src/strategies/binary_oracle_edge_taker/tests/*.rs`,
    `tests/*.rs`, `tests/support/mod.rs`: `record_position_sizer_rebuild_audit` stubs/fixtures and
    all referencing identifiers.
- **FR-003**: The embedded gate field on `BoltV3SubmitAdmissionState` MUST be renamed
  `position_sizer` → `capital_admission`.
- **FR-004**: KEEP (already accurate, MUST NOT rename): `FeeSlippagePolicy`,
  `LiabilityQuote`/`LiabilityError` type names, `VenueSpendabilitySnapshot`,
  `ReservationLedgerSnapshot`, and everything in FR-017's keep-list.
- **FR-005**: All user-visible strings tied to renamed identifiers (e.g. `StartupPositionSizerRebuild`'s
  `Display` text, `anyhow!`/`bail!` messages, log lines) MUST be updated in lockstep with the
  identifier rename — the compiler updates the match *pattern* but not string literals, so these
  require explicit edits.

**Naming — serialized / contract values (persisted bytes)**

- **FR-006**: The decision-evidence record kind value MUST change `"position_sizer_rebuild"` →
  `"capital_admission_rebuild"` (`BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND`,
  `src/bolt_v3_decision_evidence.rs:42`), including its read-dispatch `match` arms (`:1510`, `:1784`).
- **FR-007**: The audit gate-id value MUST change `"bolt_v3.position_sizer_rebuild"` →
  `"bolt_v3.capital_admission_rebuild"` (`BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID`,
  `src/bolt_v3_decision_evidence.rs:26`).
- **FR-008**: The admission outcome string value MUST change `"rejected_position_sizing"` →
  `"rejected_capital_admission"`. This has **two** sites that must agree: (a) the serde path — the
  `BoltV3AdmissionOutcome` variant rename produces the new value via `rename_all` automatically
  (`src/bolt_v3_decision_evidence.rs:961`); and (b) a **manual match arm** at
  `src/bolt_v3_submit_admission.rs:138` whose RHS string literal the compiler does **not** auto-update.
  The round-trip test expected-value table (`src/bolt_v3_decision_evidence.rs:3531-3556`) MUST be
  updated to the new value.
- **FR-009**: The evidence source label value MUST change `"nt_position_sizer_runtime_components"` →
  `"nt_capital_admission_runtime_components"`.
- **FR-010**: The decision-evidence schema version MUST be bumped
  `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` 13 → 14 (`src/bolt_v3_decision_evidence.rs:23`).

**Config + root schema**

- **FR-011**: The operator TOML key MUST be renamed `sizing_policy` → `capital_admission_policy`
  under `[[risk.capital_pools]]` (field on `CapitalPoolBlock`, `src/bolt_v3_config.rs:243`), with the
  block type renamed per FR-002. Every config under `config/` and `tests/fixtures/` and the
  `tests/config_parsing.rs` assertions MUST be updated.
- **FR-012**: Because renaming a required TOML key is a breaking root-config schema change, the root
  config schema version MUST be bumped: `SUPPORTED_ROOT_SCHEMA_VERSION` 1 → 2
  (`src/bolt_v3_validate.rs:108`; strict `!=` check at `:167`); the `schema_version` field in every
  root config under `config/` and `tests/fixtures/` updated to `2`; and the `tests/config_parsing.rs`
  test asserting `== 1` updated. (`deny_unknown_fields` alone is insufficient — verified the const
  exists and is enforced; corrects the earlier "unconfirmed" assumption.)

**Migration (one-time, offline)**

- **FR-013**: A one-time JSONL migration tool MUST rewrite existing decision-evidence directories
  from v13 to v14: it changes the record-kind / gate-id / outcome / source-label string **values**
  (FR-006/007/008/009) and sets every envelope's `schema_version` to 14 — **including** the
  un-renamed but version-bearing `submit_reservation_metadata` / `submit_reservation_fill` records.
  It MUST:
  - **Preserve bytes** outside the targeted changes: use **targeted string + integer replacement**,
    not a JSON parse/`dumps` round-trip (which can reorder keys or reformat numbers/booleans). All
    other bytes identical.
  - **Be atomic**: write each output file to a temp sibling, fsync, then atomically rename over the
    original — never a partial/truncated write.
  - **Be idempotent / resumable**: records already at v14 are left as-is, so a re-run after an
    interrupted run completes the directory rather than refusing it.
  - **Rewrite the whole directory** (mixed v13/v14 dirs otherwise fail closed at runtime).
  - **Refuse on future-schema** (schema > 14).
- **FR-014**: A one-time config migration tool MUST rewrite, in operator TOML, both
  `[risk.capital_pools.sizing_policy]` → `capital_admission_policy` AND the root `schema_version`
  `1` → `2`, preserving comments and other tables (text-based key rewrite, not a toml
  parse/serialize round-trip).
- **FR-015**: The running binary MUST accept only the new schema/names — **no** dual-path runtime
  reader, **no** accept-both TOML key, **no** accept-both root version. Migration is the single
  one-time bridge (NO DUAL PATHS). The sole concession is the below-schema audit-skip legacy literal
  in FR-017, which only *skips* (never reads/recovers) old audit-only records.

**Audit / docs / verifier**

- **FR-016**: The runtime-literal audit allowlist
  (`docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`, 72 hits), the schema
  doc (`docs/bolt-v3/2026-04-25-bolt-v3-schema.md`), and the Python schema verifier
  (`scripts/verify_bolt_v3_schema_current.py` + `scripts/test_verify_bolt_v3_schema_current.py`) MUST
  be updated for the renamed module `path`s, the new string **values**, the new decision-evidence
  `schema_version` 14, and the renamed record kind. Note: `verify_bolt_v3_schema_current.py` extracts
  the version dynamically (`extract_decision_evidence_schema_version`, `:260`), but
  `test_verify_bolt_v3_schema_current.py` and the schema doc carry literal schema/kind strings
  (including a stale `schema_version = 10` fixture at `:72`) that must be made consistent with v14 and
  the new kind name.

**Invariants & keep-list**

- **FR-017**: No trading, admission, reservation, loss-governor, or NT-feed behavior changes. The
  worst-case-liability calculation, reservation arithmetic, freshness checks, lifecycle handling, and
  admit/reject outcomes are identical pre- and post-rename. The following MUST be kept (already
  accurate / required for back-compat):
  - The `bolt_v3_capital_reservation.rs` ledger and its types; `bolt_v3_sizing.rs` /
    `choose_robust_size`; all loss-governor / loss-protection names; `enforce_submit_admission`.
  - Config child keys `min_remaining_pool_balance`, `fee_slippage`, `max_fee_liability`,
    `max_slippage_liability` (accurate; only the parent `sizing_policy` key renames).
  - The serialized record kinds `submit_reservation_metadata` / `submit_reservation_fill` and the
    field `submit_reservation_id`, and all decision-evidence JSON payload field names (no misnomer).
  - The **legacy literal** `"position_sizer_rebuild"` retained (with a deprecation comment) in the
    below-schema non-recovery skip set (`src/bolt_v3_decision_evidence.rs:2005`), so pre-rename
    audit-only records still skip rather than fail closed.
  - The const value `stringify!(nt_order_terminal_event)` (`"nt_order_terminal_event"`) behind the
    renamed `POSITION_SIZER_ORDER_TERMINAL_SOURCE` identifier — the identifier renames, the value
    never changes (no misnomer).

**Fences & guards**

- **FR-018**: The `gated_source_roots.manifest` MUST NOT require changes — the renamed files are not
  listed in any gated source root. The PR verifies this (rather than assumes it).
- **FR-019**: A **repo-wide** misnomer fence MUST be added and wired into CI (`source-fence-static`):
  it greps `src/`, `tests/`, `config/`, `scripts/`, and `docs/` for the misnomer token set
  (`position_sizer`, `PositionSizer`, `PositionSizing`, `position_sizing`, `sizing_policy`,
  `sized_quantity`, `SizedAdmission`, `sizing_state`, `nt_position_sizer`) and fails closed on any hit
  **not** in an explicit, version-controlled **allowlist file**. The allowlist enumerates every
  permitted residual line (the FR-017 legacy skip literal, this spec, the 506 spec, git-history
  prose) — replacing the unenforceable "documented historical references" carve-out. It MUST NOT
  match the legitimate sizer (`choose_robust_size`, `bolt_v3_sizing.rs`).

### Key Entities

- **Capital admission gate** — the renamed component (`bolt_v3_capital_admission.rs`): decides
  admit/reject for one order based on capital, using the reservation ledger.
- **Capital reservation ledger** — unchanged (`bolt_v3_capital_reservation.rs`): the bookkeeping the
  gate drives (`reserve` / `release` / `revalue`).
- **Decision-evidence records** — persisted JSONL; `position_sizer_rebuild` (audit-only) renamed and
  version-bumped; `submit_reservation_metadata` / `submit_reservation_fill` (recovery-critical) kept,
  version-bumped.
- **Capital pool config block** — `[[risk.capital_pools]]` with its renamed `capital_admission_policy`
  sub-block (type `CapitalAdmissionPolicyBlock`); root config at `schema_version = 2`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The repo-wide misnomer fence (FR-019) returns zero matches over `src/`, `tests/`,
  `config/`, `scripts/`, `docs/` except the lines named in its allowlist file — and the fence is a
  CI gate, not a manual check.
- **SC-002**: Exact-head remote CI is green on the PR (full `CI` workflow + Backtester CI +
  actionlint), per `AGENTS.md` remote-first Rust verification.
- **SC-003**: The runtime-literal audit, `scripts/verify_bolt_v3_schema_current.py`, and
  `scripts/test_verify_bolt_v3_schema_current.py` pass with the renamed identifiers/values and
  `schema_version = 14`.
- **SC-004**: A hand-built v13 evidence fixture with known reservation records, migrated by the
  JSONL tool, yields the **same reserved liability per pool** on the renamed binary's recovery as the
  hard-coded expected output derived from the fixture (recovery-equivalence proof, not merely
  parse-success); an un-migrated v13 reservation record fails closed; a legacy below-schema audit
  record is skipped.
- **SC-005**: A config migrated to `capital_admission_policy` + `schema_version = 2` parses to the
  same pool policy values as the pre-rename config; an un-migrated `sizing_policy` config and an
  un-migrated `schema_version = 1` config each fail fast.
- **SC-006**: No behavior test that existed before the rename changes its asserted outcome (only
  identifiers/strings/version change), demonstrating FR-017.

## Assumptions

- **No active deploy.** There is currently no running production node and #529 restart-with-open-
  orders recovery has not been live-exercised, so there is likely no production v13 evidence/config
  requiring migration today. **This does not weaken any migration requirement** — the migration tools
  are the production contract for any future deploy and rewrite the only copy of irreplaceable
  evidence, so atomicity/idempotency/equivalence (FR-013, SC-004) are mandatory regardless. The
  "no active deploy" state is asserted from session context ([[project_bolt_v2_deployed]]); if it is
  ever wrong, the migration gaps become live-outage risks.
- **#658 is merged** (2026-06-15), so the `position_sizer` naming is on `main` and this rename is
  unblocked.
- **Evidence-driven verification** per `AGENTS.md` governs (not mandatory TDD red-green). New code
  (migration tools) gets behavior tests; the mechanical rename is proven by the compiler + repo-wide
  fence + exact-head CI.
- **Golden source-digest gate is removed** — no digest re-derivation is needed (corrects the stale
  #658 note). Verified: `src/bolt_v3_source_integrity.rs` / `source_canonicalization.rs` retain only
  registry-keyed text accessors.
- **Root-config schema versioning is enforced** (corrects the earlier "unconfirmed" draft):
  `SUPPORTED_ROOT_SCHEMA_VERSION: u32 = 1` exists at `src/bolt_v3_validate.rs:108` with a strict `!=`
  check at `:167`. The TOML key rename therefore requires the root version bump in FR-012, not just
  `deny_unknown_fields`.
