# End-to-End Contract Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make target identity and evidence novelty closed contracts across configuration, public APIs, lifecycle recovery, generated Rust, and exact-head verification.

**Architecture:** Parse `configured_target_id` once into a validated `ConfiguredTargetId` newtype and carry it through every target/selection/requirement seam. Split lifecycle instrument projection from evidence identity projection so recovery never depends on evidence-only metadata. Freeze every novelty ID-to-semantic mapping and compile/lint generated Rust under the repository's remote-first policy.

**Tech Stack:** Rust 1.96, Serde, TOML, Nautilus Trader Rust APIs, Python 3 `tomllib`, repository `just` gates, GitHub Actions Rust Probe.

## Global Constraints

- The user explicitly approved repository-wide contract closure inside PR #1429 despite the normal one-issue-per-PR rule.
- No trading, admission, sizing, order-submission, settlement-policy, secret-source, deployment, or live-operation change.
- No fallback identity, alternate config format, duplicate constructor, or unchecked public mutation path.
- Recovery must preserve its pre-PR behavior without requiring evidence-only provider metadata.
- Runtime values remain TOML-owned; generated Rust remains byte-identical to generator output.
- Do not run local compile-heavy Rust commands. Use local static gates, at most two scoped Rust Probes, then exact-head remote verification.
- Every task follows red-green-refactor where a local or permitted remote test can express the contract.

## File Responsibility Map

- Create `src/bolt_v3_target_identity.rs`: canonical stable-field predicate, `ConfiguredTargetId`, checked Serde, and focused unit tests.
- Modify `src/lib.rs`: register the target-identity module.
- Modify `src/bolt_v3_evidence_novelty.rs`: consume the shared stable-field predicate; retain independent episode-component validation.
- Modify `src/bolt_v3_market_families/mod.rs`: validated dispatch boundaries, selection target, immutable selected-market requirement, and contract-matrix tests.
- Modify family files under `src/bolt_v3_market_families/`: typed target blocks/plans and consumer-specific lifecycle/evidence projections.
- Modify maker/taker config and selection files: carry `ConfiguredTargetId` through strategy construction and selection.
- Modify `scripts/verify_bolt_v3_evidence_novelty.py` and its tests: frozen semantic mapping and Clippy-clean generated arithmetic.
- Modify `ci/rust-verification.toml`: classify both novelty verifier scripts as cheap-lane commands.
- Modify affected fixtures/tests: prove registry construction, direct APIs, recovery independence, evidence fail-closed behavior, internal whitespace, and checked deserialization.

---

### Task 1: Restore a Clippy-clean generated registry and freeze semantic meanings

**Files:**
- Modify: `scripts/verify_bolt_v3_evidence_novelty.py`
- Modify: `scripts/test_verify_bolt_v3_evidence_novelty.py`
- Regenerate: `src/bolt_v3_evidence_novelty/generated.rs`
- Modify: `ci/rust-verification.toml`

**Interfaces:**
- Consumes: `Registry.states: tuple[StateRow, ...]` loaded from `config/evidence-novelty.toml`.
- Produces: `FROZEN_MARKET_STATES: tuple[tuple[int, str], ...]` and generated `EVIDENCE_NOVELTY_WORD_COUNT` using `usize::div_ceil`.

- [ ] **Step 1: Add a mutation test that swaps two semantic IDs**

Add this test to `EvidenceNoveltyVerifierTests`:

```python
def test_permanent_ids_cannot_swap_semantic_meanings(self) -> None:
    text = self.registry_text()
    text = text.replace("id = 146", "id = 999", 1)
    text = text.replace("id = 147", "id = 146", 1)
    text = text.replace("id = 999", "id = 147", 1)
    with self.assertRaisesRegex(ValueError, "states must match frozen id-to-semantic mappings"):
        self.load_text(text)
```

- [ ] **Step 2: Run the verifier test and confirm RED**

Run: `python3 scripts/test_verify_bolt_v3_evidence_novelty.py`

Expected: FAIL because swapping two IDs currently passes `load_registry`.

- [ ] **Step 3: Freeze every ID-to-semantic mapping**

In `scripts/verify_bolt_v3_evidence_novelty.py`, add a constant containing every exact `(id, producer_kind, semantic_state)` triple from the committed TOML (32 rows after review closure):

```python
FROZEN_MARKET_STATES = (
    (144, "strategy_input_snapshot.blocked_rv.accepted.watermark_absent"),
    (145, "strategy_input_snapshot.blocked_rv.accepted.watermark_present"),
    (146, "strategy_input_snapshot.blocked_rv.missing_snapshot.watermark_absent"),
    (147, "strategy_input_snapshot.blocked_rv.missing_snapshot.watermark_present"),
    (148, "strategy_input_snapshot.blocked_rv.missing_evaluation_event_time.watermark_absent"),
    (149, "strategy_input_snapshot.blocked_rv.missing_evaluation_event_time.watermark_present"),
    (150, "strategy_input_snapshot.blocked_rv.rejected_future_dated.watermark_absent"),
    (151, "strategy_input_snapshot.blocked_rv.rejected_future_dated.watermark_present"),
    (152, "strategy_input_snapshot.blocked_rv.rejected_stale.watermark_absent"),
    (153, "strategy_input_snapshot.blocked_rv.rejected_stale.watermark_present"),
    (154, "strategy_input_snapshot.blocked_rv.rejected_not_ready.watermark_absent"),
    (155, "strategy_input_snapshot.blocked_rv.rejected_not_ready.watermark_present"),
    (156, "entry_skip.strategy_core_not_registered"),
    (157, "entry_skip.entry_gate_blocked"),
    (158, "entry_skip.entry_pricing_blocked"),
    (159, "entry_skip.no_side_selected"),
    (160, "entry_skip.sized_notional_not_positive"),
    (161, "entry_skip.instrument_id_missing"),
    (162, "entry_skip.instrument_missing_from_cache"),
    (163, "entry_skip.entry_price_missing"),
    (164, "entry_skip.quantity_rounding_failed"),
    (165, "entry_skip.limit_notional_exceeds_sized_notional"),
    (166, "entry_skip.entry_quote_notional_below_venue_minimum"),
    (167, "entry_skip.entry_quote_notional_minimum_unmodeled"),
    (168, "entry_skip.quantity_not_positive"),
    (169, "entry_skip.position_contract_invalid"),
    (170, "entry_skip.entry_position_contract_unsupported"),
    (171, "entry_skip.historical_entry_fee_unavailable"),
    (172, "entry_skip.one_position_invariant_violation"),
)
```

After parsing `states`, compare exact pairs:

```python
actual_states = tuple((row.id, row.semantic_state) for row in states)
if actual_states != FROZEN_MARKET_STATES:
    raise ValueError("states must match frozen id-to-semantic mappings")
```

Keep this tuple byte-for-byte aligned with the permanent semantics in `config/evidence-novelty.toml`; do not synthesize ranges or omit anchors.

- [ ] **Step 4: Emit Clippy-clean capacity arithmetic**

Replace generator expressions with:

```python
f"pub const EVIDENCE_NOVELTY_WORD_COUNT: usize = {registry.family_capacity}.div_ceil(64);",
"const _: () = assert!(",
"    EVIDENCE_NOVELTY_WORD_COUNT == EVIDENCE_NOVELTY_FAMILY_CAPACITY.div_ceil(64),",
'    "EVIDENCE_NOVELTY_WORD_COUNT must cover EVIDENCE_NOVELTY_FAMILY_CAPACITY"',
");",
```

- [ ] **Step 5: Register both novelty scripts as cheap-lane commands**

Add `test_verify_bolt_v3_evidence_novelty.py` and `verify_bolt_v3_evidence_novelty.py` to `local_lane_policy.cheap_lane_labels` in `ci/rust-verification.toml`, preserving sorted sibling grouping.

- [ ] **Step 6: Regenerate and verify GREEN locally**

Run:

```bash
python3 scripts/verify_bolt_v3_evidence_novelty.py --write
python3 scripts/test_verify_bolt_v3_evidence_novelty.py
python3 scripts/verify_bolt_v3_evidence_novelty.py
just fmt-check
just source-fence-static
git diff --check
```

Expected: all commands PASS; generated bytes contain `.div_ceil(64)`.

- [ ] **Step 7: Commit**

```bash
git add ci/rust-verification.toml scripts/verify_bolt_v3_evidence_novelty.py scripts/test_verify_bolt_v3_evidence_novelty.py src/bolt_v3_evidence_novelty/generated.rs
git commit -m "fix: freeze evidence novelty semantic ids"
```

---

### Task 2: Separate core discovery and lifecycle recovery from evidence-only metadata

**Files:**
- Modify: `src/bolt_v3_market_families/updown.rs`
- Modify: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/bolt_v3_market_families/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/selection.rs`
- Modify: `src/strategies/binary_oracle_maker/binding.rs`
- Test: `tests/bolt_v3_position_contract.rs`
- Test: `src/strategies/binary_oracle_maker/binding.rs`
- Test: `tests/bolt_v3_binary_oracle_maker_runtime.rs`

**Interfaces:**
- Produces: `UpdownPositionInstrumentContext` from lifecycle fields only.
- Produces: family-private `EvidenceOutcomeMetadata` containing canonical `market_id`, `condition_id`, `market_slug`, `question_id`, `negative_risk`, normalized outcome, and CLOB token.
- Produces: core `SelectedBinaryOptionMarket` without evidence-only fields and `selected_market_evidence_identity(...) -> Option<SelectedMarketEvidenceIdentity>` as an explicit enrichment seam.
- Taker selection requires core selection plus evidence enrichment; maker selection consumes only core selection; recovery consumes only the lifecycle projection.

- [ ] **Step 1: Add explicit lifecycle-without-evidence-metadata regressions**

Extend the position-contract fixture so it intentionally omits `neg_risk`, then preserve this assertion:

```rust
assert_eq!(recovered.market_id.as_deref(), Some("market-1"));
assert_eq!(recovered.outcome_side, Some(OutcomeSide::Up));
assert!(recovered.interval_end_ms.is_some());
```

In family tests, add instruments with lifecycle/core discovery fields but no `neg_risk`. Assert core selection succeeds, evidence enrichment returns `None`, taker selection fails closed, and maker resolution still succeeds.

- [ ] **Step 2: Use existing exact-head CI as RED evidence**

Record that run `29495972721` fails `bolt_v3_position_contract.rs:238` and maker binding/runtime assertions because `updown_outcome_instrument` requires `neg_risk` for all consumers.

- [ ] **Step 3: Split the up/down projection**

Refactor `updown_position_instrument_context` to read only fields it returns:

```rust
pub(crate) fn updown_position_instrument_context(
    instrument: &InstrumentAny,
) -> Option<UpdownPositionInstrumentContext> {
    let InstrumentAny::BinaryOption(binary) = instrument else { return None };
    let info = binary.info.as_ref()?;
    let side = match binary.outcome.as_ref()?.as_str() {
        "Up" => OutcomeSide::Up,
        "Down" => OutcomeSide::Down,
        _ => return None,
    };
    Some(UpdownPositionInstrumentContext {
        market_id: info.get_str(METADATA_MARKET_ID_FIELD)?.to_string(),
        side,
        interval_end_ms: u64::try_from(
            Duration::from_nanos(binary.expiration_ns.as_u64()).as_millis(),
        ).ok()?,
    })
}
```

Refactor `updown_outcome_instrument` into a core projection that does not read `neg_risk`, normalized evidence outcome, or CLOB token. Add `updown_evidence_outcome_metadata`, which accepts the selected binary option and requires those evidence fields. Do not use `unwrap_or(false)` and do not add `neg_risk` to lifecycle or maker fixtures.

- [ ] **Step 4: Apply the same projection rule to static events**

Split `static_outcome_instrument` into core discovery and explicit evidence enrichment with the same contracts as up/down. Static maker resolution must not require `neg_risk`; taker evidence selection must require it.

- [ ] **Step 5: Route maker and taker through their declared projections**

Remove `evidence_identity` from core `SelectedBinaryOptionMarket`. Add this function pointer to `MarketFamilyValidationBinding`:

```rust
pub selected_market_evidence_identity: fn(
    &toml::Value,
    &SelectedBinaryOptionMarket,
    &[InstrumentAny],
) -> Option<SelectedMarketEvidenceIdentity>,
```

The taker calls it immediately after core selection and stores the returned identity in `CandidateMarket`; `None` rejects the candidate. The maker never calls it. Unsupported families return `None`.

- [ ] **Step 6: Run local non-compile checks**

Run:

```bash
just fmt-check
just source-fence-static
git diff --check
```

Expected: PASS.

- [ ] **Step 7: Commit and run Rust Probe 1**

```bash
git add src/bolt_v3_market_families/mod.rs src/bolt_v3_market_families/updown.rs src/bolt_v3_market_families/static_binary_event.rs src/strategies/binary_oracle_edge_taker/selection.rs src/strategies/binary_oracle_maker/binding.rs tests/bolt_v3_position_contract.rs tests/bolt_v3_binary_oracle_maker_runtime.rs
git commit -m "fix: separate lifecycle and evidence metadata"
just sandbox-safe-push
just rust-probe check-lib
```

Expected: the library compiles and Clippy no longer reports `manual_div_ceil`. If the probe fails, fix only compile/type errors caused by Tasks 1–2, rerun local gates, amend with a new commit, and count the retry as the second and final permitted probe.

---

### Task 3: Introduce the validated target identity type

**Files:**
- Create: `src/bolt_v3_target_identity.rs`
- Modify: `src/lib.rs`
- Modify: `src/bolt_v3_evidence_novelty.rs`
- Modify: `src/bolt_v3_market_families/mod.rs`
- Modify: `src/bolt_v3_market_families/updown.rs`
- Modify: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/bolt_v3_market_families/outcome_group.rs`
- Modify: `src/bolt_v3_market_families/hyperliquid_instrument.rs`

**Interfaces:**
- Produces: `ConfiguredTargetId::new(String) -> Result<Self, ConfiguredTargetIdError>`.
- Produces: `ConfiguredTargetId::as_str(&self) -> &str` and checked `Serialize`/`Deserialize`.
- Produces: `stable_identity_field_is_canonical(&str) -> bool` as the one shared predicate.

- [ ] **Step 1: Write newtype unit tests first**

Create tests in `src/bolt_v3_target_identity.rs`:

```rust
#[test]
fn configured_target_id_rejects_every_malformed_class() {
    for value in ["", "   ", " target", "target "] {
        assert!(ConfiguredTargetId::try_from(value).is_err(), "accepted {value:?}");
    }
}

#[test]
fn configured_target_id_round_trips_checked_serde() {
    let id = ConfiguredTargetId::try_from("target-a").expect("canonical id");
    let encoded = serde_json::to_string(&id).expect("serialize");
    assert_eq!(serde_json::from_str::<ConfiguredTargetId>(&encoded).unwrap(), id);
    assert!(serde_json::from_str::<ConfiguredTargetId>(r#"" target""#).is_err());
}

#[test]
fn stable_fields_allow_internal_whitespace() {
    assert!(stable_identity_field_is_canonical("New York Yes"));
}
```

- [ ] **Step 2: Implement the newtype**

Create:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ConfiguredTargetId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredTargetIdError;

pub fn stable_identity_field_is_canonical(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

impl TryFrom<&str> for ConfiguredTargetId {
    type Error = ConfiguredTargetIdError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value.to_string())
    }
}

impl TryFrom<String> for ConfiguredTargetId {
    type Error = ConfiguredTargetIdError;
    fn try_from(value: String) -> Result<Self, Self::Error> { Self::new(value) }
}

impl<'de> Deserialize<'de> for ConfiguredTargetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value.as_str()).map_err(|_| {
            serde::de::Error::custom("configured_target_id must be a non-empty, unpadded string")
        })
    }
}

impl ConfiguredTargetId {
    pub fn new(value: String) -> Result<Self, ConfiguredTargetIdError> {
        stable_identity_field_is_canonical(value.as_str())
            .then_some(Self(value))
            .ok_or(ConfiguredTargetIdError)
    }

    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for ConfiguredTargetId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
```

Add `pub mod bolt_v3_target_identity;` to `src/lib.rs`. Move the predicate import in evidence novelty and market families to this module.

- [ ] **Step 3: Replace target-block identity strings**

Change every family `TargetBlock.configured_target_id`, `TargetMetadata.configured_target_id`, family plan identity, and `TargetRuntimeFields.configured_target_id` to `ConfiguredTargetId`. Remove family-local trim/empty checks now enforced by checked deserialization.

- [ ] **Step 4: Short-circuit dispatch before injected bindings**

Make raw extraction return the typed identity:

```rust
fn configured_target_id_from_target(
    context: &str,
    target: &toml::Value,
) -> Result<ConfiguredTargetId, InstrumentFilterError> {
    let value = target.as_table()
        .and_then(|table| table.get(stringify!(configured_target_id)))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| InstrumentFilterError::TargetValidationFailure {
            message: format!("{context}: target.configured_target_id must be a non-empty, unpadded string"),
        })?;
    ConfiguredTargetId::try_from(value).map_err(|_| {
        InstrumentFilterError::TargetValidationFailure {
            message: format!("{context}: target.configured_target_id must be a non-empty, unpadded string"),
        }
    })
}
```

In `validate_strategy_target_with_bindings`, return immediately with this error before resolving or invoking a binding. Add an atomic call-count fake binding test proving the binding count remains zero.

- [ ] **Step 5: Gate direct family APIs**

Ensure each public `validate_target_block`, `plan_strategy_target`, and `target_runtime_fields` either deserializes a `TargetBlock` containing `ConfiguredTargetId` or accepts the validated type. Remove `InvalidConfiguredTargetId` from `BoltV3MarketIdentityError` once it becomes unreachable after typed deserialization, then update exhaustive matches and tests.

- [ ] **Step 6: Run local gates and commit**

```bash
python3 scripts/test_verify_bolt_v3_evidence_novelty.py
python3 scripts/verify_bolt_v3_evidence_novelty.py
just fmt-check
just source-fence-static
just deny
git diff --check
git add src/lib.rs src/bolt_v3_target_identity.rs src/bolt_v3_evidence_novelty.rs src/bolt_v3_market_families
git commit -m "refactor: make configured target identity validated"
```

Expected: all local gates PASS.

---

### Task 4: Carry validated identity through strategy construction and selection

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/config.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/selection.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_maker/binding.rs`
- Modify: `src/strategies/registry.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/selection.rs`
- Test: `src/strategies/binary_oracle_maker/binding.rs`
- Test: `src/strategies/registry.rs`

**Interfaces:**
- `MarketSelectionTarget<'a>` gains `configured_target_id: &'a ConfiguredTargetId`.
- Taker config and maker declaration store `ConfiguredTargetId`.
- Registry `build` and `register_strategy` cannot construct strategies from malformed IDs because strategy config Serde is checked.

- [ ] **Step 1: Add registry and selection RED tests**

For each malformed value, build a raw valid strategy config with only `configured_target_id` changed and assert both APIs fail:

```rust
for malformed in ["", "   ", " target", "target "] {
    let raw = fixture_raw_strategy_with_target_id(malformed);
    assert!(registry.build(BINARY_ORACLE_EDGE_TAKER_KIND, &raw, &context).is_err());
    assert!(registry.register_strategy(BINARY_ORACLE_EDGE_TAKER_KIND, &raw, &context, &trader).is_err());
}
```

Add maker and taker selection tests constructing `MarketSelectionTarget` only from a valid `ConfiguredTargetId`; malformed construction must fail before selection can be called.

- [ ] **Step 2: Migrate strategy config and declarations**

Change the taker config macro field from:

```rust
configured_target_id: String => String;
```

to:

```rust
configured_target_id: ConfiguredTargetId => ConfiguredTargetId;
```

Change `MakerMarketDeclaration.configured_target_id` in `src/strategies/binary_oracle_maker/binding.rs` to `ConfiguredTargetId`. At evidence/logging boundaries, call `.as_str()` or `.to_string()` through an explicit `Display` implementation; do not expose the inner field.

- [ ] **Step 3: Carry identity through selection targets**

Add:

```rust
pub struct MarketSelectionTarget<'a> {
    pub configured_target_id: &'a ConfiguredTargetId,
    // existing fields unchanged
}
```

Populate it in both `binary_oracle_edge_taker/selection.rs` and `binary_oracle_maker/binding.rs`. Family selectors use this value only as validated identity context; they must not reconstruct it from raw TOML.

- [ ] **Step 4: Remove redundant string validation paths**

Delete `configured_target_identity_error`, `ensure_configured_target_identity`, and any family-specific empty/trim branches superseded by `ConfiguredTargetId` deserialization. Keep independent validation for provider metadata and `EvidenceEpisodeId` components because those are separate trust boundaries.

- [ ] **Step 5: Run local gates and commit**

```bash
just fmt-check
just source-fence-static
just deny
git diff --check
git add src/strategies src/bolt_v3_market_families/mod.rs
git commit -m "refactor: carry validated identity through selection"
```

Expected: all local gates PASS.

---

### Task 5: Make selected-market requirements immutable and checked on Serde

**Files:**
- Modify: `src/bolt_v3_market_families/mod.rs`
- Modify: `src/bolt_v3_market_families/updown.rs`
- Modify: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/bolt_v3_market_families/hyperliquid_instrument.rs`
- Modify consumers found by `rg -n "\.configured_target_id|\.selected_market_key" src tests` after fields become private.

**Interfaces:**
- `SelectedMarketRequirement` stores private `configured_target_id: ConfiguredTargetId` and private derived key.
- Accessors: `configured_target_id(&self) -> &ConfiguredTargetId`, `selected_market_key(&self) -> &str`, plus read-only accessors required by existing consumers.
- Custom `Deserialize` rejects a serialized selected-market key that does not match the immutable identity fields.

- [ ] **Step 1: Add malformed and inconsistent Serde tests**

```rust
#[test]
fn selected_market_requirement_deserialization_rechecks_identity_and_key() {
    let requirement = selected_market_requirement_from_parts(fixture_requirement_parts(
        "fixture-market", 123,
    )).expect("valid requirement");
    let mut json = serde_json::to_value(&requirement).expect("serialize");
    json["configured_target_id"] = serde_json::json!(" target-a");
    assert!(serde_json::from_value::<SelectedMarketRequirement>(json).is_err());

    let mut json = serde_json::to_value(&requirement).expect("serialize");
    json["selected_market_key"] = serde_json::json!("forged");
    assert!(serde_json::from_value::<SelectedMarketRequirement>(json).is_err());
}
```

- [ ] **Step 2: Store the validated type and privatize fields**

Change `SelectedMarketRequirement.configured_target_id` to private `ConfiguredTargetId`. Make all other identity/key fields private and add only read-only accessors needed by existing consumers. Change `SelectedMarketRequirementParts.configured_target_id` to `&ConfiguredTargetId`.

- [ ] **Step 3: Implement checked deserialization**

Introduce a private wire struct with the serialized field names. Convert its `configured_target_id` through checked Serde, build `SelectedMarketRequirement`, recompute `selected_market_key_for_requirement`, and reject if the supplied key differs:

```rust
let expected = selected_market_key_for_requirement(&requirement)
    .map_err(serde::de::Error::custom)?;
if requirement.selected_market_key != expected {
    return Err(serde::de::Error::custom("selected_market_key does not match identity fields"));
}
```

Retain derived `Serialize`; remove derived `Deserialize` from the public struct.

- [ ] **Step 4: Update family constructors and consumers**

Pass the already validated ID into `SelectedMarketRequirementParts`. Replace direct field reads with accessors. Do not add setters or unchecked constructors.

- [ ] **Step 5: Run local gates and commit**

```bash
just fmt-check
just source-fence-static
git diff --check
git add src/bolt_v3_market_families src tests
git commit -m "refactor: close selected market requirement identity"
```

Expected: all local gates PASS.

---

### Task 6: Execute the full contract matrix and exact-head review gate

**Files:**
- Test: `src/bolt_v3_target_identity.rs`
- Test: `src/bolt_v3_market_families/mod.rs`
- Test: `src/bolt_v3_market_families/updown.rs`
- Test: `src/bolt_v3_market_families/static_binary_event.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/selection.rs`
- Test: `src/strategies/binary_oracle_maker/binding.rs`
- Test: `tests/bolt_v3_position_contract.rs`
- Test: `tests/bolt_v3_evidence_novelty.rs`

**Interfaces:**
- Consumes all previous task outputs.
- Produces exact-head evidence that the repository-wide contract is closed.

- [ ] **Step 1: Audit the contract matrix structurally**

Run targeted searches and confirm every row has an executable test:

```bash
rg -n "ConfiguredTargetId|configured_target_id" src tests
rg -n "MarketSelectionTarget \{" src tests
rg -n "SelectedMarketRequirement \{" src tests
rg -n "updown_outcome_instrument|updown_position_instrument_context|static_outcome_instrument" src tests
rg -n "FROZEN_MARKET_STATES|div_ceil" scripts src
```

Expected: no raw public identity field, unchecked struct literal, or shared recovery/evidence constructor remains.

- [ ] **Step 2: Run all permitted local checks**

```bash
python3 scripts/test_verify_bolt_v3_evidence_novelty.py
python3 scripts/verify_bolt_v3_evidence_novelty.py
just fmt-check
just deny
just ci-lint-workflow
just source-fence-static
git diff --check
```

Expected: all PASS.

- [ ] **Step 3: Commit any final test-only closure**

```bash
git add src tests scripts ci docs
git commit -m "test: close target identity contract matrix"
```

Skip this commit if Step 1 finds no missing test and the worktree is already clean.

- [ ] **Step 4: Push and run Rust Probe 2**

State before dispatch: changed files, suspected class (`identity type migration and lifecycle/evidence separation`), mode (`check-lib` followed by the suggested targeted nextest command only if check-lib succeeds`), and why this is smallest sufficient.

```bash
just sandbox-safe-push
just rust-probe suggest
just rust-probe check-lib
```

If Probe 1 already required a retry, do not dispatch another probe; proceed only after explaining the two-run limit and use full remote feedback on the coherent head. Expected: compile succeeds with no Clippy warnings.

- [ ] **Step 5: Run exact-head remote verification**

```bash
just verify-remote
git rev-parse HEAD
gh pr checks 1429
```

Expected: remote SHA equals local `HEAD`; required `clippy`, `nextest archive`, `test`, `ci-provenance-emit`, and `gate` are green. Report any red or pending status plainly and trace it before another change.

- [ ] **Step 6: Request whole-PR adversarial review**

The review prompt must name the exact head and require:

- raw config through registry build/register;
- every direct family and injected-binding seam;
- maker/taker selection;
- lifecycle recovery without evidence metadata;
- evidence selection with complete/missing metadata;
- immutable checked requirements;
- every frozen semantic ID;
- generated Rust compiler/Clippy compatibility;
- exact-head CI causal diagnosis.

Expected: no unresolved substantive findings. If a finding is valid, return to the owning task, add a regression first, fix it, rerun local gates, push a new exact head, and repeat the full review rather than narrowing the prompt.

- [ ] **Step 7: Final evidence record**

Report the exact head, local gate results, remote required statuses, resolved review findings, and remaining verification gaps. Do not merge, deploy, or trade.
