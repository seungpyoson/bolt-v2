use bolt_v2::{
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_kill_switch_action_router::{
        BoltV3KillSwitchActionClass, BoltV3KillSwitchActionDecisionMode,
        BoltV3KillSwitchActionRequest, BoltV3KillSwitchActionRouter,
        BoltV3KillSwitchActionRouterError, BoltV3KillSwitchActionScope,
    },
    bolt_v3_submit_admission::BoltV3KillSwitchForcedReductionClaim,
};
use nautilus_model::enums::TradingState;
use nautilus_model::identifiers::{AccountId, InstrumentId};

const POLICY_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONFIG_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn dry_run_router_emits_cancel_and_flatten_metadata_without_order_effects() {
    let cancel_decision = BoltV3KillSwitchActionRouter::dry_run_decision(cancel_request(
        "cancel-action-1",
        cancelling_state(),
    ))
    .expect("cancelling state should produce proof-only cancel decision");

    assert_eq!(
        cancel_decision.action_class(),
        BoltV3KillSwitchActionClass::CancelOutstandingRisk
    );
    assert_eq!(
        cancel_decision.decision_mode(),
        BoltV3KillSwitchActionDecisionMode::DryRunProofOnly
    );
    assert_eq!(cancel_decision.halt_id(), "halt-1");
    assert_eq!(cancel_decision.action_id(), "cancel-action-1");
    assert_eq!(cancel_decision.config_sha256(), CONFIG_SHA256);
    assert_eq!(cancel_decision.policy_sha256(), POLICY_SHA256);
    assert_eq!(
        cancel_decision.source_timestamp_unix_nanos(),
        1_717_200_000_000_000_000
    );
    assert_eq!(
        cancel_decision.scope().account_ids(),
        [account_id("POLYMARKET-001")]
    );
    assert_eq!(
        cancel_decision.scope().instrument_ids(),
        [instrument_id("BTC-USD.BINANCE")]
    );
    assert!(cancel_decision.forced_reduction_claim().is_none());
    assert!(cancel_decision.live_order_effects().is_empty());
    assert!(cancel_decision.venue_calls().is_empty());

    let flatten_decision = BoltV3KillSwitchActionRouter::dry_run_decision(flatten_request(
        "flatten-action-1",
        flattening_state(),
        Some(forced_reduction_claim("flatten-action-1")),
    ))
    .expect("flattening state with matching proof should produce proof-only flatten decision");

    assert_eq!(
        flatten_decision.action_class(),
        BoltV3KillSwitchActionClass::FlattenPositions
    );
    assert_eq!(
        flatten_decision.decision_mode(),
        BoltV3KillSwitchActionDecisionMode::DryRunProofOnly
    );
    assert_eq!(flatten_decision.halt_id(), "halt-1");
    assert_eq!(flatten_decision.action_id(), "flatten-action-1");
    assert_eq!(flatten_decision.config_sha256(), CONFIG_SHA256);
    assert_eq!(flatten_decision.policy_sha256(), POLICY_SHA256);
    assert_eq!(
        flatten_decision.scope().account_ids(),
        [account_id("POLYMARKET-001")]
    );
    assert_eq!(
        flatten_decision.scope().instrument_ids(),
        [instrument_id("BTC-USD.BINANCE")]
    );
    assert_eq!(
        flatten_decision
            .forced_reduction_claim()
            .map(|claim| claim.action_id()),
        Some("flatten-action-1")
    );
    assert!(flatten_decision.live_order_effects().is_empty());
    assert!(flatten_decision.venue_calls().is_empty());
}

#[test]
fn reducing_trading_state_alone_cannot_authorize_flatten_output_or_bypass_proof() {
    let halted_error = BoltV3KillSwitchActionRouter::dry_run_decision(flatten_request(
        "flatten-action-1",
        halted_state(),
        Some(forced_reduction_claim("flatten-action-1")),
    ))
    .expect_err("NT reducing state alone must not authorize flatten output");
    assert_eq!(
        halted_error,
        BoltV3KillSwitchActionRouterError::KillSwitchStateNotFlattening
    );

    let missing_proof_error = BoltV3KillSwitchActionRouter::dry_run_decision(flatten_request(
        "flatten-action-1",
        flattening_state(),
        None,
    ))
    .expect_err("flatten output must require forced-reduction proof metadata");
    assert_eq!(
        missing_proof_error,
        BoltV3KillSwitchActionRouterError::ForcedReductionProofRequired
    );

    let mismatched_proof_error = BoltV3KillSwitchActionRouter::dry_run_decision(flatten_request(
        "flatten-action-1",
        flattening_state(),
        Some(forced_reduction_claim("other-action")),
    ))
    .expect_err("flatten output must reject mismatched forced-reduction proof metadata");
    assert_eq!(
        mismatched_proof_error,
        BoltV3KillSwitchActionRouterError::ForcedReductionProofMismatch
    );
}

#[test]
fn router_rejects_invalid_metadata_before_dry_run_decision() {
    let mut missing_action_id = cancel_request(" ", cancelling_state());
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(missing_action_id.clone()),
        Err(BoltV3KillSwitchActionRouterError::MissingActionId)
    );

    let mut invalid_config_sha = cancel_request("cancel-action-1", cancelling_state());
    invalid_config_sha.config_sha256 = "not-sha256".to_string();
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(invalid_config_sha),
        Err(BoltV3KillSwitchActionRouterError::InvalidConfigSha256)
    );

    let mut invalid_policy_sha = cancel_request("cancel-action-1", cancelling_state());
    invalid_policy_sha.policy_sha256 = "not-sha256".to_string();
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(invalid_policy_sha),
        Err(BoltV3KillSwitchActionRouterError::InvalidPolicySha256)
    );

    let mut missing_timestamp = cancel_request("cancel-action-1", cancelling_state());
    missing_timestamp.source_timestamp_unix_nanos = 0;
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(missing_timestamp),
        Err(BoltV3KillSwitchActionRouterError::MissingSourceTimestamp)
    );

    assert_eq!(
        BoltV3KillSwitchActionScope::new(Vec::new(), vec!["BTC-USD.BINANCE".to_string()]),
        Err(BoltV3KillSwitchActionRouterError::InvalidScope)
    );
    assert_eq!(
        BoltV3KillSwitchActionScope::new(vec!["POLYMARKET-001".to_string()], Vec::new()),
        Err(BoltV3KillSwitchActionRouterError::InvalidScope)
    );

    missing_action_id.action_id = "cancel-action-1".to_string();
    assert!(
        BoltV3KillSwitchActionRouter::dry_run_decision(missing_action_id).is_ok(),
        "control request should remain valid after replacing the missing action id"
    );
}

#[test]
fn cancel_and_flatten_dry_run_actions_require_reducing_nt_state_and_matching_kill_state() {
    let mut non_reducing_cancel = cancel_request("cancel-action-1", cancelling_state());
    non_reducing_cancel.nt_trading_state = TradingState::Halted;
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(non_reducing_cancel),
        Err(BoltV3KillSwitchActionRouterError::NtTradingStateNotReducing)
    );

    let cancel_from_halted = cancel_request("cancel-action-1", halted_state());
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(cancel_from_halted),
        Err(BoltV3KillSwitchActionRouterError::KillSwitchStateNotCancelling)
    );

    let mut non_reducing_flatten = flatten_request(
        "flatten-action-1",
        flattening_state(),
        Some(forced_reduction_claim("flatten-action-1")),
    );
    non_reducing_flatten.nt_trading_state = TradingState::Halted;
    assert_eq!(
        BoltV3KillSwitchActionRouter::dry_run_decision(non_reducing_flatten),
        Err(BoltV3KillSwitchActionRouterError::NtTradingStateNotReducing)
    );
}

#[test]
fn phase3_router_rejects_live_or_venue_action_outputs() {
    for action_class in [
        BoltV3KillSwitchActionClass::EntrySubmit,
        BoltV3KillSwitchActionClass::ReplaceSubmit,
        BoltV3KillSwitchActionClass::LiveSubmit,
        BoltV3KillSwitchActionClass::LiveCancel,
        BoltV3KillSwitchActionClass::LiveFlatten,
        BoltV3KillSwitchActionClass::VenueSpecificCall,
    ] {
        let error = BoltV3KillSwitchActionRouter::dry_run_decision(BoltV3KillSwitchActionRequest {
            action_class,
            kill_switch_state: flattening_state(),
            nt_trading_state: TradingState::Reducing,
            action_id: "blocked-action-1".to_string(),
            config_sha256: CONFIG_SHA256.to_string(),
            policy_sha256: POLICY_SHA256.to_string(),
            source_timestamp_unix_nanos: 1_717_200_000_000_000_000,
            scope: scope(),
            forced_reduction_claim: Some(forced_reduction_claim("blocked-action-1")),
        })
        .expect_err("Phase 3 router must reject live or venue-specific action outputs");

        assert_eq!(
            error,
            BoltV3KillSwitchActionRouterError::Phase3ActionOutputDisallowed
        );
    }
}

#[test]
fn action_scope_rejects_malformed_nt_identifier_strings_fail_closed() {
    assert_eq!(
        BoltV3KillSwitchActionScope::new(
            vec!["POLYMARKET001".to_string()],
            vec!["BTC-USD.BINANCE".to_string()],
        ),
        Err(BoltV3KillSwitchActionRouterError::InvalidAccountId),
        "account scope without an issuer/account hyphen must fail closed at construction"
    );
    assert_eq!(
        BoltV3KillSwitchActionScope::new(
            vec!["POLYMARKET-001".to_string()],
            vec!["BTC-USD-BINANCE".to_string()],
        ),
        Err(BoltV3KillSwitchActionRouterError::InvalidInstrumentId),
        "instrument scope without a symbol/venue '.' separator must fail closed at construction"
    );
    assert_eq!(
        BoltV3KillSwitchActionScope::new(vec!["POLYMARKET-001".to_string()], vec![" ".to_string()],),
        Err(BoltV3KillSwitchActionRouterError::InvalidInstrumentId),
        "blank instrument scope must fail closed through NT parsing, not slip past as valid"
    );
}

fn cancel_request(
    action_id: &str,
    kill_switch_state: KillSwitchState,
) -> BoltV3KillSwitchActionRequest {
    BoltV3KillSwitchActionRequest {
        action_class: BoltV3KillSwitchActionClass::CancelOutstandingRisk,
        kill_switch_state,
        nt_trading_state: TradingState::Reducing,
        action_id: action_id.to_string(),
        config_sha256: CONFIG_SHA256.to_string(),
        policy_sha256: POLICY_SHA256.to_string(),
        source_timestamp_unix_nanos: 1_717_200_000_000_000_000,
        scope: scope(),
        forced_reduction_claim: None,
    }
}

fn flatten_request(
    action_id: &str,
    kill_switch_state: KillSwitchState,
    forced_reduction_claim: Option<BoltV3KillSwitchForcedReductionClaim>,
) -> BoltV3KillSwitchActionRequest {
    BoltV3KillSwitchActionRequest {
        action_class: BoltV3KillSwitchActionClass::FlattenPositions,
        kill_switch_state,
        nt_trading_state: TradingState::Reducing,
        action_id: action_id.to_string(),
        config_sha256: CONFIG_SHA256.to_string(),
        policy_sha256: POLICY_SHA256.to_string(),
        source_timestamp_unix_nanos: 1_717_200_000_000_000_000,
        scope: scope(),
        forced_reduction_claim,
    }
}

fn scope() -> BoltV3KillSwitchActionScope {
    BoltV3KillSwitchActionScope::new(
        vec!["POLYMARKET-001".to_string()],
        vec!["BTC-USD.BINANCE".to_string()],
    )
    .expect("valid scope should construct")
}

fn account_id(value: &str) -> AccountId {
    AccountId::new(value)
}

fn instrument_id(value: &str) -> InstrumentId {
    InstrumentId::from_as_ref(value).expect("test instrument ID should parse through NT")
}

fn cancelling_state() -> KillSwitchState {
    KillSwitchState::Cancelling {
        halt_id: "halt-1".to_string(),
    }
}

fn flattening_state() -> KillSwitchState {
    KillSwitchState::Flattening {
        halt_id: "halt-1".to_string(),
    }
}

fn halted_state() -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "halt-1".to_string(),
        trigger: bolt_v2::bolt_v3_kill_switch::KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            1_000,
            "daily loss cap breached",
        ),
    }
}

fn forced_reduction_claim(action_id: &str) -> BoltV3KillSwitchForcedReductionClaim {
    BoltV3KillSwitchForcedReductionClaim::new("halt-1", action_id, POLICY_SHA256)
        .expect("valid forced-reduction claim should construct")
}
