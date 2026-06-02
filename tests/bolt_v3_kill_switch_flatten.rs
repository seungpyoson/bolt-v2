use bolt_v2::{
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_kill_switch_flatten::{
        BoltV3KillSwitchFlattenCandidate, BoltV3KillSwitchFlattenDecisionMode,
        BoltV3KillSwitchFlattenError, BoltV3KillSwitchFlattenPlanRequest,
        BoltV3KillSwitchFlattenPolicy, BoltV3KillSwitchFlattenPositionEvidenceKind,
        BoltV3KillSwitchFlattenPositionState, BoltV3KillSwitchFlattenRouteKind,
        BoltV3KillSwitchFlattenRouteProof, BoltV3KillSwitchFlattenSnapshot,
        BoltV3KillSwitchFlattenSupervisor,
    },
    bolt_v3_order_intent::NtOrderTemplate,
    bolt_v3_submit_admission::BoltV3KillSwitchForcedReductionClaim,
};
use nautilus_model::{
    enums::{OrderSide, OrderType, PositionSide, TimeInForce, TradingState},
    identifiers::{AccountId, InstrumentId, PositionId, StrategyId},
    types::Quantity,
};

const HALT_ID: &str = "halt-phase-5";
const ACTION_ID: &str = "flatten-action-1";
const POLICY_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONFIG_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SOURCE_TIMESTAMP_UNIX_NANOS: u64 = 1_717_200_000_000_000_001;
const OBSERVED_AT_UNIX_NANOS: u64 = 1_717_200_000_000_000_005;
const MAX_SOURCE_AGE_UNIX_NANOS: u64 = 10;

#[test]
fn flatten_snapshot_distinguishes_open_flat_and_unknown_nt_position_evidence() {
    let snapshot = BoltV3KillSwitchFlattenSnapshot::new(vec![
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-long-1",
            PositionSide::Long,
            Quantity::from("1.25"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
            "position-short-1",
            PositionSide::Short,
            Quantity::from("0.50"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
            "position-flat-1",
            PositionSide::Flat,
            Quantity::from("0"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-unknown-1",
            PositionSide::NoPositionSide,
            Quantity::from("0.75"),
        ),
    ])
    .expect("mixed NT position snapshot should preserve all evidence");

    assert!(snapshot.has_residual_position_risk());
    assert_eq!(snapshot.open_positions().len(), 2);
    assert_eq!(snapshot.flat_positions().len(), 1);
    assert_eq!(snapshot.unknown_side_positions().len(), 1);
    assert_eq!(
        snapshot.open_positions()[0].evidence_kind(),
        BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition
    );
    assert_eq!(
        snapshot.open_positions()[1].evidence_kind(),
        BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport
    );
}

#[test]
fn flatten_policy_rejects_empty_and_stale_position_proof() {
    let policy = BoltV3KillSwitchFlattenPolicy::with_source_freshness(MAX_SOURCE_AGE_UNIX_NANOS)
        .expect("positive freshness policy should be valid");

    let empty_snapshot =
        BoltV3KillSwitchFlattenSnapshot::new(vec![]).expect("empty snapshot preserves proof gap");
    assert_eq!(
        policy
            .validate_snapshot(&empty_snapshot, OBSERVED_AT_UNIX_NANOS)
            .expect_err("empty position proof should fail closed"),
        BoltV3KillSwitchFlattenError::MissingPositionProof
    );

    let stale_snapshot = BoltV3KillSwitchFlattenSnapshot::new(vec![flatten_candidate_at(
        BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
        "position-stale-1",
        PositionSide::Long,
        Quantity::from("1.00"),
        OBSERVED_AT_UNIX_NANOS - MAX_SOURCE_AGE_UNIX_NANOS - 1,
    )])
    .expect("stale snapshot should preserve evidence for policy validation");
    assert_eq!(
        policy
            .validate_snapshot(&stale_snapshot, OBSERVED_AT_UNIX_NANOS)
            .expect_err("stale position proof should fail closed"),
        BoltV3KillSwitchFlattenError::StaleSourceTimestamp
    );
}

#[test]
fn flatten_supervisor_requires_flattening_state_and_reducing_trading_state() {
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    ))
    .expect("flattening state with reducing NT trading state should plan");

    assert_eq!(plan.halt_id(), HALT_ID);
    assert_eq!(
        plan.decision_mode(),
        BoltV3KillSwitchFlattenDecisionMode::DryRunProofOnly
    );

    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
            KillSwitchState::Cancelling {
                halt_id: HALT_ID.to_string(),
            },
            TradingState::Reducing,
        ))
        .expect_err("non-flattening kill-switch state must reject"),
        BoltV3KillSwitchFlattenError::KillSwitchStateNotFlattening
    );
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
            KillSwitchState::Flattening {
                halt_id: HALT_ID.to_string(),
            },
            TradingState::Active,
        ))
        .expect_err("active NT trading state must reject"),
        BoltV3KillSwitchFlattenError::NtTradingStateNotReducing
    );
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
            KillSwitchState::Flattening {
                halt_id: HALT_ID.to_string(),
            },
            TradingState::Halted,
        ))
        .expect_err("halted NT trading state must reject"),
        BoltV3KillSwitchFlattenError::NtTradingStateNotReducing
    );
}

#[test]
fn flatten_plan_commands_bind_metadata_and_nt_position_identity() {
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    ))
    .expect("valid flatten request should plan forced-reduction commands");

    let command = plan
        .commands()
        .first()
        .expect("open position should produce a planned forced-reduction command");

    assert_eq!(command.halt_id(), HALT_ID);
    assert_eq!(command.action_id(), ACTION_ID);
    assert_eq!(command.config_sha256(), CONFIG_SHA256);
    assert_eq!(command.policy_sha256(), POLICY_SHA256);
    assert_eq!(
        command.source_timestamp_unix_nanos(),
        SOURCE_TIMESTAMP_UNIX_NANOS
    );
    assert_eq!(command.account_id(), account_id());
    assert_eq!(command.instrument_id(), instrument_id());
    assert_eq!(
        command.strategy_id(),
        strategy_id("binary-oracle-edge-taker-001")
    );
    assert_eq!(command.position_id(), PositionId::from("position-long-1"));
    assert_eq!(command.position_side(), PositionSide::Long);
    assert_eq!(command.quantity(), Quantity::from("1.00"));
    assert_eq!(
        command.route_kind(),
        BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter
    );
    assert_eq!(command.order_template(), &flatten_order_template());
    assert_eq!(
        command.forced_reduction_claim().policy_sha256(),
        POLICY_SHA256
    );
}

#[test]
fn flatten_plan_rejects_forced_reduction_claim_that_does_not_match_metadata() {
    for mismatched_claim in [
        BoltV3KillSwitchForcedReductionClaim::new("other-halt", ACTION_ID, POLICY_SHA256)
            .expect("mismatched halt claim should still be structurally valid"),
        BoltV3KillSwitchForcedReductionClaim::new(HALT_ID, "other-action", POLICY_SHA256)
            .expect("mismatched action claim should still be structurally valid"),
        BoltV3KillSwitchForcedReductionClaim::new(
            HALT_ID,
            ACTION_ID,
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .expect("mismatched policy claim should still be structurally valid"),
    ] {
        let mut request = flatten_plan_request(
            KillSwitchState::Flattening {
                halt_id: HALT_ID.to_string(),
            },
            TradingState::Reducing,
        );
        request.forced_reduction_claim = mismatched_claim;

        assert_eq!(
            BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
                .expect_err("mismatched forced-reduction claim must reject"),
            BoltV3KillSwitchFlattenError::ForcedReductionProofMismatch
        );
    }
}

#[test]
fn flatten_plan_rejects_unsupported_route_proof() {
    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.route_proof =
        BoltV3KillSwitchFlattenRouteProof::new(BoltV3KillSwitchFlattenRouteKind::Unsupported);

    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("unsupported flatten route proof must reject"),
        BoltV3KillSwitchFlattenError::UnsupportedRouteProof
    );
}

#[test]
fn flatten_plan_rejects_missing_or_invalid_command_metadata() {
    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.action_id = " ".to_string();
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("blank action ID must reject"),
        BoltV3KillSwitchFlattenError::MissingActionId
    );

    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.config_sha256 = "not-a-sha256".to_string();
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("invalid config hash must reject"),
        BoltV3KillSwitchFlattenError::InvalidConfigSha256
    );

    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.policy_sha256 = "not-a-sha256".to_string();
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("invalid policy hash must reject"),
        BoltV3KillSwitchFlattenError::InvalidPolicySha256
    );

    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.source_timestamp_unix_nanos = 0;
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("missing source timestamp must reject"),
        BoltV3KillSwitchFlattenError::MissingSourceTimestamp
    );
}

#[test]
fn flatten_plan_requires_reduce_only_base_quantity_order_template() {
    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.order_template.is_reduce_only = false;
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("flatten order template must be reduce-only"),
        BoltV3KillSwitchFlattenError::OrderTemplateNotReduceOnly
    );

    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.order_template.is_quote_quantity = true;
    assert_eq!(
        BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect_err("flatten order template must use base quantity"),
        BoltV3KillSwitchFlattenError::OrderTemplateUsesQuoteQuantity
    );
}

#[test]
fn flatten_plan_rejects_order_templates_invalid_under_shared_nt_validation() {
    for invalid_template in [
        {
            let mut template = flatten_order_template();
            template.is_post_only = true;
            template
        },
        {
            let mut template = flatten_order_template();
            template.time_in_force = TimeInForce::Gtd;
            template
        },
    ] {
        let mut request = flatten_plan_request(
            KillSwitchState::Flattening {
                halt_id: HALT_ID.to_string(),
            },
            TradingState::Reducing,
        );
        request.order_template = invalid_template;

        assert_eq!(
            BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
                .expect_err("shared NT order-template validation must reject"),
            BoltV3KillSwitchFlattenError::InvalidOrderTemplate
        );
    }
}

#[test]
fn flatten_plan_maps_open_position_sides_to_forced_reduction_order_sides() {
    let snapshot = BoltV3KillSwitchFlattenSnapshot::new(vec![
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-long-1",
            PositionSide::Long,
            Quantity::from("1.00"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
            "position-short-1",
            PositionSide::Short,
            Quantity::from("0.50"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
            "position-flat-1",
            PositionSide::Flat,
            Quantity::from("0"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-unknown-1",
            PositionSide::NoPositionSide,
            Quantity::from("0.25"),
        ),
    ])
    .expect("mixed snapshot should preserve position proof");

    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request_with_snapshot(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
        snapshot,
    ))
    .expect("valid flatten request should plan open-position reductions only");

    let order_sides = plan
        .commands()
        .iter()
        .map(|command| command.order_side())
        .collect::<Vec<_>>();
    assert_eq!(order_sides, vec![OrderSide::Sell, OrderSide::Buy]);
}

fn flatten_candidate(
    evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind,
    position_id: &str,
    position_side: PositionSide,
    quantity: Quantity,
) -> BoltV3KillSwitchFlattenCandidate {
    flatten_candidate_at(
        evidence_kind,
        position_id,
        position_side,
        quantity,
        SOURCE_TIMESTAMP_UNIX_NANOS,
    )
}

fn flatten_candidate_at(
    evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind,
    position_id: &str,
    position_side: PositionSide,
    quantity: Quantity,
    source_timestamp_unix_nanos: u64,
) -> BoltV3KillSwitchFlattenCandidate {
    BoltV3KillSwitchFlattenCandidate::from_nt_position_state(BoltV3KillSwitchFlattenPositionState {
        evidence_kind,
        account_id: account_id(),
        instrument_id: instrument_id(),
        strategy_id: strategy_id("binary-oracle-edge-taker-001"),
        position_id: PositionId::from(position_id),
        position_side,
        quantity,
        source_timestamp_unix_nanos,
    })
    .expect("NT-backed flatten candidate should be valid")
}

fn flatten_plan_request(
    kill_switch_state: KillSwitchState,
    nt_trading_state: TradingState,
) -> BoltV3KillSwitchFlattenPlanRequest {
    flatten_plan_request_with_snapshot(
        kill_switch_state,
        nt_trading_state,
        BoltV3KillSwitchFlattenSnapshot::new(vec![flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-long-1",
            PositionSide::Long,
            Quantity::from("1.00"),
        )])
        .expect("single open-position snapshot should be valid"),
    )
}

fn flatten_plan_request_with_snapshot(
    kill_switch_state: KillSwitchState,
    nt_trading_state: TradingState,
    snapshot: BoltV3KillSwitchFlattenSnapshot,
) -> BoltV3KillSwitchFlattenPlanRequest {
    BoltV3KillSwitchFlattenPlanRequest {
        kill_switch_state,
        nt_trading_state,
        action_id: ACTION_ID.to_string(),
        config_sha256: CONFIG_SHA256.to_string(),
        policy_sha256: POLICY_SHA256.to_string(),
        source_timestamp_unix_nanos: SOURCE_TIMESTAMP_UNIX_NANOS,
        policy: BoltV3KillSwitchFlattenPolicy::with_source_freshness(MAX_SOURCE_AGE_UNIX_NANOS)
            .expect("positive freshness policy should be valid"),
        snapshot,
        observed_at_unix_nanos: OBSERVED_AT_UNIX_NANOS,
        route_proof: BoltV3KillSwitchFlattenRouteProof::new(
            BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
        ),
        order_template: flatten_order_template(),
        forced_reduction_claim: BoltV3KillSwitchForcedReductionClaim::new(
            HALT_ID,
            ACTION_ID,
            POLICY_SHA256,
        )
        .expect("test forced-reduction claim should be valid"),
    }
}

fn flatten_order_template() -> NtOrderTemplate {
    NtOrderTemplate {
        order_type: OrderType::Market,
        time_in_force: TimeInForce::Ioc,
        expire_time: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        is_post_only: false,
        is_reduce_only: true,
        is_quote_quantity: false,
    }
}

fn account_id() -> AccountId {
    AccountId::new("GENERIC-001")
}

fn instrument_id() -> InstrumentId {
    InstrumentId::from_as_ref("BTC-2026-06-02-UP.GENERIC")
        .expect("test instrument ID should parse through NT")
}

fn strategy_id(value: &str) -> StrategyId {
    StrategyId::new(value)
}
