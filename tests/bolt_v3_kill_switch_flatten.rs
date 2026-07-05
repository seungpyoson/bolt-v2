use bolt_v2::{
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_kill_switch_flatten::{
        BoltV3KillSwitchFlattenAggregateOutcome, BoltV3KillSwitchFlattenAttemptOutcome,
        BoltV3KillSwitchFlattenCandidate, BoltV3KillSwitchFlattenDecisionMode,
        BoltV3KillSwitchFlattenError, BoltV3KillSwitchFlattenOutcomeAggregation,
        BoltV3KillSwitchFlattenOutcomeAggregator, BoltV3KillSwitchFlattenOutcomeEvidence,
        BoltV3KillSwitchFlattenPlanRequest, BoltV3KillSwitchFlattenPolicy,
        BoltV3KillSwitchFlattenPositionEvidenceKind, BoltV3KillSwitchFlattenPositionState,
        BoltV3KillSwitchFlattenQuantitySource, BoltV3KillSwitchFlattenResult,
        BoltV3KillSwitchFlattenRetryContext, BoltV3KillSwitchFlattenRetryDecision,
        BoltV3KillSwitchFlattenRetryPolicy, BoltV3KillSwitchFlattenRetrySupervisor,
        BoltV3KillSwitchFlattenRouteKind, BoltV3KillSwitchFlattenRouteProof,
        BoltV3KillSwitchFlattenSnapshot, BoltV3KillSwitchFlattenSupervisor,
    },
    bolt_v3_order_intent::NtOrderTemplate,
    bolt_v3_submit_admission::BoltV3KillSwitchForcedReductionClaim,
};
use nautilus_model::{
    enums::{OrderSide, OrderStatus, OrderType, PositionSide, TimeInForce, TradingState},
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
fn flatten_snapshot_rejects_conflicting_cache_and_report_position_proof() {
    let error = BoltV3KillSwitchFlattenSnapshot::new(vec![
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-conflict-1",
            PositionSide::Long,
            Quantity::from("1.00"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
            "position-conflict-1",
            PositionSide::Short,
            Quantity::from("1.00"),
        ),
    ])
    .expect_err("conflicting NT cache/report position proof must fail closed");

    assert_eq!(
        error,
        BoltV3KillSwitchFlattenError::ConflictingPositionProof
    );
}

#[test]
fn flatten_snapshot_dedups_agreeing_cache_and_report_position_proof_to_one_command() {
    let snapshot = BoltV3KillSwitchFlattenSnapshot::new(vec![
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            "position-long-1",
            PositionSide::Long,
            Quantity::from("1.00"),
        ),
        flatten_candidate(
            BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
            "position-long-1",
            PositionSide::Long,
            Quantity::from("1.00"),
        ),
    ])
    .expect("agreeing NT cache/report position proof should collapse to one candidate");

    assert_eq!(
        snapshot.candidates().len(),
        1,
        "agreeing cache/report evidence for one position must dedup to a single candidate"
    );
    assert_eq!(snapshot.open_positions().len(), 1);

    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request_with_snapshot(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
        snapshot,
    ))
    .expect("deduped open position should plan forced-reduction commands");

    assert_eq!(
        plan.commands().len(),
        1,
        "agreeing duplicate position evidence must not plan duplicate reduce-only commands"
    );
    assert_eq!(
        plan.commands()[0].position_id(),
        PositionId::from("position-long-1")
    );
}

#[test]
fn flatten_candidate_rejects_open_side_with_zero_quantity() {
    for open_side in [PositionSide::Long, PositionSide::Short] {
        let error = BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
            BoltV3KillSwitchFlattenPositionState {
                evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                account_id: account_id(),
                instrument_id: instrument_id(),
                strategy_id: strategy_id("binary-oracle-edge-taker-001"),
                position_id: PositionId::from("position-zero-qty-1"),
                position_side: open_side,
                quantity: Quantity::from("0"),
                source_timestamp_unix_nanos: SOURCE_TIMESTAMP_UNIX_NANOS,
            },
        )
        .expect_err("open-side zero-quantity position evidence must fail closed");

        assert_eq!(
            error,
            BoltV3KillSwitchFlattenError::InconsistentPositionProof
        );
    }
}

#[test]
fn flatten_candidate_rejects_missing_source_timestamp() {
    let error = BoltV3KillSwitchFlattenCandidate::from_nt_position_state(
        BoltV3KillSwitchFlattenPositionState {
            evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
            account_id: account_id(),
            instrument_id: instrument_id(),
            strategy_id: strategy_id("binary-oracle-edge-taker-001"),
            position_id: PositionId::from("position-missing-source-ts-1"),
            position_side: PositionSide::Long,
            quantity: Quantity::from("1.00"),
            source_timestamp_unix_nanos: 0,
        },
    )
    .expect_err("zero source timestamp must reject at the position proof boundary");

    assert_eq!(error, BoltV3KillSwitchFlattenError::MissingSourceTimestamp);
}

#[test]
fn flatten_plan_trims_action_id_into_command_proof_records() {
    let mut request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    request.action_id = format!("  {ACTION_ID}  ");

    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
        .expect("padded action ID matching the forced-reduction claim should plan");

    assert_eq!(
        plan.commands()
            .first()
            .expect("open position should plan one command")
            .action_id(),
        ACTION_ID,
        "flatten command must store the trimmed action ID so cancel and flatten proof records match"
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

    let future_snapshot = BoltV3KillSwitchFlattenSnapshot::new(vec![flatten_candidate_at(
        BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
        "position-future-1",
        PositionSide::Long,
        Quantity::from("1.00"),
        OBSERVED_AT_UNIX_NANOS + 1,
    )])
    .expect("future-dated position proof should preserve evidence for policy validation");
    assert_eq!(
        policy
            .validate_snapshot(&future_snapshot, OBSERVED_AT_UNIX_NANOS)
            .expect_err("future-dated position proof should fail closed"),
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
        BoltV3KillSwitchFlattenDecisionMode::LiveNodeCommandRouter
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
fn flatten_plan_is_idempotent_and_preserves_quantity_source_provenance() {
    let request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    let first = BoltV3KillSwitchFlattenSupervisor::plan_flatten(request.clone())
        .expect("first flatten planning pass should succeed");
    let second = BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
        .expect("repeat planning with same evidence should succeed");

    assert_eq!(first.commands(), second.commands());
    assert_eq!(
        first
            .commands()
            .first()
            .expect("open position should plan one command")
            .quantity_source(),
        BoltV3KillSwitchFlattenQuantitySource::CachePositionQuantity
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
        command.quantity_source(),
        BoltV3KillSwitchFlattenQuantitySource::CachePositionQuantity
    );
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
fn flatten_plan_route_kinds_select_matching_decision_mode() {
    for (route_kind, expected_mode) in [
        (
            BoltV3KillSwitchFlattenRouteKind::LiveNodeCommandRouter,
            BoltV3KillSwitchFlattenDecisionMode::LiveNodeCommandRouter,
        ),
        (
            BoltV3KillSwitchFlattenRouteKind::PerStrategyActionPort,
            BoltV3KillSwitchFlattenDecisionMode::DryRunProofOnly,
        ),
    ] {
        let mut request = flatten_plan_request(
            KillSwitchState::Flattening {
                halt_id: HALT_ID.to_string(),
            },
            TradingState::Reducing,
        );
        request.route_proof = BoltV3KillSwitchFlattenRouteProof::new(route_kind);

        let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(request)
            .expect("supported route kinds should produce planned commands");

        assert_eq!(plan.decision_mode(), expected_mode);
        assert_eq!(
            plan.commands()
                .first()
                .expect("open position should plan one command")
                .route_kind(),
            route_kind
        );
    }
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

#[test]
fn flatten_outcome_aggregation_maps_nt_submit_and_position_evidence_to_flatten_results() {
    assert_eq!(
        aggregate_single_command(BoltV3KillSwitchFlattenAttemptOutcome::submit_planned()).result(),
        BoltV3KillSwitchFlattenResult::OutstandingFlattenSubmit
    );
    assert_eq!(
        aggregate_single_command(
            BoltV3KillSwitchFlattenAttemptOutcome::submit_accepted(OrderStatus::Accepted)
                .expect("accepted submit outcome should preserve accepted NT status"),
        )
        .result(),
        BoltV3KillSwitchFlattenResult::OutstandingFlattenSubmit
    );
    assert_eq!(
        aggregate_single_command(
            BoltV3KillSwitchFlattenAttemptOutcome::submit_rejected(OrderStatus::Rejected)
                .expect("rejected submit outcome should preserve rejected NT status"),
        )
        .result(),
        BoltV3KillSwitchFlattenResult::SubmitRejectedManualIntervention
    );
    assert_eq!(
        aggregate_single_command(
            BoltV3KillSwitchFlattenAttemptOutcome::partial_fill(OrderStatus::PartiallyFilled)
                .expect("partial-fill outcome should require NT partially-filled status"),
        )
        .result(),
        BoltV3KillSwitchFlattenResult::ResidualPositionRemains
    );
    assert_eq!(
        aggregate_single_command(
            BoltV3KillSwitchFlattenAttemptOutcome::residual_position_remains(
                PositionSide::Long,
                SOURCE_TIMESTAMP_UNIX_NANOS,
            )
            .expect("open residual position proof should construct"),
        )
        .result(),
        BoltV3KillSwitchFlattenResult::ResidualPositionRemains
    );
    assert_eq!(
        aggregate_single_command(
            BoltV3KillSwitchFlattenAttemptOutcome::flat_position_observed(
                PositionSide::Flat,
                SOURCE_TIMESTAMP_UNIX_NANOS,
            )
            .expect("flat position proof should construct"),
        )
        .result(),
        BoltV3KillSwitchFlattenResult::AllFlat
    );
    assert_eq!(
        aggregate_single_command(BoltV3KillSwitchFlattenAttemptOutcome::stale_position_proof())
            .result(),
        BoltV3KillSwitchFlattenResult::FailedManualIntervention
    );
    assert_eq!(
        aggregate_single_command(BoltV3KillSwitchFlattenAttemptOutcome::unsupported_instrument())
            .result(),
        BoltV3KillSwitchFlattenResult::FailedManualIntervention
    );
    assert_eq!(
        aggregate_single_command(BoltV3KillSwitchFlattenAttemptOutcome::thin_book_no_fillability())
            .result(),
        BoltV3KillSwitchFlattenResult::FailedManualIntervention
    );
}

#[test]
fn flatten_outcome_aggregation_keeps_missing_or_duplicate_evidence_fail_closed() {
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    ))
    .expect("valid flatten request should plan commands");

    assert_eq!(
        BoltV3KillSwitchFlattenOutcomeAggregation::from_plan_outcomes(&plan, Vec::new())
            .expect("missing outcome evidence should aggregate as outstanding submit")
            .result(),
        BoltV3KillSwitchFlattenResult::OutstandingFlattenSubmit
    );

    let accepted = BoltV3KillSwitchFlattenOutcomeEvidence::from_command(
        &plan.commands()[0],
        BoltV3KillSwitchFlattenAttemptOutcome::submit_accepted(OrderStatus::Accepted)
            .expect("accepted submit outcome should construct"),
    );
    let rejected = BoltV3KillSwitchFlattenOutcomeEvidence::from_command(
        &plan.commands()[0],
        BoltV3KillSwitchFlattenAttemptOutcome::submit_rejected(OrderStatus::Rejected)
            .expect("rejected submit outcome should construct"),
    );

    assert_eq!(
        BoltV3KillSwitchFlattenOutcomeAggregation::from_plan_outcomes(
            &plan,
            vec![accepted, rejected],
        )
        .expect("duplicate outcome evidence should preserve worst observed state")
        .result(),
        BoltV3KillSwitchFlattenResult::SubmitRejectedManualIntervention
    );
}

#[test]
fn flatten_outcome_aggregation_requires_evidence_for_every_planned_command() {
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request_with_snapshot(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
        BoltV3KillSwitchFlattenSnapshot::new(vec![
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
        ])
        .expect("two open positions should preserve proof"),
    ))
    .expect("valid flatten request should plan two commands");

    let first_flat = BoltV3KillSwitchFlattenOutcomeEvidence::from_command(
        &plan.commands()[0],
        BoltV3KillSwitchFlattenAttemptOutcome::flat_position_observed(
            PositionSide::Flat,
            SOURCE_TIMESTAMP_UNIX_NANOS,
        )
        .expect("flat position proof should construct"),
    );

    assert_eq!(
        BoltV3KillSwitchFlattenOutcomeAggregation::from_plan_outcomes(&plan, vec![first_flat])
            .expect("missing outcome evidence for another command should aggregate fail-closed")
            .result(),
        BoltV3KillSwitchFlattenResult::OutstandingFlattenSubmit
    );
}

#[test]
fn flatten_outcome_aggregation_rejects_out_of_plan_evidence() {
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    ))
    .expect("valid flatten request should plan commands");

    let mut other_request = flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    );
    other_request.action_id = "flatten-action-2".to_string();
    other_request.forced_reduction_claim =
        BoltV3KillSwitchForcedReductionClaim::new(HALT_ID, "flatten-action-2", POLICY_SHA256)
            .expect("test forced-reduction claim should be valid");
    let other_plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(other_request)
        .expect("valid alternate flatten request should plan commands");
    let out_of_plan = BoltV3KillSwitchFlattenOutcomeEvidence::from_command(
        &other_plan.commands()[0],
        BoltV3KillSwitchFlattenAttemptOutcome::flat_position_observed(
            PositionSide::Flat,
            SOURCE_TIMESTAMP_UNIX_NANOS,
        )
        .expect("flat position proof should construct"),
    );

    assert_eq!(
        BoltV3KillSwitchFlattenOutcomeAggregation::from_plan_outcomes(&plan, vec![out_of_plan])
            .expect_err("out-of-plan outcome evidence must fail closed"),
        BoltV3KillSwitchFlattenError::UnknownOutcomeCommand
    );
}

#[test]
fn flatten_outcome_aggregation_keys_evidence_by_full_scoped_position_identity() {
    let shared_position_id = "position-reused-by-scope-1";
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request_with_snapshot(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
        BoltV3KillSwitchFlattenSnapshot::new(vec![
            flatten_candidate_with_identity(
                BoltV3KillSwitchFlattenPositionEvidenceKind::CachePosition,
                "GENERIC-001",
                "BTC-2026-06-02-UP.GENERIC",
                "binary-oracle-edge-taker-001",
                shared_position_id,
                PositionSide::Long,
                Quantity::from("1.00"),
            ),
            flatten_candidate_with_identity(
                BoltV3KillSwitchFlattenPositionEvidenceKind::PositionStatusReport,
                "GENERIC-001",
                "BTC-2026-06-02-UP.GENERIC",
                "binary-oracle-edge-taker-002",
                shared_position_id,
                PositionSide::Long,
                Quantity::from("1.00"),
            ),
        ])
        .expect("same NT position id in different strategy scope should preserve both commands"),
    ))
    .expect("valid flatten request should plan two scoped commands");

    assert_eq!(plan.commands().len(), 2);
    assert_eq!(
        plan.commands()[0].position_id(),
        plan.commands()[1].position_id()
    );
    assert_ne!(
        plan.commands()[0].strategy_id(),
        plan.commands()[1].strategy_id()
    );

    let first_flat = BoltV3KillSwitchFlattenOutcomeEvidence::from_command(
        &plan.commands()[0],
        BoltV3KillSwitchFlattenAttemptOutcome::flat_position_observed(
            PositionSide::Flat,
            SOURCE_TIMESTAMP_UNIX_NANOS,
        )
        .expect("flat position proof should construct"),
    );

    assert_eq!(
        BoltV3KillSwitchFlattenOutcomeAggregation::from_plan_outcomes(&plan, vec![first_flat])
            .expect("flat proof for one scoped command must not satisfy another scoped command")
            .result(),
        BoltV3KillSwitchFlattenResult::OutstandingFlattenSubmit
    );
}

#[test]
fn flatten_outcome_summary_never_authorizes_durable_state_transition() {
    for (outcomes, expected) in [
        (
            vec![BoltV3KillSwitchFlattenAttemptOutcome::FlatPositionObserved],
            BoltV3KillSwitchFlattenAggregateOutcome::AllFlat,
        ),
        (
            vec![BoltV3KillSwitchFlattenAttemptOutcome::ResidualPositionRemains],
            BoltV3KillSwitchFlattenAggregateOutcome::ResidualPositionRemains,
        ),
        (
            vec![BoltV3KillSwitchFlattenAttemptOutcome::SubmitPlanned],
            BoltV3KillSwitchFlattenAggregateOutcome::OutstandingFlattenSubmit,
        ),
        (
            vec![BoltV3KillSwitchFlattenAttemptOutcome::SubmitRejected],
            BoltV3KillSwitchFlattenAggregateOutcome::SubmitRejectedManualIntervention,
        ),
        (
            vec![BoltV3KillSwitchFlattenAttemptOutcome::StalePositionProof],
            BoltV3KillSwitchFlattenAggregateOutcome::FailedManualIntervention,
        ),
    ] {
        let summary = BoltV3KillSwitchFlattenOutcomeAggregator::summarize(&outcomes);

        assert_eq!(summary.aggregate(), expected);
        assert!(
            !summary.authorizes_durable_state_transition(),
            "Phase 5 outcome proof must not claim final global flat reconciliation"
        );
    }
}

#[test]
fn flatten_retry_requires_reducing_context_budget_and_forced_reduction_cap() {
    let policy = BoltV3KillSwitchFlattenRetryPolicy::new(3, 1_000, 100)
        .expect("positive retry policy should be valid");

    assert_eq!(
        BoltV3KillSwitchFlattenRetrySupervisor::decide(
            policy,
            BoltV3KillSwitchFlattenRetryContext {
                attempts: 1,
                elapsed_ms: 250,
                nt_trading_state: TradingState::Reducing,
                live_forced_reduction_order_count: 0,
                max_live_forced_reduction_order_count: 1,
            },
        ),
        BoltV3KillSwitchFlattenRetryDecision::RetryAllowed { backoff_ms: 100 }
    );
    assert_eq!(
        BoltV3KillSwitchFlattenRetrySupervisor::decide(
            policy,
            BoltV3KillSwitchFlattenRetryContext {
                attempts: 1,
                elapsed_ms: 250,
                nt_trading_state: TradingState::Active,
                live_forced_reduction_order_count: 0,
                max_live_forced_reduction_order_count: 1,
            },
        ),
        BoltV3KillSwitchFlattenRetryDecision::RouteNoLongerReducingManualIntervention
    );
    assert_eq!(
        BoltV3KillSwitchFlattenRetrySupervisor::decide(
            policy,
            BoltV3KillSwitchFlattenRetryContext {
                attempts: 1,
                elapsed_ms: 250,
                nt_trading_state: TradingState::Reducing,
                live_forced_reduction_order_count: 1,
                max_live_forced_reduction_order_count: 1,
            },
        ),
        BoltV3KillSwitchFlattenRetryDecision::ForcedReductionCapUnavailable
    );
    assert_eq!(
        BoltV3KillSwitchFlattenRetrySupervisor::decide(
            policy,
            BoltV3KillSwitchFlattenRetryContext {
                attempts: 3,
                elapsed_ms: 250,
                nt_trading_state: TradingState::Reducing,
                live_forced_reduction_order_count: 0,
                max_live_forced_reduction_order_count: 1,
            },
        ),
        BoltV3KillSwitchFlattenRetryDecision::ExhaustedManualIntervention
    );
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
    flatten_candidate_with_identity_at(
        evidence_kind,
        "GENERIC-001",
        "BTC-2026-06-02-UP.GENERIC",
        "binary-oracle-edge-taker-001",
        position_id,
        position_side,
        quantity,
        source_timestamp_unix_nanos,
    )
}

fn flatten_candidate_with_identity(
    evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind,
    account_id_value: &str,
    instrument_id_value: &str,
    strategy_id_value: &str,
    position_id: &str,
    position_side: PositionSide,
    quantity: Quantity,
) -> BoltV3KillSwitchFlattenCandidate {
    flatten_candidate_with_identity_at(
        evidence_kind,
        account_id_value,
        instrument_id_value,
        strategy_id_value,
        position_id,
        position_side,
        quantity,
        SOURCE_TIMESTAMP_UNIX_NANOS,
    )
}

fn flatten_candidate_with_identity_at(
    evidence_kind: BoltV3KillSwitchFlattenPositionEvidenceKind,
    account_id_value: &str,
    instrument_id_value: &str,
    strategy_id_value: &str,
    position_id: &str,
    position_side: PositionSide,
    quantity: Quantity,
    source_timestamp_unix_nanos: u64,
) -> BoltV3KillSwitchFlattenCandidate {
    BoltV3KillSwitchFlattenCandidate::from_nt_position_state(BoltV3KillSwitchFlattenPositionState {
        evidence_kind,
        account_id: AccountId::new(account_id_value),
        instrument_id: InstrumentId::from_as_ref(instrument_id_value)
            .expect("test instrument ID should parse through NT"),
        strategy_id: strategy_id(strategy_id_value),
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

fn aggregate_single_command(
    outcome: BoltV3KillSwitchFlattenAttemptOutcome,
) -> BoltV3KillSwitchFlattenOutcomeAggregation {
    let plan = BoltV3KillSwitchFlattenSupervisor::plan_flatten(flatten_plan_request(
        KillSwitchState::Flattening {
            halt_id: HALT_ID.to_string(),
        },
        TradingState::Reducing,
    ))
    .expect("valid flatten request should plan commands");
    let evidence =
        BoltV3KillSwitchFlattenOutcomeEvidence::from_command(&plan.commands()[0], outcome);
    BoltV3KillSwitchFlattenOutcomeAggregation::from_plan_outcomes(&plan, vec![evidence])
        .expect("single command outcome should aggregate")
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
