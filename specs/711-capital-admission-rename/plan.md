# Implementation Plan: Rename `position_sizer` → `capital_admission` gate

**Branch**: `docs/711-capital-admission-rename` | **Date**: 2026-06-25 | **Spec**: `specs/711-capital-admission-rename/spec.md`
**Input**: GitHub issue #711

> **For implementers:** This is a behavior-preserving rename plus a one-time persistence/config
> migration. Verification is **evidence-driven per `AGENTS.md`**, not mandatory TDD red-green: the
> mechanical rename is proven by the compiler + a naming guard + `git grep` + exact-head remote CI;
> the *new* migration tools (Slice 2) get unit tests. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Rename the misnamed `position_sizer` submit-admission/capital-reservation gate to
`capital_admission`, on every surface (code, serialized evidence, TOML, audit, docs), with no
behavior change, freeing the `position_sizer` name for #712.

**Architecture:** Two PRs under one issue. **Slice 1** renames all Rust identifiers, the three
module files, and internal (non-serialized) fields while keeping every serialized string *value*,
`schema_version`, and TOML key unchanged — so it changes zero persisted/operator bytes and is
independently mergeable and behavior-neutral. **Slice 2** flips the serialized string values, bumps
`SCHEMA_VERSION` 13→14, renames the TOML key, ships one-time migration tools, and updates the
runtime-literal audit / schema verifier / docs.

**Tech Stack:** Rust (pure `LiveNode`, NT Rust API), TOML config (serde, `deny_unknown_fields`),
Python verifiers/migration scripts, Ubicloud/GitHub Actions CI.

## Global Constraints

Copied from `AGENTS.md` (every task implicitly includes these):

- **NO HARDCODES** — runtime values come from TOML; do not introduce string literals for runtime values.
- **NO DUAL PATHS** — one reader, one config key, one schema version. Migration is a one-time bridge, not a runtime accept-both.
- **NO DEBTS** — no TODO / "fix later"; a half-rename (Slice 1 without Slice 2) must not be the final state on `main`.
- **STRATEGIES PRODUCE INTENT ONLY** — this is shared submit/admission code; no strategy-local submit mechanics change.
- **Remote-first Rust verification** — local non-compile gates only (`just fmt-check`, `just source-fence-static`, `just ci-lint-workflow`, Python verifiers); Rust compile/test proof via `just verify-remote` exact-head CI on a (draft) PR. Do not run local compile-heavy cargo.
- **Review Bar** — open a PR per slice and request review from the GitHub account with node ID `U_kgDOEZMFhA`; do not merge without its approval; do not request external review until exact-head CI is green.
- **Evidence per requirement** — refactor evidence = existing tests + static checks + structural-equivalence review; new code (migration tools) = behavior tests; persisted/config contract changes = fail-closed evidence for invalid/missing inputs + exact-head proof.

## Technical Context

**Language/Version**: Rust (repo-pinned toolchain); Python 3.12 on CI (test migration scripts with `python3.12`).
**Primary Dependencies**: NautilusTrader Rust API; serde/serde_json; `aws-sdk-ssm` (unaffected).
**Storage**: Decision-evidence JSONL files (schema-versioned); operator TOML config.
**Testing**: Exact-head remote PR CI (`just verify-remote`) for Rust; `pytest`/`python3.12` for migration scripts; `just fmt-check`, `just source-fence-static`, runtime-literal audit, `scripts/verify_bolt_v3_schema_current.py` locally.
**Target Platform**: Linux server node.
**Project Type**: Single Rust project (compiler/trading runtime) with Python tooling.
**Constraints**: Behavior-preserving; fail-closed on invalid/missing/legacy inputs; exact-head CI green before review.
**Scale/Scope**: ~24 files touched; ~80 identifiers; 4 serialized strings + 1 schema bump + 1 TOML key; 2 migration scripts.

## Constitution Check

*GATE: must hold before and after design.*

| Gate | Status / how this plan satisfies it |
|------|--------------------------------------|
| NO HARDCODES | No new runtime literals; only renaming existing ones. |
| NO DUAL PATHS | Runtime reads only v14 / `capital_admission_policy`; migration is one-time offline. |
| NO DEBTS | Slice 2 mandatory; naming guard prevents regression; no TODOs. |
| Strategies = intent only | No `src/strategies/*` submit-mechanics change (only identifier references update). |
| Remote-first Rust verify | Compile/test proof via exact-head CI only. |
| Review Bar | PR + required reviewer node `U_kgDOEZMFhA` per slice. |

No constitution violations → Complexity Tracking is empty.

## Project Structure

```text
specs/711-capital-admission-rename/
├── spec.md      # requirements + acceptance (this feature)
└── plan.md      # this file

# Renamed source modules (Slice 1)
src/bolt_v3_position_sizer.rs              -> src/bolt_v3_capital_admission.rs
src/bolt_v3_position_sizer_runtime_feed.rs -> src/bolt_v3_capital_admission_runtime_feed.rs
src/bolt_v3_sizing_state.rs                -> src/bolt_v3_capital_admission_state.rs
src/lib.rs                                 (mod declarations)

# Modified for identifier references (Slice 1)
src/bolt_v3_submit_admission.rs, src/bolt_v3_live_node.rs, src/bolt_v3_decision_evidence.rs,
src/bolt_v3_order_execution.rs, src/bolt_v3_loss_protection.rs, src/strategies/registry.rs,
crates/backtesting-vertical-slice/src/runner.rs, tests/*.rs, tests/support/mod.rs

# Modified for serialized values / schema / config (Slice 2)
src/bolt_v3_decision_evidence.rs (string VALUES + SCHEMA_VERSION 13->14)
src/bolt_v3_config.rs            (TOML field sizing_policy -> capital_admission_policy)
config/strategies/*.toml, tests/fixtures/bolt_v3/*.toml, tests/config_parsing.rs
docs/bolt-v3/2026-04-25-bolt-v3-schema.md
scripts/verify_bolt_v3_schema_current.py, scripts/test_verify_bolt_v3_schema_current.py
docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml  (BOTH slices)

# New (Slice 2)
scripts/migrate_bolt_v3_decision_evidence_v13_to_v14.py
scripts/test_migrate_bolt_v3_decision_evidence_v13_to_v14.py
scripts/migrate_bolt_v3_capital_admission_config.py
scripts/test_migrate_bolt_v3_capital_admission_config.py
scripts/verify_no_position_sizer_in_renamed_modules.py   (naming guard) + CI wiring
```

**Structure Decision:** Single Rust project; renamed files keep their directory. Renamed files are
**not** in any gated source root (only `src/bolt_v3_submit_admission.rs` is) — Task 1.0 verifies
this, so `gated_source_roots.manifest` needs no change (FR-020).

---

## Slice 1 — Internal rename (PR A, behavior-neutral)

Changes zero persisted/operator bytes: all identifiers + file names become `capital_admission`,
but serialized string **values**, `schema_version`, and the TOML key stay as-is. Independently
mergeable.

### Task 1.0: Pre-flight verification

**Files:** none (read-only).

- [ ] **Step 1:** Confirm renamed files are not gated source roots.
  Run: `git grep -nE 'bolt_v3_position_sizer|bolt_v3_sizing_state' -- gated_source_roots.manifest`
  Expected: no output (FR-020 holds; no manifest change needed).
- [ ] **Step 2:** Snapshot the misnomer surface for later diffing.
  Run: `git grep -cE 'position_sizer|PositionSizing|PositionSizer|position_sizing' | sort`
  Save the per-file counts; Slice 1 reduces all *identifier* counts to zero, leaving only serialized
  string values (handled in Slice 2).

### Task 1.1: Rename the gate core module + identifiers

**Files:**
- Rename: `src/bolt_v3_position_sizer.rs` → `src/bolt_v3_capital_admission.rs` (use `git mv`)
- Modify: `src/lib.rs` (`pub mod bolt_v3_position_sizer;` → `pub mod bolt_v3_capital_admission;`)
- Modify: every referencing module (compiler enumerates them).

**Rename scheme (gate context):** `PositionSizing*`/`PositionSizer*`/`Sizing*`/`SizedAdmission*` →
`CapitalAdmission*`. Anchor renames (verified symbols):
`PositionSizingAdmissionGate` → `CapitalAdmissionGate`; `evaluate_position_sizing` →
`evaluate_capital_admission`; `PositionSizingRequest` → `CapitalAdmissionRequest`;
`PositionSizingInputs`/`PositionSizingGateInputs` → `CapitalAdmissionInputs`/`CapitalAdmissionGateInputs`;
`PositionSizingLifecycle*`/`PositionSizingRebuildDecision` → `CapitalAdmissionLifecycle*`/`CapitalAdmissionRebuildDecision`;
`SizingPolicy` → `CapitalAdmissionPolicy`; `ProductSizingSnapshot`/`PredictionMarketSizingSnapshot` →
`ProductAdmissionSnapshot`/`PredictionMarketAdmissionSnapshot`; `SizingEvidenceKind`/`SizingEvidenceSource` →
`CapitalAdmissionEvidenceKind`/`CapitalAdmissionEvidenceSource`; `SizedAdmissionDecision`/`SizedAdmissionEvidence`/`SizedAdmissionReason` →
`CapitalAdmissionDecision`/`CapitalAdmissionEvidence`/`CapitalAdmissionReason`.
**Keep:** `FeeSlippagePolicy` (already accurate), and the `LiabilityQuote`/`LiabilityError` type names.

- [ ] **Step 1:** `git mv src/bolt_v3_position_sizer.rs src/bolt_v3_capital_admission.rs`
- [ ] **Step 2:** Apply the rename scheme inside the moved file and `src/lib.rs`, then fix all
  references the compiler flags (do not change any `&str` literal *values* yet).
- [ ] **Step 3 (FR-004):** Rename the internal liability/decision fields:
  `sized_quantity` → `accepted_quantity`; `liability_before_sizing` → `calculated_liability`;
  `liability_after_sizing` → `reserved_liability` (in `LiabilityQuote`, `CapitalAdmissionDecision`,
  `CapitalAdmissionEvidence` and all constructors/readers).
- [ ] **Step 4:** Local static gates.
  Run: `just fmt-check && just source-fence-static`
  Expected: pass. (Compile proof deferred to Task 1.5 via CI.)
- [ ] **Step 5:** Commit. `git commit -m "refactor(711): rename position_sizer gate core -> capital_admission (slice 1)"`

### Task 1.2: Rename the runtime-feed module + identifiers

**Files:**
- Rename: `src/bolt_v3_position_sizer_runtime_feed.rs` → `src/bolt_v3_capital_admission_runtime_feed.rs`
- Modify: `src/lib.rs`; referencing modules.

Anchor renames: `PositionSizerRuntimeFeed` → `CapitalAdmissionRuntimeFeed`;
`PositionSizerRuntimeFeedConfig`/`...Subscription`/`...ComponentBuilder` → `CapitalAdmission*`;
`subscribe_position_sizer_runtime_feed` → `subscribe_capital_admission_runtime_feed`;
const identifier `POSITION_SIZER_ORDER_TERMINAL_SOURCE` → `CAPITAL_ADMISSION_ORDER_TERMINAL_SOURCE`
**(keep its `&str` value for Slice 2).**

- [ ] **Step 1:** `git mv` the file; apply scheme; update `src/lib.rs` and references.
- [ ] **Step 2:** `just fmt-check && just source-fence-static` → pass.
- [ ] **Step 3:** Commit. `git commit -m "refactor(711): rename position_sizer runtime feed -> capital_admission (slice 1)"`

### Task 1.3: Rename the input-state module + identifiers

**Files:**
- Rename: `src/bolt_v3_sizing_state.rs` → `src/bolt_v3_capital_admission_state.rs`
- Modify: `src/lib.rs`; referencing modules.

Anchor renames: `NtDerivedSizingState` → `NtDerivedCapitalAdmissionState`;
`PortfolioSizingSnapshot`/`OrderLifecycleSizingSnapshot` → `Portfolio…`/`OrderLifecycle…` with
`Sizing`→`CapitalAdmission`; `SizingStateError`/`SizingStateEvidence`/`SizingStateEvidenceKind`/`SizingStateEvidenceSource` →
`CapitalAdmissionStateError`/`...Evidence`/`...EvidenceKind`/`...EvidenceSource`;
`validate_nt_derived_sizing_state` → `validate_nt_derived_capital_admission_state`.
**Keep:** `VenueSpendabilitySnapshot`, `ReservationLedgerSnapshot` (already accurate).

- [ ] **Step 1:** `git mv` the file; apply scheme; update `src/lib.rs` and references.
- [ ] **Step 2:** `just fmt-check && just source-fence-static` → pass.
- [ ] **Step 3:** Commit. `git commit -m "refactor(711): rename sizing_state -> capital_admission_state (slice 1)"`

### Task 1.4: Rename embedding field, submit-admission/live-node/evidence identifiers, tests

**Files:** `src/bolt_v3_submit_admission.rs`, `src/bolt_v3_live_node.rs`,
`src/bolt_v3_decision_evidence.rs`, `src/bolt_v3_order_execution.rs`,
`src/bolt_v3_loss_protection.rs`, `src/strategies/registry.rs`,
`crates/backtesting-vertical-slice/src/runner.rs`, `tests/*.rs`, `tests/support/mod.rs`.

Anchor renames:
- `BoltV3SubmitAdmissionInner.position_sizer` field → `capital_admission` (FR-003).
- `BoltV3SubmitPositionSizerState`/`Config`/`...NtComponents`/`...OpenOrder*`/`...Rebuild*`/`...FillUpdate`/`...Lifecycle*` →
  `BoltV3SubmitCapitalAdmission*`; methods `new_with_position_sizer`/`new_with_loss_governor_and_position_sizer`/`position_sizer_*`/`apply_position_sizing_*`/`rebuild_position_sizing_*`/`finish_position_sizer_rebuild` →
  `*_capital_admission` / `apply_capital_admission_*` / `rebuild_capital_admission_*`.
- Enum **variant identifier** `BoltV3AdmissionOutcome::RejectedPositionSizing` → `RejectedCapitalAdmission`
  **(its string mapping value `rejected_position_sizing` stays for Slice 2).**
- `src/bolt_v3_decision_evidence.rs` internal type/method NAMES:
  `BoltV3PositionSizerRebuildAuditEvidence` → `BoltV3CapitalAdmissionRebuildAuditEvidence`;
  `PositionSizerRebuildAuditLine[Owned]` → `CapitalAdmissionRebuildAuditLine[Owned]`;
  `record_position_sizer_rebuild_audit` → `record_capital_admission_rebuild_audit`;
  `encode_position_sizer_rebuild_audit_line` → `encode_capital_admission_rebuild_audit_line`;
  const identifiers `BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID`/`_RECORD_KIND` →
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID`/`_RECORD_KIND` **(keep their `&str` values for Slice 2).**
- `src/bolt_v3_live_node.rs`: fields/methods `position_sizer_*`, struct
  `BoltV3PositionSizerVenueSpendabilitySourceConfig`, error variant `StartupPositionSizerRebuild`,
  builder fns `position_sizer_*_from_loaded`, `sizing_policy_from_pool` → `*capital_admission*` /
  `capital_admission_policy_from_pool`.
- Test fn names + `tests/support/mod.rs` helpers (`record_position_sizer_rebuild_audit` stubs).

- [ ] **Step 1:** Apply the renames above across all listed files (compiler-driven; do not touch
  any `&str` literal value, the `schema_version` integer, or the TOML field name).
- [ ] **Step 2:** `just fmt-check && just source-fence-static` → pass.
- [ ] **Step 3:** Grep for residual identifiers (string values still expected to remain).
  Run: `git grep -nE '\b(PositionSizing|PositionSizer)\b|position_sizer_(state|config|runtime|configured|reconciled|venue|from_nt)|fn .*position_sizing'`
  Expected: no identifier hits (only `&str` literal values like `"position_sizer_rebuild"` /
  `"rejected_position_sizing"` and `position_sizer_rebuild` audit `context` lines remain).
- [ ] **Step 4:** Commit. `git commit -m "refactor(711): rename position_sizer references in submit-admission/live-node/evidence/tests (slice 1)"`

### Task 1.5: Update runtime-literal audit for renamed source + add naming guard

**Files:**
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
- Create: `scripts/verify_no_position_sizer_in_renamed_modules.py` + wire into `source-fence-static`/CI.

- [ ] **Step 1:** Update every audit entry whose `path` is a renamed module file
  (`src/bolt_v3_capital_admission*.rs`) and every `context`/`literal` line that changed identifier
  text in Tasks 1.1–1.4. **Do not** change `classification` taxonomy or string-value literals yet
  (Slice 2).
- [ ] **Step 2:** Write the naming guard: assert `git grep` finds no `position_sizer`/`PositionSizing`/`PositionSizer`
  identifiers in `src/bolt_v3_capital_admission*.rs` (allow the `&str` values pending Slice 2 by
  matching identifier patterns only). Make it fail-closed (exit non-zero on a hit).
- [ ] **Step 3:** Run the naming guard + runtime-literal audit locally.
  Run: `python3.12 scripts/verify_no_position_sizer_in_renamed_modules.py && <runtime-literal audit recipe>`
  Expected: pass.
- [ ] **Step 4:** Commit. `git commit -m "chore(711): update runtime-literal audit for renamed modules + add naming guard (slice 1)"`

### Task 1.6: Slice 1 remote proof + PR

- [ ] **Step 1:** Push branch; open a **draft** PR titled `#711: rename position_sizer -> capital_admission (slice 1, internal)`; body states it is slice 1 of 2 and that serialized strings/schema/TOML are deliberately unchanged (intermediate state).
- [ ] **Step 2:** `just verify-remote` → full `CI` + Backtester CI + actionlint green on exact head.
  Evidence (FR-017): the pre-existing behavior tests pass unchanged (only identifiers changed).
- [ ] **Step 3:** Mark ready; request review from GitHub node `U_kgDOEZMFhA`. Do not merge without approval.

---

## Slice 2 — Serialized + contract rename + migration (PR B)

Flips the serialized string **values**, bumps `SCHEMA_VERSION`, renames the TOML key, ships
migration tools, and updates audit/verifier/docs. Depends on Slice 1 merged.

### Task 2.1: Flip serialized string values + bump schema version

**Files:** `src/bolt_v3_decision_evidence.rs`, plus the `RejectedCapitalAdmission` string mapping
and the `CAPITAL_ADMISSION_ORDER_TERMINAL_SOURCE` value.

- [ ] **Step 1 (FR-006/007/008/009):** Change the `&str` values:
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND` `"position_sizer_rebuild"` → `"capital_admission_rebuild"`;
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID` `"bolt_v3.position_sizer_rebuild"` → `"bolt_v3.capital_admission_rebuild"`;
  the `RejectedCapitalAdmission` outcome string `"rejected_position_sizing"` → `"rejected_capital_admission"`;
  the evidence source label `"nt_position_sizer_runtime_components"` → `"nt_capital_admission_runtime_components"`.
  Update the read-dispatch `match` arms to the new literals.
- [ ] **Step 2 (FR-010):** Bump `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` 13 → 14
  (`src/bolt_v3_decision_evidence.rs:23`).
- [ ] **Step 3:** Update Rust unit tests that assert the old literals/version to the new ones; keep
  the below-schema skip (audit-only) + reservation fail-closed tests and confirm they still pass
  the design at `decision_evidence.rs:1987-1994, 2284`.
- [ ] **Step 4:** `just fmt-check && just source-fence-static` → pass. Commit.
  `git commit -m "feat(711): rename serialized capital_admission strings + bump evidence schema 13->14 (slice 2)"`

### Task 2.2: Rename TOML key + fixtures

**Files:** `src/bolt_v3_config.rs`, `config/**.toml`, `tests/fixtures/bolt_v3/*.toml`,
`tests/config_parsing.rs`.

- [ ] **Step 1 (FR-011):** Rename the `CapitalPoolBlock` field `sizing_policy` →
  `capital_admission_policy` (block stays under `[[risk.capital_pools]]`, `deny_unknown_fields`
  intact). Update the doc comment prose naming the component.
- [ ] **Step 2:** Update every `[risk.capital_pools.sizing_policy]` occurrence in `config/` and
  `tests/fixtures/` to `capital_admission_policy`, and the `tests/config_parsing.rs` assertions.
- [ ] **Step 3:** Confirm (FR-Assumption) whether the root-config `schema_version` must also bump:
  inspect `src/bolt_v3_config.rs` for a supported-version constant / gate. If one exists, bump it
  and update the `root.schema_version` test; if not, record that `deny_unknown_fields` is the
  fail-closed mechanism. Document the finding in the PR.
- [ ] **Step 4:** `just fmt-check && just source-fence-static` → pass. Commit.
  `git commit -m "feat(711): rename capital pool sizing_policy -> capital_admission_policy TOML key (slice 2)"`

### Task 2.3: JSONL evidence migration tool (TDD — new code)

**Files:**
- Create: `scripts/migrate_bolt_v3_decision_evidence_v13_to_v14.py`
- Test: `scripts/test_migrate_bolt_v3_decision_evidence_v13_to_v14.py`

**Produces:** a CLI that rewrites a v13 decision-evidence directory in place: renames record
kind / gate-id / outcome / source-label strings (FR-006/007/008/009) and sets every envelope's
`schema_version` to 14, preserving all other fields. Rewrites the **whole** directory (mixed-version
dirs fail closed at runtime otherwise — spec Edge Cases).

- [ ] **Step 1:** Write failing tests: a v13 fixture with one `position_sizer_rebuild` (audit) record,
  one `submit_reservation_metadata` (recovery) record, and one `rejected_position_sizing` outcome
  record → after migration: kinds/outcomes renamed where applicable, `submit_reservation_metadata`
  kind unchanged but `schema_version` now 14, all other fields byte-identical.
  Run: `python3.12 -m pytest scripts/test_migrate_bolt_v3_decision_evidence_v13_to_v14.py -v` → FAIL.
- [ ] **Step 2:** Implement the migrator (line-by-line JSON rewrite; refuse to run on a directory
  already at v14; idempotent check).
- [ ] **Step 3:** Run tests → PASS. Add a test that an already-v14 dir is a no-op / refused.
- [ ] **Step 4:** Commit. `git commit -m "feat(711): one-time v13->v14 decision-evidence migration tool (slice 2)"`

### Task 2.4: Config migration tool (TDD — new code)

**Files:**
- Create: `scripts/migrate_bolt_v3_capital_admission_config.py`
- Test: `scripts/test_migrate_bolt_v3_capital_admission_config.py`

- [ ] **Step 1:** Write failing test: a TOML with `[risk.capital_pools.sizing_policy]` → after
  migration the table is `capital_admission_policy` with identical child values; other tables
  untouched. Run pytest → FAIL.
- [ ] **Step 2:** Implement (TOML-key rename preserving values + comments where feasible).
- [ ] **Step 3:** Run tests → PASS. Commit.
  `git commit -m "feat(711): one-time sizing_policy->capital_admission_policy config migration tool (slice 2)"`

### Task 2.5: Update audit classifications, schema doc, Python schema verifier

**Files:** `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`,
`docs/bolt-v3/2026-04-25-bolt-v3-schema.md`, `scripts/verify_bolt_v3_schema_current.py`,
`scripts/test_verify_bolt_v3_schema_current.py`.

- [ ] **Step 1 (FR-012):** Update audit `classification` taxonomy `position_sizer_*` →
  `capital_admission_*` and every `literal`/`context` line whose string **value** changed in
  Task 2.1.
- [ ] **Step 2 (FR-013):** Update the schema doc to the new record kind + `schema_version` 14, and
  the Python verifier + its test to assert the new strings/version.
- [ ] **Step 3:** Run `python3.12 scripts/verify_bolt_v3_schema_current.py` + the runtime-literal
  audit recipe + `python3.12 -m pytest scripts/test_verify_bolt_v3_schema_current.py -v` → pass.
- [ ] **Step 4:** Commit. `git commit -m "chore(711): update audit/schema-doc/verifier for capital_admission serialized names (slice 2)"`

### Task 2.6: Slice 2 remote proof + PR

- [ ] **Step 1:** Push; open PR `#711: rename serialized capital_admission contracts + migration (slice 2)`; body documents the migration steps operators must run (run both migration tools before starting the renamed binary) and the fail-closed behavior if skipped.
- [ ] **Step 2:** `just verify-remote` → full CI green on exact head.
  Evidence: migration unit tests green (FR-014/015); below-schema skip + reservation fail-closed
  Rust tests green (spec Edge Cases); schema verifier + audit green (SC-003).
- [ ] **Step 3:** Mark ready; request review from node `U_kgDOEZMFhA`.

---

## Risk / Complexity Tracking

| Risk | Mitigation | Evidence |
|------|-----------|----------|
| Missed identifier leaves an inconsistent build | Compiler + naming guard + grep snapshot (Task 1.0/1.4/1.5) | exact-head CI + naming guard exit code |
| Schema bump breaks startup reservation recovery | Recovery rides on un-renamed `submit_reservation_metadata/fill`; migration rewrites whole dir; un-migrated input fails closed by design (`decision_evidence.rs:2284`) | Task 2.3 tests + Rust fail-closed tests |
| Audit `path`/`context` drift fails CI | Audit updated in **both** slices (Task 1.5 for paths/identifiers, Task 2.5 for values/taxonomy) | runtime-literal audit recipe green |
| Half-rename lands on `main` | Slice 2 mandatory; PR A body marks the intermediate state; tracker keeps #711 open until Slice 2 merges | #711 closed only after Slice 2 |
| Operator skips migration | Currently theoretical (no active deploy); fail-closed not silent; PR B body documents the required steps | spec Assumptions + Edge Cases |

## Self-Review (spec coverage)

- FR-001..005 → Tasks 1.1–1.4. FR-006..009 → Task 2.1. FR-010 → Task 2.1. FR-011 → Task 2.2.
  FR-012 → Tasks 1.5 + 2.5. FR-013 → Task 2.5. FR-014 → Task 2.3. FR-015 → Task 2.4.
  FR-016 → Tasks 2.1/2.2 (no dual path) + 2.3/2.4 (offline migration). FR-017 → Tasks 1.6/2.6
  evidence. FR-018 (keep-list) → enforced by scope notes in 1.1/1.3 + audit. FR-019 → Task 1.5.
  FR-020 → Task 1.0.
- SC-001 → Task 1.4 grep + naming guard. SC-002 → Tasks 1.6/2.6. SC-003 → Task 2.5.
  SC-004 → Task 2.3. SC-005 → Tasks 2.2/2.4. SC-006 → Tasks 1.6/2.6.
- No spec requirement is unmapped.
