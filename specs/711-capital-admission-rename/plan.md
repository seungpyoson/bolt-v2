# Implementation Plan: Rename `position_sizer` → `capital_admission` gate

**Branch**: `docs/711-capital-admission-rename` | **Date**: 2026-06-25 | **Spec**: `specs/711-capital-admission-rename/spec.md`
**Input**: GitHub issue #711 | **Revised** after external review (GPT/Kimi/GLM), adjudicated vs code at HEAD `07db9cb04`.

> **For implementers:** This is a behavior-preserving rename plus a one-time persistence/config
> migration, shipped as **one PR**. Verification is **evidence-driven per `AGENTS.md`**, not mandatory
> TDD red-green: the mechanical rename is proven by the compiler + a repo-wide naming fence +
> `git grep` + exact-head remote CI; the *new* migration tools and the recovery-equivalence check get
> behavior tests. Steps use checkbox (`- [ ]`) syntax. Commit per task for reviewability; the PR is
> evaluated at final head, so a mid-PR commit need not be individually fence-clean — final head MUST be.

**Goal:** Rename the misnamed `position_sizer` submit-admission/capital-reservation gate to
`capital_admission`, on every surface (code, serialized evidence, TOML, root-config version, audit,
docs), with no behavior change, freeing the `position_sizer` name for #712 — in a single debt-free PR.

**Architecture:** One PR. Mechanical identifier renames first (compiler-driven, byte-neutral), then
the serialized-value flips + decision-evidence schema 13→14, then the config key + root-config
schema 1→2, then the two one-time offline migration tools, then the repo-wide fence + audit/doc/
verifier updates. No dual runtime path; migration is the one-time bridge.

**Tech Stack:** Rust (pure `LiveNode`, NT Rust API), TOML config (serde, `deny_unknown_fields`),
Python 3.12 verifiers/migration tools, Ubicloud/GitHub Actions CI.

## Global Constraints

Copied from `AGENTS.md` (every task implicitly includes these):

- **NO HARDCODES** — runtime values come from TOML; do not introduce string literals for runtime values.
- **NO DUAL PATHS** — one reader, one config key, one schema version each. Migration is a one-time bridge, not a runtime accept-both. The only residual old literal is the below-schema audit-*skip* legacy string (it never reads/recovers state).
- **NO DEBTS** — no TODO / "fix later"; no half-rename left on `main` (this is why it is one PR).
- **STRATEGIES PRODUCE INTENT ONLY** — shared submit/admission code; no strategy-local submit mechanics change (only identifier references update).
- **Remote-first Rust verification** — local non-compile gates only (`just fmt-check`, `just source-fence-static`, `just ci-lint-workflow`, Python verifiers/tests); Rust compile/test proof via `just verify-remote` exact-head CI on a draft PR. Do not run local compile-heavy cargo.
- **Review Bar** — open the PR and request review from the GitHub account with node ID `U_kgDOEZMFhA`; do not merge without its approval; do not request external review until exact-head CI is green.
- **Evidence per requirement** — refactor evidence = existing tests + static checks + structural-equivalence review; new code (migration tools, equivalence test) = behavior tests; persisted/config contract changes = fail-closed evidence for invalid/missing/legacy inputs + exact-head proof.

## Technical Context

**Language/Version**: Rust (repo-pinned toolchain); Python 3.12 on CI (test migration/verifier scripts with `python3.12`).
**Primary Dependencies**: NautilusTrader Rust API; serde/serde_json; `aws-sdk-ssm` (unaffected).
**Storage**: Decision-evidence JSONL files (schema-versioned); operator TOML config (root `schema_version`).
**Testing**: Exact-head remote PR CI (`just verify-remote`) for Rust; `pytest`/`python3.12` for migration + verifier scripts; `just fmt-check`, `just source-fence-static`, runtime-literal audit, `scripts/verify_bolt_v3_schema_current.py` locally.
**Target Platform**: Linux server node.
**Project Type**: Single Rust project (compiler/trading runtime) with Python tooling.
**Constraints**: Behavior-preserving; fail-closed on invalid/missing/legacy inputs; exact-head CI green before review.
**Scale/Scope**: ~30 files touched; ~768 misnomer hits (src 507 / tests 261 / docs 75 / scripts 6, measured at `07db9cb04`); 4 serialized string values + decision-evidence schema bump + TOML key + root-config schema bump; 2 migration tools; 1 repo-wide fence.

## Constitution Check

*GATE: must hold before and after design.*

| Gate | Status / how this plan satisfies it |
|------|--------------------------------------|
| NO HARDCODES | No new runtime literals; only renaming existing ones. |
| NO DUAL PATHS | Runtime reads only v14 / `capital_admission_policy` / root v2; migration is one-time offline; the only old literal is the audit-*skip* legacy string. |
| NO DEBTS | One PR — no half-rename on `main`; repo-wide fence prevents regression; no TODOs. |
| Strategies = intent only | No `src/strategies/*` submit-mechanics change (only identifier references + audit-stub names update). |
| Remote-first Rust verify | Compile/test proof via exact-head CI only. |
| Review Bar | PR + required reviewer node `U_kgDOEZMFhA`. |

No constitution violations → Complexity Tracking is empty.

## Project Structure

```text
specs/711-capital-admission-rename/
├── spec.md      # requirements + acceptance (this feature)
└── plan.md      # this file

# Renamed source modules (git mv) — Task 1/2/3
src/bolt_v3_position_sizer.rs               -> src/bolt_v3_capital_admission.rs
src/bolt_v3_position_sizer_runtime_feed.rs  -> src/bolt_v3_capital_admission_runtime_feed.rs
src/bolt_v3_sizing_state.rs                 -> src/bolt_v3_capital_admission_state.rs
tests/bolt_v3_position_sizer_runtime_feed.rs -> tests/bolt_v3_capital_admission_runtime_feed.rs
src/lib.rs                                  (mod declarations)

# Modified for identifier references — Task 4 (counts at 07db9cb04)
src/bolt_v3_submit_admission.rs (267), src/bolt_v3_live_node.rs (85),
src/bolt_v3_decision_evidence.rs (22), src/bolt_v3_order_execution.rs (15),
src/bolt_v3_validate.rs (8), src/bolt_v3_config.rs, src/bolt_v3_loss_protection.rs,
src/strategies/registry.rs, src/strategies/binary_oracle_maker/mod.rs,
src/strategies/binary_oracle_edge_taker/tests/{shared_fixture,orders_admission,core_glue}.rs,
crates/backtesting-vertical-slice/src/runner.rs,
tests/bolt_v3_basket_admission.rs (47), tests/bolt_v3_submit_admission.rs (10),
tests/bolt_v3_decision_evidence.rs (6), tests/support/mod.rs, and the remaining tests/*.rs hits.

# Modified for serialized values / schema — Task 5
src/bolt_v3_decision_evidence.rs (string VALUES + SCHEMA_VERSION 13->14 + read-dispatch arms + legacy skip literal)
src/bolt_v3_submit_admission.rs   (manual outcome match arm :138)

# Modified for config key + root schema — Task 6
src/bolt_v3_config.rs   (field sizing_policy -> capital_admission_policy; type CapitalPoolSizingPolicyBlock -> CapitalAdmissionPolicyBlock)
src/bolt_v3_validate.rs (SUPPORTED_ROOT_SCHEMA_VERSION 1 -> 2)
config/**/*.toml, tests/fixtures/bolt_v3/*.toml (sizing_policy + schema_version), tests/config_parsing.rs

# Modified for audit/docs/verifier — Task 10
docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml (72)
docs/bolt-v3/2026-04-25-bolt-v3-schema.md
scripts/verify_bolt_v3_schema_current.py, scripts/test_verify_bolt_v3_schema_current.py

# New files — Tasks 7/8/9
scripts/migrate_bolt_v3_decision_evidence_v13_to_v14.py            (+ test_…)
scripts/migrate_bolt_v3_capital_admission_config.py               (+ test_…)
scripts/check_no_position_sizer_misnomer.py                        (repo-wide fence)
specs/711-capital-admission-rename/misnomer-allowlist.txt          (fence allowlist)
tests/bolt_v3_capital_admission_recovery_equivalence.rs           (SC-004 Rust equivalence test)
tests/fixtures/bolt_v3/capital_admission_recovery/{v13_*.jsonl}    (hand-built equivalence fixture)
```

**Structure Decision:** Single Rust project; renamed files keep their directory. Renamed files are
**not** in any gated source root — Task 0 verifies this, so `gated_source_roots.manifest` needs no
change (FR-018).

---

## Task 0: Pre-flight verification

**Files:** none (read-only).

- [ ] **Step 1 (FR-018):** Confirm renamed files are not gated source roots.
  Run: `git grep -nE 'bolt_v3_position_sizer|bolt_v3_sizing_state' -- gated_source_roots.manifest`
  Expected: no output.
- [ ] **Step 2 (FR-012):** Re-confirm the root-config version const + check still present at head.
  Run: `git grep -n 'SUPPORTED_ROOT_SCHEMA_VERSION' -- src/bolt_v3_validate.rs`
  Expected: `:108` const `= 1` and `:167` `!=` check.
- [ ] **Step 3:** Snapshot the misnomer surface for completion-diffing.
  Run: `git grep -cniE 'position_siz|sizing_policy|sized_quantity|sizedadmission|sizing_state|nt_position_sizer' -- src tests config scripts docs | sort -t: -k2 -rn`
  Save the per-file counts; the final head must reduce these to allowlisted lines only.

## Task 1: Rename the gate core module + identifiers

**Files:**
- Rename: `src/bolt_v3_position_sizer.rs` → `src/bolt_v3_capital_admission.rs` (`git mv`)
- Modify: `src/lib.rs` (`pub mod bolt_v3_position_sizer;` → `pub mod bolt_v3_capital_admission;`); every referencing module (compiler enumerates).

**Interfaces — Produces:** module `bolt_v3_capital_admission`; `CapitalAdmissionGate`,
`evaluate_capital_admission(...)`, `CapitalAdmissionPolicy`, `LiabilityQuote` (kept) with renamed
fields `accepted_quantity` / `calculated_liability` / `reserved_liability`.

**Rename rule (gate context):** `PositionSizing*`/`PositionSizer*`/`Sizing*`(gate-context)/`SizedAdmission*`
→ `CapitalAdmission*`. Anchor renames: `PositionSizingAdmissionGate` → `CapitalAdmissionGate`;
`evaluate_position_sizing` → `evaluate_capital_admission`; `PositionSizingRequest` →
`CapitalAdmissionRequest`; `PositionSizingInputs`/`PositionSizingGateInputs` →
`CapitalAdmissionInputs`/`CapitalAdmissionGateInputs`;
`PositionSizingLifecycle*`/`PositionSizingRebuildDecision` →
`CapitalAdmissionLifecycle*`/`CapitalAdmissionRebuildDecision`; `SizingPolicy` →
`CapitalAdmissionPolicy`; `ProductSizingSnapshot`/`PredictionMarketSizingSnapshot` →
`ProductAdmissionSnapshot`/`PredictionMarketAdmissionSnapshot`;
`SizingEvidenceKind`/`SizingEvidenceSource` →
`CapitalAdmissionEvidenceKind`/`CapitalAdmissionEvidenceSource`;
`SizedAdmissionDecision`/`SizedAdmissionEvidence`/`SizedAdmissionReason` →
`CapitalAdmissionDecision`/`CapitalAdmissionEvidence`/`CapitalAdmissionReason`.
**Keep (FR-004):** `FeeSlippagePolicy`, `LiabilityQuote`/`LiabilityError`.

- [ ] **Step 1:** `git mv src/bolt_v3_position_sizer.rs src/bolt_v3_capital_admission.rs`
- [ ] **Step 2:** Apply the rule inside the moved file and `src/lib.rs`. **Do not** touch any `&str`
  literal value, the `schema_version` integer, the TOML field, or the `BoltV3AdmissionOutcome` variant
  yet (those are Tasks 5/6).
- [ ] **Step 3 (FR-002 internal fields):** Rename `sized_quantity` → `accepted_quantity`;
  `liability_before_sizing` → `calculated_liability`; `liability_after_sizing` → `reserved_liability`
  in `LiabilityQuote`, `CapitalAdmissionDecision`, `CapitalAdmissionEvidence` and all
  constructors/readers (verified: these structs carry no `Serialize` derive → internal-only).
- [ ] **Step 4:** Local static gates. Run: `just fmt-check && just source-fence-static`. Expected: pass.
- [ ] **Step 5:** Commit. `git commit -m "refactor(711): rename position_sizer gate core -> capital_admission"`

## Task 2: Rename the runtime-feed module + identifiers + its test file

**Files:**
- Rename: `src/bolt_v3_position_sizer_runtime_feed.rs` → `src/bolt_v3_capital_admission_runtime_feed.rs`;
  `tests/bolt_v3_position_sizer_runtime_feed.rs` → `tests/bolt_v3_capital_admission_runtime_feed.rs`
- Modify: `src/lib.rs`; referencing modules.

Anchor renames: `PositionSizerRuntimeFeed` → `CapitalAdmissionRuntimeFeed`;
`PositionSizerRuntimeFeedConfig`/`...Subscription`/`...ComponentBuilder` → `CapitalAdmission*`;
`subscribe_position_sizer_runtime_feed` → `subscribe_capital_admission_runtime_feed`;
const identifier `POSITION_SIZER_ORDER_TERMINAL_SOURCE` → `CAPITAL_ADMISSION_ORDER_TERMINAL_SOURCE`.
**Note (FR-017):** that const's value is `stringify!(nt_order_terminal_event)` = `"nt_order_terminal_event"`
— the identifier renames; **the value never changes** and is not a misnomer. Do not introduce a string
literal for it.

- [ ] **Step 1:** `git mv` both files; apply rule; update `src/lib.rs` and references.
- [ ] **Step 2:** `just fmt-check && just source-fence-static` → pass.
- [ ] **Step 3:** Commit. `git commit -m "refactor(711): rename position_sizer runtime feed (+test) -> capital_admission"`

## Task 3: Rename the input-state module + identifiers

**Files:**
- Rename: `src/bolt_v3_sizing_state.rs` → `src/bolt_v3_capital_admission_state.rs`
- Modify: `src/lib.rs`; referencing modules.

Anchor renames: `NtDerivedSizingState` → `NtDerivedCapitalAdmissionState`;
`PortfolioSizingSnapshot`/`OrderLifecycleSizingSnapshot` → `Portfolio…`/`OrderLifecycle…` with
`Sizing`→`CapitalAdmission`;
`SizingStateError`/`SizingStateEvidence`/`SizingStateEvidenceKind`/`SizingStateEvidenceSource` →
`CapitalAdmissionStateError`/`...Evidence`/`...EvidenceKind`/`...EvidenceSource`;
`validate_nt_derived_sizing_state` → `validate_nt_derived_capital_admission_state`.
**Keep (FR-004):** `VenueSpendabilitySnapshot`, `ReservationLedgerSnapshot`.

- [ ] **Step 1:** `git mv` the file; apply rule; update `src/lib.rs` and references.
- [ ] **Step 2:** `just fmt-check && just source-fence-static` → pass.
- [ ] **Step 3:** Commit. `git commit -m "refactor(711): rename sizing_state -> capital_admission_state"`

## Task 4: Rename embedding field + submit-admission / live-node / validate / strategies / tests identifiers

**Files:** `src/bolt_v3_submit_admission.rs`, `src/bolt_v3_live_node.rs`, `src/bolt_v3_validate.rs`,
`src/bolt_v3_decision_evidence.rs` (identifier NAMES only — values in Task 5),
`src/bolt_v3_order_execution.rs`, `src/bolt_v3_loss_protection.rs`, `src/strategies/registry.rs`,
`src/strategies/binary_oracle_maker/mod.rs`,
`src/strategies/binary_oracle_edge_taker/tests/{shared_fixture,orders_admission,core_glue}.rs`,
`crates/backtesting-vertical-slice/src/runner.rs`, `tests/*.rs`, `tests/support/mod.rs`.

Anchor renames:
- `BoltV3SubmitAdmissionInner.position_sizer` field → `capital_admission` (FR-003).
- `BoltV3SubmitPositionSizerState`/`Config`/`...NtComponents`/`...OpenOrder*`/`...Rebuild*`/`...FillUpdate`/`...Lifecycle*`
  → `BoltV3SubmitCapitalAdmission*`; methods `new_with_position_sizer`/`new_with_loss_governor_and_position_sizer`/`position_sizer_*`/`apply_position_sizing_*`/`rebuild_position_sizing_*`/`finish_position_sizer_rebuild`
  → `*_capital_admission` / `apply_capital_admission_*` / `rebuild_capital_admission_*`.
- Request/claim field + type (internal, **not serialized** — verified no `Serialize` derive, zero
  `"position_sizing"` JSON keys): `position_sizing: Option<BoltV3CompiledOrderSizingEvidence>`
  (`submit_admission.rs:2706`, `:2829`) → `admission_evidence: Option<BoltV3CompiledOrderAdmissionEvidence>`;
  rename `BoltV3CompiledOrderSizingEvidence` → `BoltV3CompiledOrderAdmissionEvidence`.
- `BoltV3SubmitAdmissionError::PositionSizingRejected` → `CapitalAdmissionRejected`;
  `BoltV3PositionSizerRejectReason` + variants (`SizingRejected`/`MissingSizingEvidence`/`SizedQuantityMismatch`)
  → `BoltV3CapitalAdmissionRejectReason` + (`CapitalAdmissionRejected`/`MissingAdmissionEvidence`/`AcceptedQuantityMismatch`);
  `BoltV3PositionSizerReservationRollback`/`BoltV3PositionSizerSubmitDecision` →
  `BoltV3CapitalAdmissionReservationRollback`/`BoltV3CapitalAdmissionSubmitDecision`.
- `src/bolt_v3_decision_evidence.rs` internal NAMES (values stay until Task 5):
  `BoltV3PositionSizerRebuildAuditEvidence` → `BoltV3CapitalAdmissionRebuildAuditEvidence`;
  `PositionSizerRebuildAuditLine[Owned]` → `CapitalAdmissionRebuildAuditLine[Owned]`;
  `record_position_sizer_rebuild_audit` → `record_capital_admission_rebuild_audit`;
  `encode_position_sizer_rebuild_audit_line` → `encode_capital_admission_rebuild_audit_line`;
  const identifiers `BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID`/`_RECORD_KIND` →
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID`/`_RECORD_KIND` **(keep `&str` VALUES until Task 5).**
- `src/bolt_v3_live_node.rs`: struct `BoltV3PositionSizerVenueSpendabilitySourceConfig` →
  `BoltV3CapitalAdmissionVenueSpendabilitySourceConfig`; error variant `StartupPositionSizerRebuild` →
  `StartupCapitalAdmissionRebuild`; `sizing_policy_from_pool` → `capital_admission_policy_from_pool`;
  `position_sizer_*` accessors/builders/open-order-rebuild helpers → `capital_admission_*`.
- `src/bolt_v3_validate.rs`: `validate_position_sizer_recovery_evidence` →
  `validate_capital_admission_recovery_evidence`; validation label identifier strings (the persisted
  label value `nt_position_sizer_runtime_components` is flipped in Task 5).
- Strategy audit stubs `record_position_sizer_rebuild_audit` (maker/mod.rs, edge_taker tests) +
  test fn names + `tests/support/mod.rs` helpers → `*_capital_admission_*`.
- **Defer to Task 5:** the `BoltV3AdmissionOutcome::RejectedPositionSizing` variant (its rename
  auto-flips the serde value, so it belongs with the serialized batch) and the manual outcome match
  arm at `submit_admission.rs:138`.

- [ ] **Step 1 (FR-005):** Apply the renames above. Update every user-visible string tied to a renamed
  identifier in the same edit — `StartupCapitalAdmissionRebuild`'s `Display` text, `anyhow!`/`bail!`
  messages, log lines — since the compiler updates patterns but not string literals.
- [ ] **Step 2:** `just fmt-check && just source-fence-static` → pass.
- [ ] **Step 3:** Commit. `git commit -m "refactor(711): rename position_sizer references in submit-admission/live-node/validate/strategies/tests"`

## Task 5: Flip serialized string VALUES + bump decision-evidence schema + retain legacy skip literal

**Files:** `src/bolt_v3_decision_evidence.rs`, `src/bolt_v3_submit_admission.rs`.

- [ ] **Step 1 (FR-008 — variant + both string sites):** Rename the outcome variant
  `BoltV3AdmissionOutcome::RejectedPositionSizing` → `RejectedCapitalAdmission`. This auto-changes the
  serde-serialized value to `"rejected_capital_admission"` via `rename_all` (`decision_evidence.rs:961`).
  Then **manually** change the RHS string literal in the match arm at `submit_admission.rs:138`
  (`=> "rejected_position_sizing"` → `=> "rejected_capital_admission"`), which the compiler does not
  auto-update, and update the round-trip test expected-value table at `decision_evidence.rs:3531-3556`.
- [ ] **Step 2 (FR-006/007/009):** Change the kept-name const VALUES:
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND` `"position_sizer_rebuild"` → `"capital_admission_rebuild"`;
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID` `"bolt_v3.position_sizer_rebuild"` → `"bolt_v3.capital_admission_rebuild"`;
  the evidence source label `"nt_position_sizer_runtime_components"` → `"nt_capital_admission_runtime_components"`.
  Update the read-dispatch `match` arm string literals at `:1510` and `:1784` to the new kind value.
- [ ] **Step 3 (FR-017 — legacy skip literal):** In
  `decision_evidence_header_is_below_current_schema_non_recovery_record` (`:1995-2006`), retain the
  **legacy literal** `"position_sizer_rebuild"` in the matched set alongside the new
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND`, with a deprecation comment — so pre-rename
  audit-only records still skip rather than fail closed (spec Edge Cases / SC-004).
- [ ] **Step 4 (FR-010):** Bump `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` 13 → 14 (`:23`).
- [ ] **Step 5:** Update Rust unit tests asserting the old literals/version to the new values; keep the
  below-schema audit-skip + reservation fail-closed tests and confirm they still encode the design at
  `decision_evidence.rs:1995-2006, 2284`. Add a test that a record with the **legacy** kind
  `"position_sizer_rebuild"` at schema 13 is classified skippable (not fail-closed).
- [ ] **Step 6:** `just fmt-check && just source-fence-static` → pass. Commit.
  `git commit -m "feat(711): flip serialized capital_admission values + bump evidence schema 13->14 + keep legacy audit-skip literal"`

## Task 6: Rename TOML key + type + bump root-config schema version

**Files:** `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, `config/**/*.toml`,
`tests/fixtures/bolt_v3/*.toml`, `tests/config_parsing.rs`.

- [ ] **Step 1 (FR-011):** Rename the `CapitalPoolBlock` field `sizing_policy` →
  `capital_admission_policy` (`:243`) and the block type `CapitalPoolSizingPolicyBlock` →
  `CapitalAdmissionPolicyBlock` (`:256`); block stays under `[[risk.capital_pools]]`,
  `deny_unknown_fields` intact. Update doc-comment prose naming the component. Keep child keys
  `min_remaining_pool_balance`, `fee_slippage`, `max_fee_liability`, `max_slippage_liability`.
- [ ] **Step 2 (FR-012):** Bump `SUPPORTED_ROOT_SCHEMA_VERSION` 1 → 2 (`src/bolt_v3_validate.rs:108`).
  Update the `tests/config_parsing.rs` assertion `root.schema_version == 1` → `== 2`.
- [ ] **Step 3:** Update every config under `config/` and `tests/fixtures/`:
  `[risk.capital_pools.sizing_policy]` → `[risk.capital_pools.capital_admission_policy]` and root
  `schema_version = 1` → `schema_version = 2`.
- [ ] **Step 4:** `just fmt-check && just source-fence-static` → pass. Commit.
  `git commit -m "feat(711): rename sizing_policy TOML key/type + bump root-config schema 1->2"`

## Task 7: JSONL evidence migration tool + recovery-equivalence test (new code, TDD)

**Files:**
- Create: `scripts/migrate_bolt_v3_decision_evidence_v13_to_v14.py` + `scripts/test_migrate_bolt_v3_decision_evidence_v13_to_v14.py`
- Create: `tests/bolt_v3_capital_admission_recovery_equivalence.rs` + fixtures under
  `tests/fixtures/bolt_v3/capital_admission_recovery/`

**Interfaces — Produces:** a CLI `migrate_bolt_v3_decision_evidence_v13_to_v14.py <dir>` that, per
file, applies targeted replacements (record-kind / gate-id / outcome / source-label string values per
FR-006/007/008/009) and sets each envelope's `schema_version` to 14 — **including**
`submit_reservation_metadata` / `submit_reservation_fill` — via byte-targeted string/integer
substitution (no JSON round-trip), writing atomically (temp + fsync + os.replace), idempotently
(records already at v14 untouched), over the whole directory, refusing schema > 14.

- [ ] **Step 1:** Write failing Python tests:
  (a) a v13 fixture with one `position_sizer_rebuild` (audit) record, one `submit_reservation_metadata`
  (recovery) record, and one `admission_decision` record whose `outcome` is `"rejected_position_sizing"`
  → after migration: kind/outcome/source-label values renamed, `submit_reservation_metadata` kind
  unchanged but `schema_version` now 14, and **every other byte (key order, number formatting)
  identical** (assert exact line bytes for unaffected fields);
  (b) re-running on the migrated dir is a no-op (idempotent);
  (c) a dir already containing a v14 record is completed, not refused (resumable);
  (d) a schema-15 record causes refusal.
  Run: `python3.12 -m pytest scripts/test_migrate_bolt_v3_decision_evidence_v13_to_v14.py -v` → FAIL.
- [ ] **Step 2:** Implement the migrator (targeted substitution + `tempfile` + `os.fsync` +
  `os.replace`; per-record version guard for idempotency; pre-scan to refuse future-schema).
- [ ] **Step 3:** Run tests → PASS.
- [ ] **Step 4 (SC-004 — Rust recovery equivalence):** Add `tests/bolt_v3_capital_admission_recovery_equivalence.rs`:
  check in a hand-built v13 fixture dir representing known per-pool reservations; in the test, migrate
  it (invoke the tool or check in the paired migrated v14 fixture), load it through the renamed
  recovery path, and assert the recovered **reserved liability per pool** equals hard-coded expected
  values derived by hand from the fixture payloads (which are unchanged by migration). Add the two
  fail-closed/skip assertions: an un-migrated v13 reservation record → fail closed; a v13 legacy
  `position_sizer_rebuild` audit record → skipped.
- [ ] **Step 5:** Commit. `git commit -m "feat(711): atomic idempotent v13->v14 evidence migrator + recovery-equivalence test"`

## Task 8: Config migration tool (new code, TDD)

**Files:**
- Create: `scripts/migrate_bolt_v3_capital_admission_config.py` + `scripts/test_migrate_bolt_v3_capital_admission_config.py`

- [ ] **Step 1:** Write failing test: a TOML with `[risk.capital_pools.sizing_policy]` and root
  `schema_version = 1` → after migration the table is `capital_admission_policy` with identical child
  values, root `schema_version = 2`, and comments / other tables byte-preserved.
  Run: `python3.12 -m pytest scripts/test_migrate_bolt_v3_capital_admission_config.py -v` → FAIL.
- [ ] **Step 2:** Implement (text-based key + version rewrite preserving comments; not a toml
  parse/serialize round-trip).
- [ ] **Step 3:** Run tests → PASS. Commit.
  `git commit -m "feat(711): one-time sizing_policy->capital_admission_policy + root schema 1->2 config migrator"`

## Task 9: Repo-wide misnomer fence + allowlist, wired into CI

**Files:**
- Create: `scripts/check_no_position_sizer_misnomer.py`, `specs/711-capital-admission-rename/misnomer-allowlist.txt`
- Modify: the `source-fence-static` recipe (`justfile`) + the CI workflow step that runs it.

**Interfaces — Produces:** a fail-closed CLI that greps `src/`, `tests/`, `config/`, `scripts/`,
`docs/` for the token set `position_sizer|PositionSizer|PositionSizing|position_sizing|sizing_policy|sized_quantity|SizedAdmission|sizing_state|nt_position_sizer`
and exits non-zero on any hit whose `path:line` is not listed in the allowlist file. It MUST NOT
match the legitimate sizer (`choose_robust_size`, `bolt_v3_sizing.rs`) — those tokens are not in the
set; if any incidental overlap occurs, list it in the allowlist with a justification comment.

- [ ] **Step 1:** Write the allowlist file: the FR-017 legacy skip literal line in
  `decision_evidence.rs`, this spec, the 506 spec, and any deliberate historical-prose line — each
  with a one-line justification. No code identifiers permitted.
- [ ] **Step 2:** Write the fence script (exit non-zero on any non-allowlisted hit; print the
  offending `path:line`).
- [ ] **Step 3:** Run it. Run: `python3.12 scripts/check_no_position_sizer_misnomer.py`. Expected:
  pass (zero non-allowlisted hits) once Tasks 1–8 + 10 are done; if it flags a residual, fix the
  residual (do not add code identifiers to the allowlist).
- [ ] **Step 4:** Wire it into `source-fence-static` and the CI lint step so the fence is a gate
  (SC-001). Run: `just source-fence-static` → pass.
- [ ] **Step 5:** Commit. `git commit -m "chore(711): add repo-wide position_sizer misnomer fence + allowlist + CI wiring"`

## Task 10: Update runtime-literal audit, schema doc, Python schema verifier

**Files:** `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`,
`docs/bolt-v3/2026-04-25-bolt-v3-schema.md`, `scripts/verify_bolt_v3_schema_current.py`,
`scripts/test_verify_bolt_v3_schema_current.py`.

- [ ] **Step 1 (FR-016):** Update every audit entry whose `path` is a renamed module file, every
  `classification` value `position_sizer_*` → `capital_admission_*`, and every `literal`/`context`
  line whose identifier text or string **value** changed.
- [ ] **Step 2 (FR-016):** Update the schema doc to the new record kind + `schema_version` 14; update
  `verify_bolt_v3_schema_current.py` + `test_verify_bolt_v3_schema_current.py` so their literal
  kind/version strings (including the stale `schema_version = 10` fixture at
  `test_verify_…:72`) are consistent with v14 and the renamed kind.
- [ ] **Step 3 (SC-003):** Run `python3.12 scripts/verify_bolt_v3_schema_current.py` + the
  runtime-literal audit recipe + `python3.12 -m pytest scripts/test_verify_bolt_v3_schema_current.py -v`
  → pass.
- [ ] **Step 4:** Commit. `git commit -m "chore(711): update audit/schema-doc/verifier for capital_admission serialized names + schema 14"`

## Task 11: Remote proof + PR

- [ ] **Step 1:** Re-run the Task 0 snapshot grep over final head; confirm only allowlisted lines
  remain. Run: `python3.12 scripts/check_no_position_sizer_misnomer.py` → pass.
- [ ] **Step 2:** Push branch; open a PR titled `#711: rename position_sizer -> capital_admission gate`.
  Body: bullets covering the rename surfaces, the decision-evidence schema 13→14 + root-config 1→2
  bumps, the two migration tools and **the exact operator steps** (run both migration tools before
  starting the renamed binary; fail-closed behavior if skipped), and `Closes #711` is **omitted** per
  the no-close-keyword rule — link with `Refs #711` / `Blocks #712` instead.
- [ ] **Step 3:** `just verify-remote` → full `CI` + Backtester CI + actionlint green on exact head.
  Evidence: pre-existing behavior tests pass unchanged (FR-017 / SC-006); migration + equivalence
  tests green (FR-013/014 / SC-004); fence green (SC-001); schema verifier + audit green (SC-003).
- [ ] **Step 4:** Mark ready; request review from GitHub node `U_kgDOEZMFhA`. Do not merge without
  approval (the user owns the merge decision).

---

## Risk / Complexity Tracking

| Risk | Mitigation | Evidence |
|------|-----------|----------|
| Missed identifier leaves an inconsistent build / residual misnomer | Compiler + **repo-wide** fence (Task 9) over src/tests/config/scripts/docs with allowlist; Task 0 snapshot diff | exact-head CI + fence exit code (SC-001) |
| Outcome string missed at the manual match arm (`submit_admission.rs:138`) — compiler won't catch RHS literal | Task 5 Step 1 changes both the serde-driven value and the manual arm + round-trip test | round-trip test asserts `outcome` byte (`decision_evidence.rs:3531-3556`) |
| Old audit records fail closed after kind-value flip | Retain legacy `"position_sizer_rebuild"` literal in the below-schema skip set (Task 5 Step 3) | Task 5 Step 5 legacy-skip test + SC-004 |
| Schema bump breaks startup reservation recovery | Recovery rides on un-renamed `submit_reservation_metadata/fill` payloads; migrator rewrites whole dir; un-migrated input fails closed by design (`decision_evidence.rs:2284`) | Task 7 Rust recovery-equivalence + fail-closed tests (SC-004) |
| Migration corrupts the only evidence copy (reorder/truncate) | Targeted byte substitution (no JSON round-trip) + atomic temp/fsync/rename + idempotent/resumable + refuse future-schema (FR-013) | Task 7 byte-identity + idempotency + refusal tests |
| Root-config key rename silently accepted on old configs | Bump `SUPPORTED_ROOT_SCHEMA_VERSION` 1→2 + config migrator (Task 6/8); `deny_unknown_fields` is secondary | Task 6 config_parsing assertion + Task 8 migrator test (SC-005) |
| Audit `path`/`context`/`classification` drift fails CI | Audit updated for paths+identifiers+values in one pass (Task 10) | runtime-literal audit recipe + schema verifier green (SC-003) |
| Operator skips migration | Currently theoretical (no active deploy) but **not** weakened; fail-closed not silent; PR body documents the required steps | spec Assumptions + Edge Cases + Task 11 PR body |

## Self-Review (spec coverage)

- FR-001 → Tasks 1/2/3 (modules + test-file rename). FR-002 → Tasks 1–4 (rule + verified surfaces).
  FR-003 → Task 4. FR-004 (keep) → scope notes in Tasks 1/3. FR-005 (strings) → Task 4 Step 1.
  FR-006/007/009 → Task 5 Step 2. FR-008 (two sites) → Task 5 Step 1. FR-010 → Task 5 Step 4.
  FR-011 → Task 6 Steps 1/3. FR-012 (root version) → Task 6 Step 2. FR-013 → Task 7. FR-014 → Task 8.
  FR-015 (no dual path) → Tasks 5/6 (single reader) + 7/8 (offline migration). FR-016 → Task 10.
  FR-017 (invariants + keep-list + legacy skip literal + terminal-source value) → Tasks 1/3 scope +
  Task 5 Step 3 + Task 2 note. FR-018 → Task 0 Step 1. FR-019 (repo-wide fence) → Task 9.
- SC-001 → Task 9/11 fence gate. SC-002 → Task 11. SC-003 → Task 10. SC-004 → Task 7 Step 4.
  SC-005 → Tasks 6/8. SC-006 → Task 11 evidence.
- Placeholder scan: none (every step has a command or concrete edit). Type-consistency: renamed
  types used in later tasks (`CapitalAdmissionGate`, `BoltV3CompiledOrderAdmissionEvidence`,
  `CapitalAdmissionPolicyBlock`, `BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND`) match their
  defining tasks. No spec requirement is unmapped.
