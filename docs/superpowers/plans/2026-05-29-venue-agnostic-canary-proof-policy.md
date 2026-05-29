# Venue-Agnostic Canary Proof Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a config-owned canary proof policy that can emit one capped proof-only order from strategy-owned candidates without hardcoding venue, market, or strategy behavior.

**Architecture:** Add a generic proof-policy layer between strategy evidence and submit admission. Strategies expose normalized proof candidates; adapters expose normalized instrument constraints; the proof policy converts an approved candidate into the existing order-intent path with `proof_only` evidence labels.

**Tech Stack:** Rust, NautilusTrader Rust API, TOML config, existing Bolt-v3 operator artifacts, existing live canary gate, existing submit admission.

---

## File Structure

- Modify `src/bolt_v3_config.rs`: parse `[live_canary.proof_policy]` and operator evidence path bindings.
- Modify `src/bolt_v3_validate.rs`: validate proof-policy fields without venue/market literals.
- Create `src/bolt_v3_canary_proof_policy.rs`: generic proof policy, candidate selection, constraint normalization, and fail-closed reasons.
- Modify `src/bolt_v3_submit_admission.rs`: accept proof-only order intents through the existing cap checks.
- Modify `src/bolt_v3_operator_artifacts.rs`: generate and verify proof-policy, candidate, rotation, and proof-order-intent artifacts.
- Modify `src/strategies/mod.rs`: expose a generic strategy proof-candidate provider registry.
- Add `tests/support/canary_proof_strategy.rs`: test-only provider that proves the core policy has no concrete strategy dependency.
- Modify `src/bolt_v3_live_node.rs`: route proof-only order intents through the normal submit path when final canary gate admits them.
- Modify `src/main.rs`: add operator-artifact subcommands for proof policy, rotation observation, and proof order intent materialization.
- Add `tests/bolt_v3_canary_proof_policy.rs`: policy and constraint unit tests.
- Add `tests/bolt_v3_canary_proof_operator_artifacts.rs`: artifact and final-packet tests.
- Add `tests/bolt_v3_canary_proof_live_gate.rs`: approval/gate tests.
- Update `config/root.toml` and `config/live.local.toml`: disabled-by-default proof policy example values.
- Update `specs/024-production-trade-readiness/tiny-canary.md`: operator runbook text for proof-only T044.

## Task 1: Config Shape And Validation

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate.rs`
- Modify: `tests/config_parsing.rs`
- Modify: `config/root.toml`

- [ ] **Step 1: Write failing config parse tests**

Add tests in `tests/config_parsing.rs`:

```rust
#[test]
fn live_canary_proof_policy_is_disabled_by_default() {
    let loaded = load_bolt_v3_fixture_config("root.toml");
    let live_canary = loaded.root.live_canary.expect("live canary configured");
    assert!(live_canary.proof_policy.is_none() || !live_canary.proof_policy.unwrap().enabled);
}

#[test]
fn live_canary_proof_policy_rejects_non_proof_claim() {
    let err = parse_bolt_v3_config_with_patch(
        r#"
        [live_canary.proof_policy]
        enabled = true
        policy_kind = "least_bad_strategy_candidate"
        proof_claim = "alpha_ready"
        strategy_instance_id = "strategy-a"
        execution_client_id = "exec-a"
        notional_mode = "fixed"
        proof_notional = "1.00"
        candidate_score_source = "strategy_evidence"
        allow_negative_expected_ev = true
        rotation_observation_enabled = true
        rotation_min_distinct_markets = 1
        rotation_max_attempts = 1
        "#
    )
    .expect_err("non proof claim must fail");
    assert!(err.to_string().contains("proof_claim"));
    assert!(err.to_string().contains("proof_only"));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --test config_parsing live_canary_proof_policy -- --nocapture`

Expected: tests fail because `proof_policy` is not parsed.

- [ ] **Step 3: Add config structs**

In `src/bolt_v3_config.rs`, add:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BoltV3LiveCanaryProofPolicyConfig {
    pub enabled: bool,
    pub policy_kind: String,
    pub proof_claim: String,
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub notional_mode: String,
    pub proof_notional: String,
    pub candidate_score_source: String,
    pub allow_negative_expected_ev: bool,
    pub rotation_observation_enabled: bool,
    pub rotation_min_distinct_markets: u32,
    pub rotation_max_attempts: u32,
}
```

Add `pub proof_policy: Option<BoltV3LiveCanaryProofPolicyConfig>` to the existing live canary config struct using the existing serde style in that file.

Do not add config toggles for source readiness, strategy candidate presence, instrument constraints, or submit admission. Those gates are mandatory invariants of proof mode and are enforced by the policy, final-packet verifier, and live submit path.

- [ ] **Step 4: Add validation**

In `src/bolt_v3_validate.rs`, add validation called from the existing live canary validation path:

```rust
fn validate_live_canary_proof_policy(
    context: &str,
    policy: &BoltV3LiveCanaryProofPolicyConfig,
    max_notional_per_order: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    if policy.proof_claim != "proof_only" {
        errors.push(format!("{context}: live_canary.proof_policy.proof_claim must be `proof_only`"));
    }
    if policy.policy_kind != "least_bad_strategy_candidate" {
        errors.push(format!("{context}: live_canary.proof_policy.policy_kind is unsupported"));
    }
    if policy.notional_mode != "fixed" {
        errors.push(format!("{context}: live_canary.proof_policy.notional_mode must be `fixed`"));
    }
    if policy.candidate_score_source != "strategy_evidence" {
        errors.push(format!("{context}: live_canary.proof_policy.candidate_score_source must be `strategy_evidence`"));
    }
    if policy.strategy_instance_id.trim().is_empty() || policy.strategy_instance_id.trim() != policy.strategy_instance_id {
        errors.push(format!("{context}: live_canary.proof_policy.strategy_instance_id is invalid"));
    }
    if policy.execution_client_id.trim().is_empty() || policy.execution_client_id.trim() != policy.execution_client_id {
        errors.push(format!("{context}: live_canary.proof_policy.execution_client_id is invalid"));
    }
    let proof_notional = crate::bolt_v3_validate::parse_decimal_string(&policy.proof_notional);
    let max_notional = crate::bolt_v3_validate::parse_decimal_string(max_notional_per_order);
    match (proof_notional, max_notional) {
        (Ok(proof), Ok(_max)) if proof <= rust_decimal::Decimal::ZERO => {
            errors.push(format!("{context}: live_canary.proof_policy.proof_notional must be positive"));
        }
        (Ok(proof), Ok(max)) if proof > max => {
            errors.push(format!("{context}: live_canary.proof_policy.proof_notional must be <= live_canary.max_notional_per_order"));
        }
        (Err(reason), _) => errors.push(format!("{context}: live_canary.proof_policy.proof_notional is invalid: {reason}")),
        (_, Err(reason)) => errors.push(format!("{context}: live_canary.max_notional_per_order is invalid: {reason}")),
        _ => {}
    }
    if policy.rotation_min_distinct_markets == 0 {
        errors.push(format!("{context}: live_canary.proof_policy.rotation_min_distinct_markets must be positive"));
    }
    if policy.rotation_max_attempts == 0 {
        errors.push(format!("{context}: live_canary.proof_policy.rotation_max_attempts must be positive"));
    }
    if policy.rotation_min_distinct_markets > policy.rotation_max_attempts {
        errors.push(format!("{context}: live_canary.proof_policy.rotation_min_distinct_markets must be <= rotation_max_attempts"));
    }
    errors
}
```

Wire this validator from the existing full-config validation context, not from an isolated parser helper. The full-context validation must also prove:

- `strategy_instance_id` resolves to exactly one configured strategy.
- `execution_client_id` resolves to exactly one configured execution client.
- the selected strategy can route to the configured execution client or explicitly declares an execution-client override.
- `proof_notional` is less than or equal to the selected strategy's configured risk maximum when such a cap exists.
- `enabled = true` is rejected unless the operator-evidence section includes all proof artifact paths and hashes required by Task 4.

- [ ] **Step 5: Add disabled example config**

In `config/root.toml`, add:

```toml
[live_canary.proof_policy]
enabled = false
policy_kind = "least_bad_strategy_candidate"
proof_claim = "proof_only"
strategy_instance_id = "configured-strategy-id"
execution_client_id = "configured-execution-client-id"
notional_mode = "fixed"
proof_notional = "1.00"
candidate_score_source = "strategy_evidence"
allow_negative_expected_ev = true
rotation_observation_enabled = true
rotation_min_distinct_markets = 1
rotation_max_attempts = 1
```

- [ ] **Step 6: Verify and commit**

Run: `cargo test --test config_parsing live_canary_proof_policy -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/bolt_v3_config.rs src/bolt_v3_validate.rs tests/config_parsing.rs config/root.toml
git commit -m "feat(canary): add proof policy config"
```

## Task 2: Generic Proof Policy Module

**Files:**
- Create: `src/bolt_v3_canary_proof_policy.rs`
- Modify: `src/lib.rs`
- Test: `tests/bolt_v3_canary_proof_policy.rs`

- [ ] **Step 1: Write failing policy tests**

Create `tests/bolt_v3_canary_proof_policy.rs`:

```rust
use bolt_v2::bolt_v3_canary_proof_policy::{
    select_canary_proof_candidate, CanaryProofCandidate, CanaryProofOrderSide,
    CanaryProofOrderType, CanaryProofPolicyInput,
    CanaryProofPolicyRejection,
};
use rust_decimal::Decimal;

#[test]
fn proof_policy_selects_highest_scored_source_bound_candidate_without_venue_names() {
    let input = CanaryProofPolicyInput::fixture()
        .with_proof_notional("1.00")
        .with_current_source_ref("source-hash-a")
        .with_candidate(
            CanaryProofCandidate::fixture()
                .with_instrument_id("instrument-a")
                .with_order_side(CanaryProofOrderSide::Buy)
                .with_score("-12.5")
                .with_source_ref("source-hash-a"),
        )
        .with_candidate(
            CanaryProofCandidate::fixture()
                .with_instrument_id("instrument-b")
                .with_order_side(CanaryProofOrderSide::Sell)
                .with_score("-7.5")
                .with_source_ref("source-hash-a"),
        );

    let selected = select_canary_proof_candidate(&input).expect("candidate selected");

    assert_eq!(selected.instrument_id, "instrument-b");
    assert_eq!(selected.proof_claim, "proof_only");
    assert!(selected.sizing.notional_for_submit_admission() > Decimal::ZERO);
}

#[test]
fn proof_policy_rejects_candidate_without_current_source_ref() {
    let input = CanaryProofPolicyInput::fixture()
        .with_current_source_ref("source-hash-a")
        .with_candidate(CanaryProofCandidate::fixture().with_source_ref("source-hash-b"));

    let err = select_canary_proof_candidate(&input).expect_err("source mismatch rejected");

    assert_eq!(err, CanaryProofPolicyRejection::ProofCandidateSourceMismatch);
}

#[test]
fn proof_policy_rejects_candidate_without_source_refs() {
    let input = CanaryProofPolicyInput::fixture()
        .with_current_source_ref("source-hash-a")
        .with_candidate(CanaryProofCandidate::fixture().without_source_refs());

    let err = select_canary_proof_candidate(&input).expect_err("empty source refs rejected");

    assert_eq!(err, CanaryProofPolicyRejection::ProofCandidateSourceMismatch);
}

#[test]
fn proof_policy_rejects_order_type_not_supported_by_constraints() {
    let input = CanaryProofPolicyInput::fixture()
        .with_current_source_ref("source-hash-a")
        .with_constraints_supporting([CanaryProofOrderType::Limit])
        .with_candidate(
            CanaryProofCandidate::fixture()
                .with_order_type(CanaryProofOrderType::Market)
                .with_source_ref("source-hash-a"),
        );

    let err = select_canary_proof_candidate(&input).expect_err("unsupported order type rejected");

    assert_eq!(err, CanaryProofPolicyRejection::InstrumentConstraintsRejectOrderType);
}

#[test]
fn proof_policy_rejects_negative_ev_when_disallowed() {
    let input = CanaryProofPolicyInput::fixture()
        .with_allow_negative_expected_ev(false)
        .with_current_source_ref("source-hash-a")
        .with_candidate(CanaryProofCandidate::fixture().with_score("-0.01").with_source_ref("source-hash-a"));

    let err = select_canary_proof_candidate(&input).expect_err("negative ev rejected");

    assert_eq!(err, CanaryProofPolicyRejection::ProofPolicyNegativeEvDisallowed);
}

#[test]
fn proof_policy_filters_negative_ev_candidates_before_selecting_survivor() {
    let input = CanaryProofPolicyInput::fixture()
        .with_allow_negative_expected_ev(false)
        .with_current_source_ref("source-hash-a")
        .with_candidate(CanaryProofCandidate::fixture().with_score("-0.01").with_source_ref("source-hash-a"))
        .with_candidate(
            CanaryProofCandidate::fixture()
                .with_instrument_id("instrument-b")
                .with_score("0.01")
                .with_source_ref("source-hash-a"),
        );

    let selected = select_canary_proof_candidate(&input).expect("positive ev survivor selected");

    assert_eq!(selected.instrument_id, "instrument-b");
}

#[test]
fn proof_policy_rejects_source_not_ready_before_candidate_selection() {
    let input = CanaryProofPolicyInput::fixture().with_source_ready(false);

    let err = select_canary_proof_candidate(&input).expect_err("source readiness required");

    assert_eq!(err, CanaryProofPolicyRejection::ProofPolicySourceNotReady);
}

#[test]
fn proof_policy_rejects_rounded_quantity_below_adapter_minimum() {
    let input = CanaryProofPolicyInput::fixture()
        .with_proof_notional("0.01")
        .with_base_quantity_constraints(|constraints| {
            constraints.min_quantity = dec("1.00");
        });

    let err = select_canary_proof_candidate(&input).expect_err("below minimum quantity rejected");

    assert_eq!(err, CanaryProofPolicyRejection::InstrumentConstraintsBelowMinQuantity);
}

#[test]
fn proof_policy_rejects_rounded_notional_below_adapter_minimum() {
    let input = CanaryProofPolicyInput::fixture()
        .with_proof_notional("0.50")
        .with_base_quantity_constraints(|constraints| {
            constraints.min_notional = Some(dec("1.00"));
        });

    let err = select_canary_proof_candidate(&input).expect_err("below minimum notional rejected");

    assert_eq!(err, CanaryProofPolicyRejection::InstrumentConstraintsBelowMinNotional);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --test bolt_v3_canary_proof_policy -- --nocapture`

Expected: FAIL because module does not exist.

- [ ] **Step 3: Implement types and selection**

Create `src/bolt_v3_canary_proof_policy.rs` with:

```rust
use std::collections::BTreeSet;
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryProofPolicyRejection {
    ProofPolicyDisabled,
    ProofPolicyInvalidClaim,
    ProofPolicySourceNotReady,
    ProofPolicyStrategyMismatch,
    ProofPolicyExecutionClientMismatch,
    ProofCandidateMissing,
    ProofCandidateSourceMismatch,
    ProofPolicyNegativeEvDisallowed,
    ProofPolicyRotationNotObserved,
    ProofCandidateTieWithoutPriority,
    InstrumentConstraintsMissing,
    InstrumentConstraintsRejectOrderType,
    InstrumentConstraintsRejectTimeInForce,
    InstrumentConstraintsRejectPositionSide,
    InstrumentConstraintsRejectPriceTick,
    InstrumentConstraintsRejectSizingMode,
    InstrumentConstraintsRoundToZero,
    InstrumentConstraintsBelowMinQuantity,
    InstrumentConstraintsBelowMinNotional,
    ProofNotionalExceedsCap,
    ProofNotionalExceedsStrategyRiskCap,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanaryProofOrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanaryProofPositionSide {
    Long,
    Short,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanaryProofOrderType {
    Market,
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanaryProofTimeInForce {
    Gtc,
    Ioc,
    Fok,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryProofCandidateScoreKind {
    ExpectedValue,
    StrategyRank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryProofPositionSideRequirement {
    Unsupported,
    Optional,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryProofSizingMode {
    BaseQuantityRequired,
    QuoteNotionalAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryProofOrderSizing {
    BaseQuantity {
        rounded_quantity: Decimal,
        rounded_notional: Decimal,
    },
    QuoteNotional {
        quote_notional: Decimal,
        estimated_rounded_quantity: Option<Decimal>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanaryProofSourceEvidenceRef {
    pub artifact_kind: String,
    pub artifact_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofCandidate {
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub market_id: String,
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub position_side: Option<CanaryProofPositionSide>,
    pub order_type: CanaryProofOrderType,
    pub time_in_force: CanaryProofTimeInForce,
    pub post_only: bool,
    pub reduce_only: bool,
    pub limit_price: Option<Decimal>,
    pub sizing_price: Decimal,
    pub notional_hint: Decimal,
    pub candidate_score: Decimal,
    pub score_kind: CanaryProofCandidateScoreKind,
    pub candidate_priority: Option<u32>,
    pub source_evidence_refs: Vec<CanaryProofSourceEvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofInstrumentConstraints {
    pub instrument_id: String,
    pub supported_order_types: BTreeSet<CanaryProofOrderType>,
    pub supported_time_in_force: BTreeSet<CanaryProofTimeInForce>,
    pub price_tick: Decimal,
    pub quantity_step: Decimal,
    pub min_quantity: Decimal,
    pub min_notional: Option<Decimal>,
    pub position_side_requirement: CanaryProofPositionSideRequirement,
    pub sizing_mode: CanaryProofSizingMode,
    pub supports_post_only: bool,
    pub supports_reduce_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofPolicyInput {
    pub enabled: bool,
    pub proof_claim: String,
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub source_ready: bool,
    pub allow_negative_expected_ev: bool,
    pub proof_notional: Decimal,
    pub max_notional_per_order: Decimal,
    pub strategy_risk_max_notional: Option<Decimal>,
    pub require_rotation_observation: bool,
    pub observed_distinct_market_count: u32,
    pub required_distinct_market_count: u32,
    pub observed_rotation_attempt_count: u32,
    pub rotation_max_attempts: u32,
    pub current_source_evidence_refs: BTreeSet<CanaryProofSourceEvidenceRef>,
    pub candidates: Vec<CanaryProofCandidate>,
    pub constraints: Vec<CanaryProofInstrumentConstraints>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryProofOrderIntent {
    pub proof_claim: String,
    pub strategy_instance_id: String,
    pub execution_client_id: String,
    pub market_id: String,
    pub instrument_id: String,
    pub order_side: CanaryProofOrderSide,
    pub position_side: Option<CanaryProofPositionSide>,
    pub order_type: CanaryProofOrderType,
    pub time_in_force: CanaryProofTimeInForce,
    pub post_only: bool,
    pub reduce_only: bool,
    pub limit_price: Option<Decimal>,
    pub notional: Decimal,
    pub sizing: CanaryProofOrderSizing,
    pub source_evidence_refs: Vec<CanaryProofSourceEvidenceRef>,
}

pub fn select_canary_proof_candidate(
    input: &CanaryProofPolicyInput,
) -> Result<CanaryProofOrderIntent, CanaryProofPolicyRejection> {
    if !input.enabled {
        return Err(CanaryProofPolicyRejection::ProofPolicyDisabled);
    }
    if input.proof_claim != "proof_only" {
        return Err(CanaryProofPolicyRejection::ProofPolicyInvalidClaim);
    }
    if !input.source_ready {
        return Err(CanaryProofPolicyRejection::ProofPolicySourceNotReady);
    }
    if input.proof_notional > input.max_notional_per_order {
        return Err(CanaryProofPolicyRejection::ProofNotionalExceedsCap);
    }
    if let Some(strategy_cap) = input.strategy_risk_max_notional {
        if input.proof_notional > strategy_cap {
            return Err(CanaryProofPolicyRejection::ProofNotionalExceedsStrategyRiskCap);
        }
    }
    if input.require_rotation_observation
        && input.observed_distinct_market_count < input.required_distinct_market_count
    {
        return Err(CanaryProofPolicyRejection::ProofPolicyRotationNotObserved);
    }
    if input.require_rotation_observation
        && input.observed_rotation_attempt_count > input.rotation_max_attempts
    {
        return Err(CanaryProofPolicyRejection::ProofPolicyRotationNotObserved);
    }
    let mut candidates = input
        .candidates
        .iter()
        .filter(|candidate| candidate.strategy_instance_id == input.strategy_instance_id)
        .filter(|candidate| candidate.execution_client_id == input.execution_client_id)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(CanaryProofPolicyRejection::ProofCandidateMissing);
    }
    candidates.retain(|candidate| {
        !candidate.source_evidence_refs.is_empty()
            && candidate.source_evidence_refs.iter().all(|reference| {
                input.current_source_evidence_refs.contains(reference)
            })
    });
    if candidates.is_empty() {
        return Err(CanaryProofPolicyRejection::ProofCandidateSourceMismatch);
    }
    if !input.allow_negative_expected_ev {
        candidates.retain(|candidate| {
            candidate.score_kind != CanaryProofCandidateScoreKind::ExpectedValue
                || candidate.candidate_score >= Decimal::ZERO
        });
        if candidates.is_empty() {
            return Err(CanaryProofPolicyRejection::ProofPolicyNegativeEvDisallowed);
        }
    }
    candidates.sort_by(|left, right| {
        right
            .candidate_score
            .cmp(&left.candidate_score)
            .then_with(|| {
                priority_sort_key(left.candidate_priority)
                    .cmp(&priority_sort_key(right.candidate_priority))
            })
    });
    let selected = candidates[0];
    if candidates.len() > 1
        && selected.candidate_score == candidates[1].candidate_score
        && selected.candidate_priority == candidates[1].candidate_priority
    {
        return Err(CanaryProofPolicyRejection::ProofCandidateTieWithoutPriority);
    }
    let constraints = input
        .constraints
        .iter()
        .find(|constraints| constraints.instrument_id == selected.instrument_id)
        .ok_or(CanaryProofPolicyRejection::InstrumentConstraintsMissing)?;
    if !constraints.supported_order_types.contains(&selected.order_type) {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRejectOrderType);
    }
    if !constraints.supported_time_in_force.contains(&selected.time_in_force) {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRejectTimeInForce);
    }
    if selected.post_only && !constraints.supports_post_only {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRejectOrderType);
    }
    if selected.reduce_only && !constraints.supports_reduce_only {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRejectOrderType);
    }
    if selected.order_type == CanaryProofOrderType::Limit && selected.limit_price.is_none() {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRejectPriceTick);
    }
    validate_position_side(selected.position_side.as_ref(), &constraints.position_side_requirement)?;
    validate_limit_price_tick(selected.limit_price, constraints.price_tick)?;
    if constraints.quantity_step <= Decimal::ZERO
        || constraints.min_quantity <= Decimal::ZERO
        || selected.sizing_price <= Decimal::ZERO
    {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRoundToZero);
    }
    let sizing = normalize_order_sizing(input.proof_notional, selected.sizing_price, constraints)?;
    Ok(CanaryProofOrderIntent {
        proof_claim: input.proof_claim.clone(),
        strategy_instance_id: selected.strategy_instance_id.clone(),
        execution_client_id: selected.execution_client_id.clone(),
        market_id: selected.market_id.clone(),
        instrument_id: selected.instrument_id.clone(),
        order_side: selected.order_side.clone(),
        position_side: selected.position_side.clone(),
        order_type: selected.order_type.clone(),
        time_in_force: selected.time_in_force.clone(),
        post_only: selected.post_only,
        reduce_only: selected.reduce_only,
        limit_price: selected.limit_price,
        notional: sizing.notional_for_submit_admission(),
        sizing,
        source_evidence_refs: selected.source_evidence_refs.clone(),
    })
}
```

Implement the helpers in the same module with focused tests:

- `round_down_to_step(quantity, step)` uses decimal arithmetic only and rejects non-positive steps.
- `validate_limit_price_tick(limit_price, price_tick)` rejects a limit price that is not an exact multiple of the adapter-provided tick.
- `validate_position_side(position_side, requirement)` rejects a missing side when required and rejects any side when unsupported.
- `priority_sort_key(priority)` maps `Some(value)` to `value` and `None` to the maximum key so explicit candidate priority always beats a missing priority at equal score.
- `normalize_order_sizing(proof_notional, sizing_price, constraints)` branches only on adapter-provided `sizing_mode`; base-quantity venues receive rounded base quantity, quote-notional venues receive quote notional plus estimated quantity evidence.
- `CanaryProofOrderSizing::notional_for_submit_admission()` returns rounded notional for base-quantity sizing and quote notional for quote-notional sizing, so the existing submit cap sees the correct bounded notional in both modes.
- source refs are compared by artifact kind and hash against the hash-bound current source packet; no source path is read without its expected hash.

`normalize_order_sizing` must enforce the adapter minimums after rounding:

```rust
fn enforce_instrument_minimums(
    rounded_quantity: Decimal,
    notional: Decimal,
    constraints: &CanaryProofInstrumentConstraints,
) -> Result<(), CanaryProofPolicyRejection> {
    if rounded_quantity <= Decimal::ZERO {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsRoundToZero);
    }
    if rounded_quantity < constraints.min_quantity {
        return Err(CanaryProofPolicyRejection::InstrumentConstraintsBelowMinQuantity);
    }
    if let Some(min_notional) = constraints.min_notional {
        if notional < min_notional {
            return Err(CanaryProofPolicyRejection::InstrumentConstraintsBelowMinNotional);
        }
    }
    Ok(())
}
```

Both `BaseQuantityRequired` and `QuoteNotionalAccepted` sizing paths must call this helper when adapter metadata exposes `min_quantity`; quote-notional mode records `estimated_rounded_quantity` for the minimum-quantity check and fails closed if a required estimate cannot be computed.

The implementation must use the repo's existing normalized order, price, and side types if equivalent types already exist. The local enum names above are the fallback shape for the new module, not a second production vocabulary.

Export it from `src/lib.rs`:

```rust
pub mod bolt_v3_canary_proof_policy;
```

- [ ] **Step 4: Verify and commit**

Run: `cargo test --test bolt_v3_canary_proof_policy -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/bolt_v3_canary_proof_policy.rs src/lib.rs tests/bolt_v3_canary_proof_policy.rs
git commit -m "feat(canary): add generic proof policy"
```

## Task 3: Strategy Candidate Provider Boundary And Source Artifact Producer

**Files:**
- Modify: `src/strategies/mod.rs`
- Modify: `src/bolt_v3_operator_artifacts.rs`
- Create: `tests/support/canary_proof_strategy.rs`
- Test: `tests/bolt_v3_canary_proof_policy.rs`

- [ ] **Step 1: Write failing provider test**

Add:

```rust
#[test]
fn configured_strategy_provider_exposes_candidates_even_when_alpha_selects_none() {
    let strategy = TestCanaryProofStrategy::new()
        .with_strategy_instance_id(STRATEGY_FIXTURE_ID)
        .with_execution_client_id(EXECUTION_CLIENT_FIXTURE_ID)
        .with_current_source_ref(SOURCE_FIXTURE_HASH)
        .with_negative_but_ranked_candidates();
    let source_packet = hash_bound_strategy_source_packet_fixture(SOURCE_FIXTURE_HASH);
    let candidates = strategy
        .canary_proof_candidates(&source_packet)
        .expect("candidate provider supported");

    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|candidate| candidate.strategy_instance_id == STRATEGY_FIXTURE_ID));
    assert!(candidates.iter().all(|candidate| candidate.execution_client_id == EXECUTION_CLIENT_FIXTURE_ID));
    assert!(candidates.iter().all(|candidate| !candidate.source_evidence_refs.is_empty()));
}

#[test]
fn strategy_registry_materializes_hash_bound_candidate_source_for_configured_provider() {
    let root = tempdir().expect("tempdir");
    let config = write_config_with_enabled_proof_policy(root.path());
    let source_packet = write_current_hash_bound_source_packet(root.path());

    let artifact = generate_canary_proof_candidate_source(&config, &source_packet)
        .expect("configured strategy provider should materialize candidate source");

    assert_eq!(artifact.record_kind, "bolt_v3_canary_proof_candidate_source");
    assert_eq!(artifact.proof_claim, "proof_only");
    assert!(artifact.candidate_count > 0);
    assert!(artifact.candidates.iter().all(|candidate| !candidate.source_evidence_refs.is_empty()));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --test bolt_v3_canary_proof_policy configured_strategy_provider_exposes_candidates -- --nocapture`

Expected: FAIL because provider method does not exist.

- [ ] **Step 3: Add generic provider trait**

In `src/strategies/mod.rs`:

```rust
use crate::bolt_v3_canary_proof_policy::CanaryProofCandidate;

pub trait CanaryProofCandidateProvider {
    fn canary_proof_candidates(
        &self,
        source_packet: &HashBoundStrategySourcePacket,
    ) -> anyhow::Result<Vec<CanaryProofCandidate>>;
}
```

- [ ] **Step 4: Implement a test-only provider**

Create `tests/support/canary_proof_strategy.rs` so the core contract is proven without any production strategy dependency:

```rust
use bolt_v2::bolt_v3_canary_proof_policy::{
    CanaryProofCandidate, CanaryProofCandidateScoreKind, CanaryProofOrderSide,
    CanaryProofOrderType, CanaryProofSourceEvidenceRef, CanaryProofTimeInForce,
};
use bolt_v2::strategies::CanaryProofCandidateProvider;
use rust_decimal::Decimal;

pub struct TestCanaryProofStrategy {
    strategy_instance_id: String,
    execution_client_id: String,
    source_ref: CanaryProofSourceEvidenceRef,
    candidates: Vec<CanaryProofCandidate>,
}

impl TestCanaryProofStrategy {
    pub fn new() -> Self {
        Self {
            strategy_instance_id: String::new(),
            execution_client_id: String::new(),
            source_ref: CanaryProofSourceEvidenceRef::fixture(),
            candidates: Vec::new(),
        }
    }

    pub fn with_strategy_instance_id(mut self, value: &str) -> Self {
        self.strategy_instance_id = value.to_string();
        self
    }

    pub fn with_execution_client_id(mut self, value: &str) -> Self {
        self.execution_client_id = value.to_string();
        self
    }

    pub fn with_current_source_ref(mut self, value: &str) -> Self {
        self.source_ref = CanaryProofSourceEvidenceRef::strategy_input_fixture(value);
        self
    }

    pub fn with_negative_but_ranked_candidates(mut self) -> Self {
        self.candidates = vec![
            CanaryProofCandidate::fixture()
                .with_strategy_instance_id(&self.strategy_instance_id)
                .with_execution_client_id(&self.execution_client_id)
                .with_instrument_id(INSTRUMENT_FIXTURE_ID_A)
                .with_order_side(CanaryProofOrderSide::Buy)
                .with_order_type(CanaryProofOrderType::Limit)
                .with_time_in_force(CanaryProofTimeInForce::Gtc)
                .with_score_kind(CanaryProofCandidateScoreKind::ExpectedValue)
                .with_score("-12.5")
                .with_source_ref(self.source_ref.clone()),
            CanaryProofCandidate::fixture()
                .with_strategy_instance_id(&self.strategy_instance_id)
                .with_execution_client_id(&self.execution_client_id)
                .with_instrument_id(INSTRUMENT_FIXTURE_ID_B)
                .with_order_side(CanaryProofOrderSide::Sell)
                .with_order_type(CanaryProofOrderType::Limit)
                .with_time_in_force(CanaryProofTimeInForce::Gtc)
                .with_score_kind(CanaryProofCandidateScoreKind::ExpectedValue)
                .with_score("-7.5")
                .with_source_ref(self.source_ref.clone()),
        ];
        self
    }
}

impl CanaryProofCandidateProvider for TestCanaryProofStrategy {
    fn canary_proof_candidates(
        &self,
        source_packet: &HashBoundStrategySourcePacket,
    ) -> anyhow::Result<Vec<CanaryProofCandidate>> {
        ensure_source_packet_matches_ref(source_packet, &self.source_ref)?;
        Ok(self.candidates.clone())
    }
}
```

Production strategies can then opt in by implementing the same trait in their own modules. That strategy-specific opt-in is intentionally outside the generic proof-policy core.

The selected configured strategy used for the production-readiness packet must be reachable through this provider registry before proof mode can run. Do not hardcode that strategy in the proof-policy module. The registry should resolve the configured strategy instance id, ask that strategy for candidates from its existing source/evaluation state, and fail closed with `ProofCandidateMissing` or `ProofCandidateSourceMismatch` when no source-bound candidates are available.

Add a materializer used by the CLI command in Task 4:

```rust
pub fn generate_canary_proof_candidate_source(
    config: &BoltV3Config,
    source_packet: &HashBoundStrategySourcePacket,
) -> Result<CanaryProofCandidateSourceArtifact, CanaryProofPolicyRejection> {
    let provider = resolve_canary_proof_candidate_provider(config.live_canary.proof_policy.strategy_instance_id)?;
    let candidates = provider.canary_proof_candidates(source_packet)?;
    CanaryProofCandidateSourceArtifact::from_source_bound_candidates(config, source_packet, candidates)
}
```

This materializer is the only production path that writes `canary_proof_candidate_source`. Hand-authored candidate files are rejected by the final-packet verifier because their hash/provenance cannot match the source packet.

- [ ] **Step 5: Verify and commit**

Run: `cargo test --test bolt_v3_canary_proof_policy configured_strategy_provider_exposes_candidates -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/strategies/mod.rs src/bolt_v3_operator_artifacts.rs tests/support/canary_proof_strategy.rs tests/bolt_v3_canary_proof_policy.rs
git commit -m "feat(canary): expose strategy proof candidates"
```

## Task 4: Operator Artifacts And Final Packet Bindings

**Files:**
- Modify: `src/bolt_v3_operator_artifacts.rs`
- Modify: `src/bolt_v3_tiny_canary_evidence.rs`
- Modify: `src/main.rs`
- Test: `tests/bolt_v3_canary_proof_operator_artifacts.rs`

- [ ] **Step 1: Write failing artifact test**

Create `tests/bolt_v3_canary_proof_operator_artifacts.rs`:

```rust
#[test]
fn final_packet_requires_hash_bound_canary_proof_artifacts_when_policy_enabled() {
    let root = tempdir().expect("tempdir");
    let config = write_config_with_enabled_proof_policy(root.path());
    let packet = assemble_packet_without_proof_policy_hashes(root.path(), &config);

    let err = verify_final_packet_pre_run(&config, &packet).expect_err("missing proof bindings rejected");

    assert!(err.to_string().contains("canary_proof_policy"));
}

#[test]
fn config_validation_rejects_enabled_proof_policy_without_operator_evidence_bindings() {
    let config = write_config_with_enabled_proof_policy_and_no_proof_artifact_bindings();

    let err = validate_full_config(&config).expect_err("enabled proof policy requires evidence bindings");

    assert!(err.to_string().contains("canary_proof_policy"));
    assert!(err.to_string().contains("sha256"));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --test bolt_v3_canary_proof_operator_artifacts -- --nocapture`

Expected: FAIL because proof artifact bindings do not exist.

- [ ] **Step 3: Add artifact structs**

In `src/bolt_v3_operator_artifacts.rs`, add serialized structs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryProofPolicyArtifact {
    pub schema_version: u32,
    pub record_kind: String,
    pub proof_claim: String,
    pub strategy_instance_id_hash: String,
    pub execution_client_id_hash: String,
    pub policy_kind: String,
    pub proof_notional: String,
    pub max_notional_per_order: String,
    pub strategy_risk_max_notional: Option<String>,
    pub allow_negative_expected_ev: bool,
    pub rotation_min_distinct_markets: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryProofCandidateSourceArtifact {
    pub schema_version: u32,
    pub record_kind: String,
    pub proof_claim: String,
    pub strategy_instance_id_hash: String,
    pub execution_client_id_hash: String,
    pub source_evidence_refs: Vec<HashBoundSourceEvidenceRef>,
    pub candidate_count: u32,
    pub candidates: Vec<CanaryProofCandidateArtifact>,
    pub generation_rejection_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryProofCandidateArtifact {
    pub market_id_hash: String,
    pub instrument_id_hash: String,
    pub order_side: String,
    pub position_side: Option<String>,
    pub order_type: String,
    pub time_in_force: String,
    pub post_only: bool,
    pub reduce_only: bool,
    pub limit_price: Option<String>,
    pub sizing_price: String,
    pub notional_hint: String,
    pub candidate_score: String,
    pub score_kind: String,
    pub candidate_priority: Option<u32>,
    pub source_evidence_refs: Vec<HashBoundSourceEvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryProofOrderIntentArtifact {
    pub schema_version: u32,
    pub record_kind: String,
    pub proof_claim: String,
    pub strategy_instance_id_hash: String,
    pub execution_client_id_hash: String,
    pub market_id_hash: String,
    pub instrument_id_hash: String,
    pub order_side: String,
    pub position_side: Option<String>,
    pub order_type: String,
    pub time_in_force: String,
    pub post_only: bool,
    pub reduce_only: bool,
    pub limit_price: Option<String>,
    pub notional: String,
    pub sizing_mode: String,
    pub rounded_quantity: Option<String>,
    pub quote_notional: Option<String>,
    pub estimated_rounded_quantity: Option<String>,
    pub source_evidence_refs: Vec<HashBoundSourceEvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashBoundSourceEvidenceRef {
    pub artifact_kind: String,
    pub artifact_sha256: String,
}
```

- [ ] **Step 4: Add CLI commands**

In `src/main.rs`, add subcommands:

```rust
GenerateCanaryProofPolicy {
    #[arg(short, long)]
    config: PathBuf,
    #[arg(long)]
    strategy_instance_id: String,
    #[arg(long)]
    output: PathBuf,
},
GenerateCanaryProofCandidateSource {
    #[arg(short, long)]
    config: PathBuf,
    #[arg(long)]
    strategy_instance_id: String,
    #[arg(long)]
    strategy_source_packet: PathBuf,
    #[arg(long)]
    strategy_source_packet_sha256: String,
    #[arg(long)]
    max_strategy_source_packet_bytes: u64,
    #[arg(long)]
    output: PathBuf,
},
GenerateCanaryProofOrderIntent {
    #[arg(short, long)]
    config: PathBuf,
    #[arg(long)]
    strategy_instance_id: String,
    #[arg(long)]
    candidate_source: PathBuf,
    #[arg(long)]
    candidate_source_sha256: String,
    #[arg(long)]
    rotation_observation: PathBuf,
    #[arg(long)]
    rotation_observation_sha256: String,
    #[arg(long)]
    max_candidate_source_bytes: u64,
    #[arg(long)]
    output: PathBuf,
},
GenerateCanaryRotationObservation {
    #[arg(short, long)]
    config: PathBuf,
    #[arg(long)]
    market_selection_source: PathBuf,
    #[arg(long)]
    market_selection_source_sha256: String,
    #[arg(long)]
    output: PathBuf,
},
```

`GenerateCanaryProofOrderIntent` must load only hash-bound candidate and rotation artifacts, validate their source refs against the current source packet, enforce rotation count when configured, run the generic proof policy, and write an artifact whose source refs and rounded quantity match the selected intent.

`GenerateCanaryProofCandidateSource` must be the only command that writes `CanaryProofCandidateSourceArtifact`. It must load the hash-bound strategy source packet, resolve the configured strategy provider from the strategy registry, materialize normalized candidates from that provider, and fail closed when the provider is absent or returns candidates without matching source refs.

- [ ] **Step 5: Bind artifacts in operator evidence**

Add these optional fields to the operator evidence config and require them when `live_canary.proof_policy.enabled` is true:

```rust
pub canary_proof_policy_path: Option<String>,
pub canary_proof_policy_sha256: Option<String>,
pub canary_proof_candidate_source_path: Option<String>,
pub canary_proof_candidate_source_sha256: Option<String>,
pub canary_rotation_observation_path: Option<String>,
pub canary_rotation_observation_sha256: Option<String>,
pub canary_proof_order_intent_path: Option<String>,
pub canary_proof_order_intent_sha256: Option<String>,
```

The pre-run final-packet verifier must reject missing proof bindings, hash mismatch, stale source evidence, candidate source mismatch, strategy or execution-client mismatch, selected market mismatch, non-`proof_only` order intent, proof notional above the approved live-canary cap, proof notional above the selected strategy risk cap, and insufficient rotation observation count.

- [ ] **Step 6: Verify and commit**

Run: `cargo test --test bolt_v3_canary_proof_operator_artifacts -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/bolt_v3_operator_artifacts.rs src/bolt_v3_tiny_canary_evidence.rs src/main.rs tests/bolt_v3_canary_proof_operator_artifacts.rs
git commit -m "feat(canary): bind proof artifacts into final packet"
```

## Task 5: Submit Admission And Live Node Integration

**Files:**
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Test: `tests/bolt_v3_canary_proof_live_gate.rs`

- [ ] **Step 1: Write failing submit-admission test**

Create `tests/bolt_v3_canary_proof_live_gate.rs`:

```rust
#[test]
fn proof_order_intent_consumes_same_live_canary_order_cap() {
    let mut gate = live_canary_gate_with_max_order_count(1);
    let intent = proof_only_order_intent_with_notional("1.00");

    let first = gate.admit_order_intent(&intent).expect("first proof order admitted");
    let second = gate.admit_order_intent(&intent).expect_err("second proof order rejected");

    assert_eq!(first.admitted_order_count, 1);
    assert!(second.to_string().contains("max_live_order_count"));
}

#[test]
fn proof_order_intent_rejects_notional_above_canary_cap() {
    let mut gate = live_canary_gate_with_max_notional("5.00");
    let intent = proof_only_order_intent_with_notional("5.01");

    let err = gate.admit_order_intent(&intent).expect_err("above cap rejected");

    assert!(err.to_string().contains("max_notional_per_order"));
}

#[test]
fn live_node_proof_branch_records_normal_submit_evidence_after_admission() {
    let harness = live_node_harness_with_hash_bound_proof_intent();

    let result = harness.run_one_order_canary().expect("proof canary run succeeds");

    assert!(result.submit_admission_recorded);
    assert!(result.submit_event_recorded);
    assert!(result.venue_order_state_recorded);
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --test bolt_v3_canary_proof_live_gate -- --nocapture`

Expected: FAIL because proof intents are not connected to submit admission.

- [ ] **Step 3: Route proof intent through normal admission**

In `src/bolt_v3_submit_admission.rs`, add `canary_proof_claim: Option<String>` to the existing order-intent admission input. Reject any value other than `proof_only` and apply the same count/notional checks already used for normal canary order admission.

- [ ] **Step 4: Live node integration**

In `src/bolt_v3_live_node.rs`, after final canary gate passes and before submit, add a single branch:

```rust
if proof_policy_enabled(&loaded.root) {
    let source_readiness = load_hash_bound_source_readiness(&loaded)?;
    ensure_source_ready_for_proof(&source_readiness)?;
    let proof_intent = load_verified_canary_proof_order_intent(&loaded, &source_readiness)?;
    submit_normalized_order_intent_through_existing_canary_path(live_node, proof_intent)?;
} else {
    submit_normal_strategy_order_intent_through_existing_canary_path(live_node)?;
}
```

The helpers must load only hash-bound operator-evidence paths. `source_ready = true` may be passed into the policy only after the source packet hash, freshness window, selected market hash, and strategy-input source refs match the final packet. The live path must fail closed before order-intent loading when source readiness is false or stale. It must not inspect venue names or strategy archetype names. Both branches must continue through the existing submit-admission, submit-event, venue-order-state, and post-run evidence path; proof mode changes how the order intent is chosen, not how a live order is admitted or recorded.

- [ ] **Step 5: Verify and commit**

Run: `cargo test --test bolt_v3_canary_proof_live_gate -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/bolt_v3_submit_admission.rs src/bolt_v3_live_node.rs tests/bolt_v3_canary_proof_live_gate.rs
git commit -m "feat(canary): route proof intents through submit admission"
```

## Task 6: Proof-Only Accounting Isolation

**Files:**
- Modify: `src/bolt_v3_tiny_canary_evidence.rs`
- Modify: `src/bolt_v3_position_contract.rs` only if the existing position contract needs a proof-only classification field.
- Test: `tests/bolt_v3_tiny_canary_operator.rs`
- Test: `tests/bolt_v3_tiny_canary_preconditions.rs` only if the existing precondition tests own position-attribution behavior.

- [ ] **Step 1: Write failing accounting-isolation tests**

Add tests proving proof-only fills and positions cannot be counted as alpha performance:

```rust
#[test]
fn proof_only_fill_is_recorded_as_operational_canary_not_alpha_trade() {
    let fill = proof_only_canary_fill_fixture();

    let record = classify_canary_fill_for_accounting(&fill).expect("fill classified");

    assert_eq!(record.proof_claim.as_deref(), Some("proof_only"));
    assert_eq!(record.accounting_bucket, AccountingBucket::OperationalCanary);
    assert!(!record.counts_toward_alpha_trade_count);
    assert!(!record.counts_toward_strategy_performance);
}

#[test]
fn proof_only_position_is_excluded_from_alpha_position_attribution() {
    let position = proof_only_canary_position_fixture();

    let attribution = classify_position_for_strategy_attribution(&position).expect("position classified");

    assert_eq!(attribution.proof_claim.as_deref(), Some("proof_only"));
    assert!(!attribution.counts_toward_alpha_pnl);
    assert!(!attribution.counts_toward_normal_position_attribution);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --test bolt_v3_tiny_canary_operator proof_only -- --nocapture`

Expected: FAIL because proof-only accounting classification is not yet explicit.

- [ ] **Step 3: Implement proof-only accounting classification**

Carry `proof_claim = "proof_only"` from proof order intent into canary submit/fill/position evidence. Add a small classification helper that maps proof-only records to an operational canary bucket and marks alpha PnL, alpha trade count, strategy performance, and normal position attribution as false.

Do not create a second portfolio or execution ledger. NT remains the account/position/fill source of truth. This task only prevents proof-only operational evidence from being summarized as alpha performance.

- [ ] **Step 4: Verify and commit**

Run: `cargo test --test bolt_v3_tiny_canary_operator proof_only -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/bolt_v3_tiny_canary_evidence.rs src/bolt_v3_position_contract.rs tests/bolt_v3_tiny_canary_operator.rs tests/bolt_v3_tiny_canary_preconditions.rs
git commit -m "feat(canary): isolate proof-only accounting"
```

## Task 7: Rotation Observation Evidence

**Files:**
- Modify: `src/bolt_v3_operator_artifacts.rs`
- Test: `tests/bolt_v3_canary_proof_operator_artifacts.rs`

- [ ] **Step 1: Write failing rotation evidence test**

Add:

```rust
#[test]
fn rotation_observation_counts_distinct_selected_markets_from_source() {
    let source = rotation_source_with_markets(["market-a", "market-b"]);
    let artifact = generate_rotation_observation(&source, 2).expect("rotation proven");

    assert_eq!(artifact.distinct_selected_market_count, 2);
    assert_eq!(artifact.proof_claim, "proof_only");
}

#[test]
fn proof_order_intent_generation_rejects_insufficient_rotation_observation() {
    let source = rotation_source_with_markets(["market-a"]);
    let rotation = generate_rotation_observation(&source, 2).expect_err("rotation count rejected");

    assert!(rotation.to_string().contains("rotation_min_distinct_markets"));
}
```

- [ ] **Step 2: Implement rotation artifact**

Add `CanaryRotationObservationArtifact`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryRotationObservationArtifact {
    pub schema_version: u32,
    pub record_kind: String,
    pub proof_claim: String,
    pub strategy_instance_id_hash: String,
    pub execution_client_id_hash: String,
    pub distinct_selected_market_count: u32,
    pub observed_attempt_count: u32,
    pub rotation_max_attempts: u32,
    pub selected_market_id_hashes: Vec<String>,
}
```

Generate it from market-selection source artifacts, not from runtime string guesses.

`GenerateCanaryRotationObservation` must be the only command that writes this artifact. `GenerateCanaryProofOrderIntent` must require the artifact and its expected hash whenever `rotation_observation_enabled = true`, and must reject when `distinct_selected_market_count < rotation_min_distinct_markets` or `observed_attempt_count > rotation_max_attempts`.

- [ ] **Step 3: Verify and commit**

Run: `cargo test --test bolt_v3_canary_proof_operator_artifacts rotation_observation -- --nocapture`

Expected: PASS.

Commit:

```bash
git add src/bolt_v3_operator_artifacts.rs tests/bolt_v3_canary_proof_operator_artifacts.rs
git commit -m "feat(canary): prove market rotation observations"
```

## Task 8: Docs, Runbook, And Final Verification

**Files:**
- Modify: `specs/024-production-trade-readiness/tiny-canary.md`
- Modify: `specs/024-production-trade-readiness/closeout-runbook.md`
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` only if a verifier identifies new classified literals.

- [ ] **Step 1: Update operator runbook**

Document the proof-only run sequence:

```md
1. Enable `[live_canary.proof_policy]` only in the approved root TOML packet.
2. Generate source evidence and strategy proof candidates.
3. Generate canary proof policy, rotation observation, and proof order intent artifacts.
4. Run `operator-artifacts verify-final --verification-stage pre-run`.
5. Run no-submit readiness.
6. Run `bolt-v2 run --config <approved-config>` only inside the approval window.
7. Record canary evidence, submit event, venue order state, restart reconciliation, and post-run hygiene.
8. Label the result `proof_only`; do not claim alpha readiness.
9. Verify any proof-only fill or residual position is excluded from alpha PnL, strategy performance, and normal trade-count summaries while remaining visible in operational canary evidence.
```

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test --test config_parsing live_canary_proof_policy -- --nocapture
cargo test --test bolt_v3_canary_proof_policy -- --nocapture
cargo test --test bolt_v3_canary_proof_operator_artifacts -- --nocapture
cargo test --test bolt_v3_canary_proof_live_gate -- --nocapture
cargo test --test bolt_v3_tiny_canary_operator proof_only -- --nocapture
```

Expected: all PASS.

- [ ] **Step 3: Run repo verifiers**

Run:

```bash
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_provider_leak.py
python3 scripts/verify_bolt_v3_core_boundary.py
```

Expected: all exit 0. If runtime-literals fails, classify only true production literals with a narrow reason.

- [ ] **Step 4: Commit docs**

```bash
git add specs/024-production-trade-readiness/tiny-canary.md specs/024-production-trade-readiness/closeout-runbook.md docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml
git commit -m "docs(canary): document proof-only live canary path"
```

## Self-Review Checklist

- Every implementation task keeps proof policy generic over venue and strategy.
- Strategy support is opt-in through the generic provider trait, not proof-policy core logic.
- No task adds concrete venue names, asset symbols, outcome labels, or cadence values as proof-policy defaults.
- Proof-only order intents still pass through submit admission.
- Proof artifacts are hash-bound into final packet verification.
- Candidate-source artifacts have a schema and exactly one source-owned producer.
- Adapter minimum quantity and minimum notional are enforced after sizing normalization.
- The runbook states that proof-only canary evidence is not alpha/profit evidence.
- Proof-only fills and positions are accounted as operational canary evidence, not alpha PnL or strategy performance.
