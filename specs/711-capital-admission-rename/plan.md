# Implementation Plan: Rename `position_sizer` → `capital_admission` gate

> **Historical reference only.** This plan is superseded by current `AGENTS.md`; its unchecked
> steps and commands are non-operational and must not be executed.

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
- **Remote-first Rust verification** — use the current local non-compile gates from `AGENTS.md`; publish with `git push` and adjudicate advisory CI for the exact head. Do not run local compile-heavy cargo.
- **Review Bar** — open the PR and request review from the GitHub account with node ID `U_kgDOEZMFhA`; do not merge without its approval; do not request external review until exact-head CI is green.
- **Evidence per requirement** — refactor evidence = existing tests + static checks + structural-equivalence review; new code (migration tools, equivalence test) = behavior tests; persisted/config contract changes = fail-closed evidence for invalid/missing/legacy inputs + exact-head proof.
- **Gating model** — the per-task local gate is `just fmt-check`, which (via `fmt-check-inner`, justfile:240) runs `verify_bolt_v3_runtime_literals.py` + `verify_bolt_v3_provider_leaks.py`. **The runtime-literal audit therefore gates EVERY task, not just the final head.** Its allowlist (`docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`, rows keyed `(path, kind, literal, context)`) is part of the rename surface: any task that moves a file or renames a symbol on a classified line MUST update that row's `path`/`context` **in the same commit**, or both the new path's literals (unclassified) and the old rows (stale) fail `fmt-check` (`main()` fails on `unclassified or stale`). Serialized `literal` VALUES change only in Task 5, which updates those rows' `literal` field. What is genuinely deferred to the integrated head (Task 11) are the verifiers that run ONLY in `source-fence-static-inner` and NOT in `fmt-check`: `verify_bolt_v3_naming.py` (Task 9), status-map, and schema-current — plus the audit-TOML `classification` metadata (not part of the scan key, so it never breaks `fmt-check`; batched in Task 10). Run the full `just source-fence-static` ONCE on the integrated head in Task 11; the final head MUST be fence-clean.

## Technical Context

**Language/Version**: Rust (repo-pinned toolchain); Python 3.12 on CI (test migration/verifier scripts with `python3.12`).
**Primary Dependencies**: NautilusTrader Rust API; serde/serde_json; `aws-sdk-ssm` (unaffected).
**Storage**: Decision-evidence JSONL files (schema-versioned); operator TOML config (root `schema_version`).
**Testing**: Advisory exact-head PR CI for Rust after `git push`; `pytest`/`python3.12` for migration + verifier scripts; use the current local gates from `AGENTS.md`.
**Target Platform**: Linux server node.
**Project Type**: Single Rust project (compiler/trading runtime) with Python tooling.
**Constraints**: Behavior-preserving; fail-closed on invalid/missing/legacy inputs; exact-head CI green before review.
**Scale/Scope**: ~30 files touched; ~768 misnomer hits (src 507 / tests 261 / docs 75 / scripts 6, measured at `07db9cb04`); **5** serialized string values (kind, gate-id, outcome, `nt_position_sizer_runtime_components`, **`nt_sizing_state`**) + decision-evidence schema bump 13→14 + TOML key + root-config schema bump 1→2; 2 migration tools; misnomer fence folded into `verify_bolt_v3_naming.py`.

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
scripts/migrate_bolt_v3_decision_evidence_to_v15.py            (+ test_…)
scripts/migrate_bolt_v3_capital_admission_config.py               (+ test_…)
scripts/verify_bolt_v3_naming.py                                  (EXTENDED into the misnomer fence — not a new script)
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
- [ ] **Step 4:** Local static gate. Run: `just fmt-check`. Expected: pass. (`fmt-check` runs the runtime-literal + provider-leak audits — in THIS commit update the `path` (and `context`, where a renamed symbol appears on the line) of every `bolt-v3-runtime-literal-audit.toml` row for the file(s)/line(s) this task moved or renamed; leave serialized `literal` VALUES for Task 5. The `source-fence-static`-only verifiers — `verify_bolt_v3_naming.py`, status-map, schema-current — are deferred to Task 11.)
- [ ] **Step 5:** Commit. `git commit -m "refactor(711): rename position_sizer gate core -> capital_admission"`

## Task 2: Rename the runtime-feed module + identifiers + its test file

**Files:**
- Rename: `src/bolt_v3_position_sizer_runtime_feed.rs` → `src/bolt_v3_capital_admission_runtime_feed.rs`;
  `tests/bolt_v3_position_sizer_runtime_feed.rs` → `tests/bolt_v3_capital_admission_runtime_feed.rs`
- Modify: `src/lib.rs`; referencing modules.
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` — set `path` →
  `src/bolt_v3_capital_admission_runtime_feed.rs` on the ~11 rows for the moved file, and update
  `context` on any row whose source line names a symbol renamed in this task (e.g. the
  `POSITION_SIZER_ORDER_TERMINAL_SOURCE` const row). Do NOT change those rows' `literal` VALUES —
  the serialized strings (`"nt_position_sizer_runtime_components"`, etc.) flip in Task 5.

Anchor renames: `PositionSizerRuntimeFeed` → `CapitalAdmissionRuntimeFeed`;
`PositionSizerRuntimeFeedConfig`/`...Subscription`/`...ComponentBuilder` → `CapitalAdmission*`;
`subscribe_position_sizer_runtime_feed` → `subscribe_capital_admission_runtime_feed`;
const identifier `POSITION_SIZER_ORDER_TERMINAL_SOURCE` → `CAPITAL_ADMISSION_ORDER_TERMINAL_SOURCE`.
**Note (FR-017):** that const's value is `stringify!(nt_order_terminal_event)` = `"nt_order_terminal_event"`
— the identifier renames; **the value never changes** and is not a misnomer. Do not introduce a string
literal for it.

- [ ] **Step 1:** `git mv` both files; apply rule; update `src/lib.rs` and references.
- [ ] **Step 2:** `just fmt-check` → pass. (`fmt-check` runs the runtime-literal + provider-leak audits — in THIS commit update the `path` (and `context`, where a renamed symbol appears on the line) of every `bolt-v3-runtime-literal-audit.toml` row for the file(s)/line(s) this task changed; leave serialized `literal` VALUES for Task 5. `source-fence-static`-only verifiers (naming fence, status-map, schema-current) are deferred to Task 11.)
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
- [ ] **Step 2:** `just fmt-check` → pass. (`fmt-check` runs the runtime-literal + provider-leak audits — in THIS commit update the `path` (and `context`, where a renamed symbol appears on the line) of every `bolt-v3-runtime-literal-audit.toml` row for the file(s)/line(s) this task changed; leave serialized `literal` VALUES for Task 5. `source-fence-static`-only verifiers (naming fence, status-map, schema-current) are deferred to Task 11.)
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
- [ ] **Step 2:** `just fmt-check` → pass. (`fmt-check` runs the runtime-literal + provider-leak audits — in THIS commit update the `path` (and `context`, where a renamed symbol appears on the line) of every `bolt-v3-runtime-literal-audit.toml` row for the file(s)/line(s) this task changed; leave serialized `literal` VALUES for Task 5. `source-fence-static`-only verifiers (naming fence, status-map, schema-current) are deferred to Task 11.)
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
- [ ] **Step 2b (FR-009b — `nt_sizing_state`, the 5th serialized value):** Rename variant
  `BoltV3LossSnapshotSource::NtSizingState` → `NtCapitalAdmissionState` (`:789`); change the const
  `LOSS_SNAPSHOT_SOURCE_NT_SIZING_STATE` value `stringify!(nt_sizing_state)` →
  `stringify!(nt_capital_admission_state)` (`:805`) and rename the const identifier; update the decode
  arm (`:822`); and change the three hard-coded emit literals `source: "nt_sizing_state".to_string()`
  at `src/bolt_v3_order_execution.rs:1548`, `src/bolt_v3_position_sizer.rs:802` (now
  `bolt_v3_capital_admission.rs`), `src/bolt_v3_sizing_state.rs:426` (now
  `bolt_v3_capital_admission_state.rs`), and the test at
  `tests/bolt_v3_capital_admission_runtime_feed.rs:2475`. Leave the other ten `LOSS_SNAPSHOT_SOURCE_*`
  labels unchanged.
- [ ] **Step 3 (FR-017 — legacy skip literal):** In
  `decision_evidence_header_is_below_current_schema_non_recovery_record` (`:1995-2006`), retain the
  **legacy literal** `"position_sizer_rebuild"` in the matched set alongside the new
  `BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND`, with a deprecation comment — so pre-rename
  audit-only records still skip rather than fail closed (spec Edge Cases / SC-004).
- [ ] **Step 4 (FR-010):** Bump `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION` 13 → 14 (`:23`).
- [ ] **Step 5 (blocking legacy-skip test):** Update Rust unit tests asserting the old literals/version
  to the new values; keep the below-schema audit-skip + reservation fail-closed tests at
  `decision_evidence.rs:1995-2006, 2284`. Add an **explicit, blocking** test asserting BOTH: (a) a
  schema-13 record with kind `"position_sizer_rebuild"` is classified skippable (non-recovery), and
  (b) a schema-13 `submit_reservation_metadata` record is NOT skippable (fails closed). This pins the
  FR-017 legacy-literal addition, which is an easy drop inside a ~768-hit rename.
- [ ] **Step 6:** `just fmt-check` → pass. (This task changes serialized `literal` VALUES, so `fmt-check`'s runtime-literal audit will fail unless you update the matching `bolt-v3-runtime-literal-audit.toml` rows' `literal`/`context` to the new values — `"capital_admission_rebuild"`, `"bolt_v3.capital_admission_rebuild"`, `"nt_capital_admission_runtime_components"`, `"nt_capital_admission_state"` — in THIS commit. `source-fence-static`-only verifiers are deferred to Task 11.) Commit.
  `git commit -m "feat(711): flip serialized capital_admission values + bump evidence schema 13->14 + keep legacy audit-skip literal"`

## Task 6: Rename TOML key + type + bump root-config schema version

**Files:** `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, `config/**/*.toml`,
`tests/fixtures/bolt_v3/*.toml`, `tests/config_parsing.rs`.

- [ ] **Step 1 (FR-011):** Rename the `CapitalPoolBlock` field `sizing_policy` →
  `capital_admission_policy` (`:243`) and the block type `CapitalPoolSizingPolicyBlock` →
  `CapitalAdmissionPolicyBlock` (`:256`); block stays under `[[risk.capital_pools]]`,
  `deny_unknown_fields` intact. Update doc-comment prose naming the component. Keep child keys
  `min_remaining_pool_balance`, `fee_slippage`, `max_fee_liability`, `max_slippage_liability`.
  Also update the user-visible config-path strings that embed the key (compiler won't catch these):
  `"risk.capital_pools.sizing_policy.*"` at `src/bolt_v3_live_node.rs:5476,5481,5485` and the
  `"{label}.sizing_policy.*"` `format!` strings at `src/bolt_v3_validate.rs:1528,1534,1539`.
- [ ] **Step 2 (FR-012):** Bump `SUPPORTED_ROOT_SCHEMA_VERSION` 1 → 2 (`src/bolt_v3_validate.rs:108`).
  Update the `tests/config_parsing.rs` assertion `root.schema_version == 1` → `== 2`. **Do NOT touch**
  `SUPPORTED_STRATEGY_SCHEMA_VERSION` (`:109`, already `= 2`) — it is a separate constant for strategy
  configs; keep the two `config_parsing` assertions distinct (root → 2, strategy stays 2).
- [ ] **Step 3:** Update every config under `config/` and `tests/fixtures/`:
  `[risk.capital_pools.sizing_policy]` → `[risk.capital_pools.capital_admission_policy]` and root
  `schema_version = 1` → `schema_version = 2`.
- [ ] **Step 4:** `just fmt-check` → pass. (The config-path `format!`/string literals you change at `live_node.rs:5476,5481,5485` and `validate.rs:1528,1534,1539` may be classified rows — update their `literal`/`context` in `bolt-v3-runtime-literal-audit.toml` in THIS commit so the runtime-literal audit stays green. `source-fence-static`-only verifiers are deferred to Task 11.) Commit.
  `git commit -m "feat(711): rename sizing_policy TOML key/type + bump root-config schema 1->2"`

## Task 7: JSONL evidence migration tool + recovery-equivalence test (new code, TDD)

**Files:**
- Create: `scripts/migrate_bolt_v3_decision_evidence_to_v15.py` + `scripts/test_migrate_bolt_v3_decision_evidence_to_v15.py`
- Create: `tests/bolt_v3_capital_admission_recovery_equivalence.rs` + fixtures under
  `tests/fixtures/bolt_v3/capital_admission_recovery/`

**Interfaces — Produces:** a CLI `migrate_bolt_v3_decision_evidence_to_v15.py <dir> [--dry-run]`
that, per file, applies **key-scoped** replacements (record-kind / gate-id / outcome / source-label
string values per FR-006/007/008/009/009b) and sets each envelope's `schema_version` to 14 —
**including** `submit_reservation_metadata` / `submit_reservation_fill` — writing atomically (temp +
fsync + os.replace), idempotently (records already at v14 untouched), over the whole directory.
Replacements MUST be anchored to JSON keys (`"schema_version":13`, `"kind":"position_sizer_rebuild"`,
`"gate_id":"bolt_v3.position_sizer_rebuild"`, `"outcome":"rejected_position_sizing"`,
`"source":"nt_sizing_state"`, `"source":"nt_position_sizer_runtime_components"`) — NOT a bare `13`→`14`
(would corrupt `recorded_at_utc_ns` timestamps) and NOT a JSON `loads`/`dumps` round-trip (serde's
`Value` is a `BTreeMap` → reorders keys). Accept only schema `13` (migrate) or `14` (skip); refuse any
other version. Emit a changed-file manifest (path + before/after hash).

- [ ] **Step 1:** Write failing Python tests:
  (a) a v13 fixture with one `position_sizer_rebuild` (audit) record, one `submit_reservation_metadata`
  (recovery) record, one `admission_decision` record whose `outcome` is `"rejected_position_sizing"`,
  and one record carrying `source:"nt_sizing_state"` → after migration: kind/outcome/source-label
  values renamed (incl. `nt_sizing_state` → `nt_capital_admission_state`), `submit_reservation_metadata`
  kind unchanged but `schema_version` now 14, and **every other byte identical** (assert exact line
  bytes for unaffected fields);
  (b) **non-corruption guard** — a record whose `recorded_at_utc_ns` value contains `13` (e.g.
  `1731234567890123456`) AND a payload string field (`strategy_id`/`client_order_id`) whose *value* is
  literally `"position_sizer_rebuild"` / `"nt_sizing_state"`: assert the timestamp and those payload
  values survive **byte-unchanged** (proves key-scoping);
  (c) re-running on the migrated dir is a no-op (idempotent);
  (d) a dir already containing a v14 record is completed, not refused (resumable);
  (e) a schema-15 AND a schema-12 record each cause refusal (accept only 13/14).
  Run: `python3.12 -m pytest scripts/test_migrate_bolt_v3_decision_evidence_to_v15.py -v` → FAIL.
- [ ] **Step 2:** Implement the migrator: **key-anchored regex** replacements (per Interfaces — never a
  bare integer replace, never JSON round-trip) + `tempfile` + `os.fsync` + `os.replace`; per-record
  version guard for idempotency; pre-scan to refuse any version other than 13/14; `--dry-run` + manifest.
- [ ] **Step 3:** Run tests → PASS.
- [ ] **Step 4 (SC-004 — Rust recovery equivalence):** Add `tests/bolt_v3_capital_admission_recovery_equivalence.rs`
  with a fixture dir exercising **≥2 capital pools** and the reservation lifecycle:
  `submit_reservation_metadata`, `submit_reservation_fill` (partial + complete/release), a revalue, and
  a rejected admission that reserves nothing. Migrate it, load through the renamed recovery path, and:
  (a) assert each migrated v14 record **decodes field-identical** to its v13 original (the real,
  provable claim — migration touches no reservation payload field); (b) assert recovered state matches
  a **checked-in golden snapshot** at **per-reservation** granularity (reservation id, order mapping,
  fill/release/revalue state) — NOT hand-derived aggregate pool totals. Add the fail-closed/skip
  assertions: an un-migrated v13 reservation record → fail closed; a v13 legacy `position_sizer_rebuild`
  audit record → skipped.
- [ ] **Step 5:** Commit. `git commit -m "feat(711): atomic idempotent v13->v14 evidence migrator + recovery-equivalence test"`

## Task 8: Config migration tool (new code, TDD)

**Files:**
- Create: `scripts/migrate_bolt_v3_capital_admission_config.py` + `scripts/test_migrate_bolt_v3_capital_admission_config.py`

- [ ] **Step 1:** Write failing tests covering: (a) a TOML with `[risk.capital_pools.sizing_policy]` and
  root `schema_version = 1` → after migration the table is `capital_admission_policy` with identical
  child values, root `schema_version = 2`, comments preserved; (b) **multiple** `[[risk.capital_pools]]`
  blocks → every `sizing_policy` migrated; (c) a comment containing the word `sizing_policy` → NOT
  rewritten; (d) a `sizing_policy` token in an unrelated table outside `risk.capital_pools` → NOT
  rewritten. Run: `python3.12 -m pytest scripts/test_migrate_bolt_v3_capital_admission_config.py -v` → FAIL.
- [ ] **Step 2:** Implement with the **Python stdlib only — NO third-party dependency.** The repo has
  no Python manifest and pins deps only via hash-locked `.github/requirements/*.txt` for CI; `tomlkit`
  is unavailable to operators running a one-time field migration, so it is rejected. Use a
  **line-anchored, table-context-scoped rewriter** — the same no-round-trip discipline as the Task 7
  JSONL migrator — which preserves comments/order/formatting **byte-for-byte** by editing only the
  targeted tokens:
  1. Bump the **root-scope** `schema_version = 1` → `2`: the `schema_version` key that appears before
     the first `[table]` header. Never touch `report_schema_version` or any nested `schema_version`.
  2. While scanning lines, **track the current table context** (the most recent `[…]` / `[[…]]` header)
     and rename the `sizing_policy` segment → `capital_admission_policy` ONLY when it is a path segment
     of a `[risk.capital_pools.sizing_policy…]` header, OR a bare/dotted/inline `sizing_policy` key while
     the active context is `[[risk.capital_pools]]` / `[risk.capital_pools]`. Leave any `sizing_policy`
     in a comment, a value, or an unrelated table untouched (covers test cases c, d).

  Provide `--dry-run` (print the unified diff / manifest; write nothing). This validates against
  `tests/fixtures/bolt_v3/root.toml`'s real shape: root `schema_version` on line 1, headers
  `[risk.capital_pools.sizing_policy]` + `[risk.capital_pools.sizing_policy.fee_slippage]`.
- [ ] **Step 3:** Run tests → PASS. Commit.
  `git commit -m "feat(711): one-time sizing_policy->capital_admission_policy + root schema 1->2 config migrator"`

## Task 9: Extend the existing naming verifier into a repo-wide misnomer fence

**Do this AFTER Task 10** (or in the same commit): the verifier scans `docs/`, so it only goes green
once the audit TOML's `position_sizer` references are updated by Task 10.

**Files:**
- Modify: `scripts/verify_bolt_v3_naming.py` (+ `scripts/test_verify_bolt_v3_naming.py`) — extend the
  EXISTING verifier (already run by `source-fence-static`), do NOT add a parallel fence script.
- Create: `specs/711-capital-admission-rename/misnomer-allowlist.txt`.

**Interfaces — Produces:** the extended verifier fails closed on any misnomer hit over `src/`, `tests/`,
`config/`, `scripts/`, `docs/` not in the allowlist file. It MUST:
- Match **case-insensitively** so SCREAMING_SNAKE (`BOLT_V3_POSITION_SIZER_*`,
  `POSITION_SIZER_ORDER_TERMINAL_SOURCE`, `EXPECTED_POSITION_SIZER_*`) and PascalCase (`SizingPolicy`,
  `SizedQuantityMismatch`) are caught — a case-sensitive set silently misses ~19 SCREAMING_SNAKE lines
  including the constants being renamed.
- Cover stems: `position[_]?siz`, `sizing_policy`, `sizing_state`, `sized_quantity`/`SizedQuantity`,
  `SizedAdmission`, `nt_sizing_state`, `nt_position_sizer`, and gate-context `*Sizing*` evidence types.
- Carry an explicit **legit-sizer keep-list** (`bolt_v3_sizing.rs`, `choose_robust_size`, `RobustSize*`,
  `SUPPORTED_STRATEGY_SCHEMA_VERSION`) so it does NOT over-match the real sizer.
- **Fail closed if the allowlist file is missing.**

- [ ] **Step 1:** Write the allowlist file: the FR-017 legacy skip literal line in
  `decision_evidence.rs`, this spec, the 506 spec, and any deliberate historical-prose line — each with
  a one-line justification. No code identifiers permitted.
- [ ] **Step 2:** Extend `verify_bolt_v3_naming.py` (case-insensitive matcher + stems + keep-list +
  allowlist-required) and its test `test_verify_bolt_v3_naming.py` (assert a SCREAMING_SNAKE residual
  is caught, a legit-sizer token is not flagged, and a missing allowlist fails closed).
- [ ] **Step 3:** Run the verifier standalone. Run: `python3.12 scripts/verify_bolt_v3_naming.py`.
  Expected: pass once Tasks 1–8 AND Task 10 are done; if it flags a residual, fix the residual (do not
  add code identifiers to the allowlist). (The full `just source-fence-static` runs in Task 11.)
- [ ] **Step 4:** Commit. `git commit -m "chore(711): extend verify_bolt_v3_naming into repo-wide capital_admission fence + allowlist"`

## Task 10: Update runtime-literal audit, schema doc, Python schema verifier

**Files:** `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`,
`docs/bolt-v3/2026-04-25-bolt-v3-schema.md`, `scripts/verify_bolt_v3_schema_current.py`,
`scripts/test_verify_bolt_v3_schema_current.py`.

- [ ] **Step 1 (FR-016):** `path`/`context`/`literal` rows are already kept in sync per-task (Tasks
  1–6 — `fmt-check` gates them, so the audit cannot have drifted there). Here, update only the audit-TOML
  metadata the per-task gate does NOT scan: every `classification` value `position_sizer_*` →
  `capital_admission_*`. Then re-run the runtime-literal audit to confirm zero residual
  `path`/`context`/`literal` drift remains.
- [ ] **Step 2 (FR-016):** Update the schema doc to the new record kind + `schema_version` 14; update
  `verify_bolt_v3_schema_current.py` + `test_verify_bolt_v3_schema_current.py` so their literal
  kind/version strings (including the stale `schema_version = 10` fixture at
  `test_verify_…:72`) are consistent with v14 and the renamed kind.
- [ ] **Step 3 (SC-003):** Run `python3.12 scripts/verify_bolt_v3_schema_current.py` + the
  runtime-literal audit recipe + `python3.12 -m pytest scripts/test_verify_bolt_v3_schema_current.py -v`
  → pass.
- [ ] **Step 4:** Commit. `git commit -m "chore(711): update audit/schema-doc/verifier for capital_admission serialized names + schema 14"`

## Task 11: Remote proof + PR

- [ ] **Step 1 (final local gate on integrated head):** Run the FULL `just source-fence-static`
  (runtime-literal audit + extended `verify_bolt_v3_naming.py` fence + status-map) + `just fmt-check`
  → all pass. This is the first point the full fence can pass (Tasks 1–10 complete). Confirm only
  allowlisted misnomer lines remain.
- [ ] **Step 2:** Push branch; open a PR titled `#711: rename position_sizer -> capital_admission gate`.
  Body: bullets covering the rename surfaces, the decision-evidence schema 13→14 + root-config 1→2
  bumps, the two migration tools and **the exact operator runbook** (run BOTH migration tools — with
  `--dry-run` first to inspect the manifest — before starting the renamed binary; both partial states
  fail closed if skipped), and `Closes #711` is **omitted** per the no-close-keyword rule — link with
  `Refs #711` / `Blocks #712` instead.
- [ ] **Step 3:** `git push` → adjudicate advisory CI evidence for the exact head.
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
| Migration corrupts the only evidence copy (bare `13`→`14` hits timestamps; JSON round-trip reorders keys) | **Key-anchored** substitution (no bare integer, no JSON round-trip) + atomic temp/fsync/rename + idempotent/resumable + accept-only-13/14 (FR-013) | Task 7 non-corruption guard (13-in-timestamp + old-string-as-payload) + idempotency + refusal tests |
| Fence silently misses SCREAMING_SNAKE / Pascal misnomer | Case-insensitive matcher folded into `verify_bolt_v3_naming.py` + legit-sizer keep-list + allowlist-required (FR-019) | Task 9 test: SCREAMING_SNAKE residual caught, legit sizer not flagged |
| 5th serialized value (`nt_sizing_state`) left unmigrated | Renamed at variant/const/3 emit sites/decode (Task 5 Step 2b) + migrated (FR-013) + fence-caught | Task 5/7 tests + fence |
| Root-config key rename silently accepted on old configs | Bump `SUPPORTED_ROOT_SCHEMA_VERSION` 1→2 + config migrator (Task 6/8); `deny_unknown_fields` is secondary | Task 6 config_parsing assertion + Task 8 migrator test (SC-005) |
| Audit `path`/`context`/`literal` drift fails `fmt-check` mid-rename (the audit runs in `fmt-check`, every task) | Each rename commit updates the matching audit rows in lockstep (Global Constraints gating model); `classification` metadata batched in Task 10 | per-task `just fmt-check` green + Task 10 audit recipe (SC-003) |
| Operator skips migration | Currently theoretical (no active deploy) but **not** weakened; fail-closed not silent; PR body documents the required steps | spec Assumptions + Edge Cases + Task 11 PR body |

## Self-Review (spec coverage)

- FR-001 → Tasks 1/2/3 (modules + test-file rename). FR-002 → Tasks 1–4 (rule + verified surfaces).
  FR-003 → Task 4. FR-004 (keep) → scope notes in Tasks 1/3. FR-005 (strings) → Task 4 Step 1.
  FR-006/007/009 → Task 5 Step 2. FR-009b (`nt_sizing_state`, 5th value) → Task 5 Step 2b + Task 7.
  FR-008 (two sites) → Task 5 Step 1. FR-010 → Task 5 Step 4.
  FR-011 (key + type + config-path strings) → Task 6 Steps 1/3. FR-012 (root version, ≠ strategy) →
  Task 6 Step 2. FR-013 (key-scoped migrator) → Task 7. FR-014 (stdlib config migrator) → Task 8.
  FR-015 (no dual path) → Tasks 5/6 (single reader) + 7/8 (offline migration). FR-016 → Task 10.
  FR-017 (invariants + keep-list + legacy skip literal + terminal-source value) → Tasks 1/3 scope +
  Task 5 Step 3 + Task 2 note. FR-018 → Task 0 Step 1. FR-019 (repo-wide fence) → Task 9.
- SC-001 → Task 9/11 fence gate. SC-002 → Task 11. SC-003 → Task 10. SC-004 → Task 7 Step 4.
  SC-005 → Tasks 6/8. SC-006 → Task 11 evidence.
- Placeholder scan: none (every step has a command or concrete edit). Type-consistency: renamed
  types used in later tasks (`CapitalAdmissionGate`, `BoltV3CompiledOrderAdmissionEvidence`,
  `CapitalAdmissionPolicyBlock`, `BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND`) match their
  defining tasks. No spec requirement is unmapped.
