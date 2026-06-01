# T036H15 Selected-Market Requirement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement provider-neutral selected-market requirement extraction for market-family selections, including config-resolved resolution identity, market-complete sorted instrument ids, metadata provenance hashing, and canonical `selected_market_key`.

**Architecture:** Keep the generic API and canonical hashing in `src/bolt_v3_market_families/mod.rs`; keep updown target parsing, mapping selection, and binary-option identity extraction in `src/bolt_v3_market_families/updown.rs`. Do not add `GateEvidence`, `GateSatisfaction`, `EntryReadinessGateSession`, provider collectors, CLI flags, or consumer rewiring in this slice.

**Tech Stack:** Rust, `serde`, `serde_json`, `sha2`, NT `InstrumentId`, existing `InstrumentFilterError`.

---

## Scope Boundary

T036H15 is limited to selected-market requirement extraction. It may add tests in the two market-family modules. It must not modify provider collection, operator-artifact session joins, decision evidence, tiny/live canary consumers, or PR state.

## Files

- Modify: `src/bolt_v3_market_families/mod.rs`
  - Add `SelectedMarketRequirement`.
  - Add canonical identity/provenance hashing helpers that build `BTreeMap` inputs so JSON object keys are lexicographically sorted before hashing.
  - Add a generic dispatcher function such as `selected_market_requirement_from_target`.
  - Extend `MarketFamilyValidationBinding` with a requirement-extraction callback.
  - Add generic dispatcher tests for injected family bindings and canonical hash determinism.
- Modify: `src/bolt_v3_market_families/updown.rs`
  - Implement the updown callback.
  - Resolve `resolution_kind`, `resolution_identity`, and `value_kind` from `target.gate_subscriptions.resolution.market_mappings`.
  - Derive venue from the NT selected market instruments and require a single matching venue.
  - Use sorted `up_instrument_id`/`down_instrument_id` as the market-complete instrument set.
  - Compute `metadata_provenance_sha256` from canonical JSON containing normalized metadata source fields already available to this slice: `condition_id`, `family_key`, sorted instrument ids, `market_class`, `market_id`, `market_slug`, `question_id`, `source_kind = "nt_instrument_metadata"`, and venue.

## Planned Public API

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SelectedMarketRequirement {
    pub configured_target_id: String,
    pub venue: String,
    pub family_key: String,
    pub market_id: String,
    pub instrument_ids: Vec<String>,
    pub market_class: String,
    pub resolution_kind: String,
    pub resolution_identity: String,
    pub value_kind: String,
    pub metadata_provenance_sha256: String,
    pub selected_market_key: String,
    pub selected_at_ms: u64,
}

pub fn selected_market_requirement_from_target(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError>;
```

The canonical key hash input excludes `selected_at_ms` and contains exactly:

```json
{
  "configured_target_id": "...",
  "family_key": "...",
  "instrument_ids": ["..."],
  "market_class": "...",
  "market_id": "...",
  "metadata_provenance_sha256": "...",
  "resolution_kind": "...",
  "resolution_identity": "...",
  "value_kind": "...",
  "venue": "..."
}
```

The implementation must construct this hash input separately from `SelectedMarketRequirement`; it must never serialize the full requirement because that would include `selected_at_ms` and `selected_market_key` in the hash. `selected_at_ms` is the millisecond POSIX timestamp captured by the caller at the moment the market is selected from instrument metadata.

`market_class`, `resolution_kind`, and `value_kind` remain `String` in this slice to avoid pulling T036H16 enum work forward. Their values must exactly match the contract's lower snake-case enum serialization names, for example `binary_option` and `price`.

## Task 1: Add RED Generic Dispatcher And Canonicalization Tests

- [ ] Add a test in `src/bolt_v3_market_families/mod.rs` proving an injected fake family binding owns selected-market requirement extraction and returns a deterministic fixture requirement. This fails before the binding callback and dispatcher exist.

```rust
#[test]
fn selected_market_requirement_uses_injected_family_binding_without_parent_family_branch() {
    let target = toml::toml! {
        configured_target_id = "fixture-target"
        rotating_market_family = "fixture_family"
    }
    .into();
    let selected = fake_selected_binary_option_market();

    let production_error = selected_market_requirement_from_target(&target, &selected, 123)
        .expect_err("production registry should not know the test family");
    assert!(production_error.to_string().contains("not supported by this build"));

    let requirement = selected_market_requirement_from_target_with_bindings(
        &target,
        &selected,
        123,
        FAKE_FAMILY_BINDINGS,
    )
    .expect("injected family binding should own requirement extraction");

    assert_eq!(requirement.configured_target_id, "fixture-target");
    assert_eq!(requirement.family_key, "fixture_family");
    assert_eq!(requirement.selected_at_ms, 123);
}
```

- [ ] Add a test in `src/bolt_v3_market_families/mod.rs` proving `selected_market_key` is unchanged when only `selected_at_ms` changes, and changes when a canonical identity field changes. This fails before canonical hash helpers exist.

- [ ] Add a test proving canonical helpers use lexicographically sorted JSON keys by comparing the hash against an explicitly sorted `BTreeMap` input. This prevents struct-field-order drift from changing `selected_market_key`.

Run:

```bash
cargo test --lib bolt_v3_market_families::tests::selected_market_requirement -- --nocapture
```

Expected before implementation: compile failure for missing API or test failure for missing canonical hashing.

## Task 2: Add RED Updown Requirement Tests

- [ ] Add a test in `src/bolt_v3_market_families/updown.rs` proving an updown selected market emits:
  - `configured_target_id` from target TOML.
  - venue from matching NT instrument ids.
  - `family_key = "updown"`.
  - `market_id` from selected market metadata.
  - sorted full `[down, up]` or lexicographic instrument id set, not only the traded `instrument_id`.
  - `market_class = "binary_option"`.
  - resolution fields from the matching `target.gate_subscriptions.resolution.market_mappings` row.
  - lowercase 64-char `metadata_provenance_sha256`.
  - lowercase 64-char `selected_market_key`.

- [ ] Add a test proving extraction fails closed when venue metadata does not provide a resolution identity and the target has no matching config-owned mapping.

- [ ] Add tests proving extraction fails closed when `target.gate_subscriptions` is absent, when the `resolution` subscription is absent, and when the `resolution.market_mappings` list is absent or empty.

- [ ] Add a test proving extraction fails closed when multiple mapping rows match `(family_key = "updown", market_class = "binary_option")` at extraction time with different resolution identities.

- [ ] Add a test proving extraction fails closed when a selected market's up/down instrument venues differ.

- [ ] Add a test proving extraction fails closed when any string flowing into selected-market key or metadata provenance contains `|`; the check must cover target id, venue, family key, market id, instrument ids, market class, resolution fields, value kind, and source identity fields.

Run:

```bash
cargo test --lib bolt_v3_market_families::updown::tests::selected_market_requirement -- --nocapture
```

Expected before implementation: compile failure for missing API or test failure for missing extraction.

## Task 3: Implement Generic API In `mod.rs`

- [ ] Extend `MarketFamilyValidationBinding`:

```rust
pub selected_market_requirement:
    fn(&toml::Value, &SelectedBinaryOptionMarket, u64)
        -> Result<SelectedMarketRequirement, InstrumentFilterError>,
```

- [ ] Add `SelectedMarketRequirement`.

- [ ] Add canonical hashing helpers using `BTreeMap` and `serde_json::to_vec` plus `hex::encode(Sha256::digest(bytes))`.

- [ ] Add separate `selected_market_key` and metadata-provenance hash-input builders that take only the allowed fields.

- [ ] Add `selected_market_requirement_from_target` and `_with_bindings` dispatcher mirroring `target_runtime_fields_from_target`.

- [ ] Update the production `UPDOWN_BINDING` and the test `FAKE_FAMILY_BINDINGS` with the new callback.

Run:

```bash
cargo test --lib bolt_v3_market_families::tests::selected_market_requirement -- --nocapture
```

Expected after this task: generic tests pass; updown tests may still fail.

## Task 4: Implement Updown Extraction

- [ ] Add the updown binding callback in `validation_binding()`.

- [ ] Deserialize `TargetBlock` from the raw `toml::Value`.

- [ ] Find exactly one `target.gate_subscriptions.resolution.market_mappings` row where:
  - `family_key == "updown"`.
  - `market_class == "binary_option"`.
  - `value_kind`, `resolution_kind`, and `resolution_identity` are non-empty.
  - No selected-market key component contains `|`.

- [ ] Keep a short source comment in the updown callback explaining that this slice extracts the `resolution` selected-market requirement because current updown readiness uses the resolution role; role joins and additional roles remain T036H16+.

- [ ] Prefer any future venue-native resolution fields inside selected-market metadata before config mappings. Current updown `BinaryOption` metadata does not expose those fields, so this slice safely fails closed unless the config-owned mapping resolves the identity.

- [ ] Build sorted `instrument_ids` from `selected.up_instrument_id` and `selected.down_instrument_id`; require the two venues match via `InstrumentId.venue`.

- [ ] Sort `instrument_ids` lexicographically on `InstrumentId.to_string()` using the full `SYMBOL.VENUE` representation.

- [ ] Build metadata provenance canonical JSON from the updown market metadata available in `SelectedBinaryOptionMarket`:

```json
{
  "condition_id": "...",
  "family_key": "updown",
  "instrument_ids": ["..."],
  "market_class": "binary_option",
  "market_id": "...",
  "market_slug": "...",
  "question_id": "...",
  "source_kind": "nt_instrument_metadata",
  "venue": "..."
}
```

- [ ] Return `SelectedMarketRequirement` with canonical hashes.

Run:

```bash
cargo test --lib bolt_v3_market_families::updown::tests::selected_market_requirement -- --nocapture
```

Expected after this task: updown tests pass.

## Task 5: Focused And Full Verification

- [x] Run:

```bash
cargo fmt --check
git diff --check
cargo test --lib bolt_v3_market_families -- --nocapture
cargo test --test config_parsing -- --nocapture
cargo test --test bolt_v3_operator_artifacts market_selection -- --nocapture
just clippy
just source-fence
just test
```

- [x] Confirm `specs/024-production-trade-readiness/tasks.md` marks only `T036H15` complete if and only if verification passes.

## Self-Review

- Spec coverage: Covers selected-market identity fields, config-resolved mapping, sorted market-complete instrument ids, metadata provenance hash, canonical key derivation, and fail-closed cases. Leaves GateEvidence/session/consumer/provider work to T036H16-T036H19.
- Placeholder scan: No deferred implementation placeholders are planned.
- Type consistency: Public API names are consistent across generic and updown tasks.
- Review disposition: DeepSeek and GLM plan-review findings are folded in: canonical JSON uses `BTreeMap`, hash inputs are separate from the public requirement type, binding const updates are explicit, extraction-time ambiguity and absent-subscription tests are added, provenance includes a source discriminator, `|` rejection is uniform, and `selected_at_ms` plus instrument-id string semantics are specified.
