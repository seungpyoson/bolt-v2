use crate::support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_capital_admission::{PredictionMarketAdmissionSnapshot, ProductAdmissionSnapshot},
    bolt_v3_capital_admission_runtime_feed::POLYMARKET_VENUE_TRUTH_REST_SOURCE,
    bolt_v3_capital_admission_state::{
        NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
        PortfolioCapitalAdmissionSnapshot, ReservationLedgerSnapshot, VenueSpendabilitySnapshot,
    },
    bolt_v3_config::{
        ReferencePriceBlock, ReferencePriceDriftPolicy, ReferencePriceProvider,
        ReferencePriceSelectionPolicy, ReferencePriceSourceBlock, ReferencePriceStalePolicy,
        load_bolt_v3_config,
    },
    bolt_v3_iv::config::load_iv_config_from_toml,
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_operator_health::{
        BoltV3InputHealth, BoltV3OperatorHealthStatus, BoltV3RejectObserverHealth,
        BoltV3RuntimeFeedAnnouncementStatus, BoltV3VenueTruthHealth,
        node_scoped_runtime_source_announcements, runtime_source_announcements,
    },
    bolt_v3_order_reject_observer_feed::BoltV3OrderRejectObserverHealthSnapshot,
    bolt_v3_reference_price_health::{
        ReferenceCurrentPriceHealthReport, ReferenceCurrentPriceSourceUpdateObservation,
    },
    bolt_v3_strategy_registration::{BoltV3RegisteredStrategy, BoltV3StrategyRegistrationSummary},
    bolt_v3_submit_admission::VENUE_TRUTH_CAPTURE_FAILURE_RESERVATION_SOURCE,
};
use nautilus_model::identifiers::ClientId;
use rust_decimal::Decimal;

#[test]
fn input_health_marks_unobserved_reference_source_as_missing_input() {
    let report = ReferenceCurrentPriceHealthReport {
        targets: Vec::new(),
        clients: Vec::new(),
        source_update_observations: vec![
            ReferenceCurrentPriceSourceUpdateObservation {
                strategy_instance_id: "binary-oracle".to_string(),
                source_id: "chainlink_primary".to_string(),
                asset: "BTC".to_string(),
                provider: "chainlink".to_string(),
                provider_instrument: "BTC/USD".to_string(),
                status: "observed".to_string(),
                reason: "observed".to_string(),
                observed_ts_ms: Some(1_000),
                received_ts_ms: Some(1_005),
            },
            ReferenceCurrentPriceSourceUpdateObservation {
                strategy_instance_id: "binary-oracle".to_string(),
                source_id: "polyresearch_backup".to_string(),
                asset: "BTC".to_string(),
                provider: "polyresearch".to_string(),
                provider_instrument: "BTC".to_string(),
                status: "timed_out".to_string(),
                reason: "timed_out".to_string(),
                observed_ts_ms: None,
                received_ts_ms: None,
            },
        ],
    };

    let health = BoltV3InputHealth::from_reference_current_price_report(&report);

    assert_eq!(health.status, BoltV3OperatorHealthStatus::MissingInput);
    assert_eq!(health.configured_source_count, 2);
    assert_eq!(health.observed_source_count, 1);
    assert_eq!(health.missing_sources.len(), 1);
    assert_eq!(health.missing_sources[0].source_id, "polyresearch_backup");
    assert_eq!(health.missing_sources[0].reason, "timed_out");
}

#[test]
fn configured_but_cold_input_health_renders_unobserved() {
    let health = BoltV3InputHealth::unobserved(2);

    assert_eq!(health.status, BoltV3OperatorHealthStatus::Unobserved);
    assert_eq!(health.configured_source_count, 2);
    assert_eq!(health.observed_source_count, 0);
    assert!(health.missing_sources.is_empty());
}

#[test]
fn node_scoped_announcements_cover_venue_truth_rest_and_configured_iv_sources() {
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture v3 config should load");
    loaded
        .root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should configure capital pools")[0]
        .enforce_submit_admission = true;
    loaded.root.iv =
        Some(load_iv_config_from_toml(valid_iv_toml()).expect("fixture IV config should load"));

    let announcements = node_scoped_runtime_source_announcements(&loaded, true);

    let venue_truth = announcements
        .venue_truth_rest_capture
        .expect("venue-truth REST capture must be announced");
    assert_eq!(venue_truth.source_id, POLYMARKET_VENUE_TRUTH_REST_SOURCE);
    assert_eq!(
        venue_truth.status,
        BoltV3RuntimeFeedAnnouncementStatus::Active
    );
    assert!(venue_truth.enabled);
    assert!(venue_truth.runtime_available);
    assert_eq!(announcements.iv_runtime_sources.len(), 2);
    assert!(
        announcements
            .iv_runtime_sources
            .iter()
            .any(|source| source.source_id == "configured-greeks-source")
    );
    assert!(
        announcements
            .iv_runtime_sources
            .iter()
            .all(|source| source.status == BoltV3RuntimeFeedAnnouncementStatus::Active)
    );
}

#[test]
fn runtime_source_announcements_include_disabled_and_unsupported_reference_sources() {
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture v3 config should load");
    let strategy_index = 0;
    loaded.strategies[strategy_index]
        .config
        .reference_current_price = Some(ReferencePriceBlock {
        asset: "BTC".to_string(),
        source_order: vec![
            "disabled_chainlink".to_string(),
            "unsupported_reference".to_string(),
        ],
        min_valid_sources: 1,
        selection_policy: ReferencePriceSelectionPolicy::FirstValidPerInterval,
        max_source_age_ms: 1_500,
        max_source_drift_bps: 10,
        drift_policy: ReferencePriceDriftPolicy::Observe,
        stale_policy: ReferencePriceStalePolicy::Block,
        sources: BTreeMap::from([
            (
                "disabled_chainlink".to_string(),
                ReferencePriceSourceBlock {
                    provider: ReferencePriceProvider::new("chainlink_ws")
                        .expect("test provider key should be valid"),
                    enabled: false,
                    required: false,
                    client_id: ClientId::from("chainlink_reference"),
                    instrument_id: Some("BTC-USD.CHAINLINK".to_string()),
                    symbol: None,
                },
            ),
            (
                "unsupported_reference".to_string(),
                ReferencePriceSourceBlock {
                    provider: ReferencePriceProvider::from_serialized("unsupported_reference_ws"),
                    enabled: true,
                    required: false,
                    client_id: ClientId::from("chainlink_reference"),
                    instrument_id: Some("BTC-USD.UNSUPPORTED".to_string()),
                    symbol: None,
                },
            ),
        ]),
    });
    let strategy = &loaded.strategies[strategy_index];
    let summary = BoltV3StrategyRegistrationSummary {
        registered: vec![BoltV3RegisteredStrategy {
            strategy_instance_id: strategy.config.strategy_instance_id.clone(),
            strategy_archetype: strategy.config.strategy_archetype.clone(),
            registered_strategy_id: "configured-nt-strategy".to_string(),
        }],
    };

    let announcements = runtime_source_announcements(&loaded, &summary)
        .expect("reference source announcements should render");

    let reference_sources = &announcements[0].reference_current_price_sources;
    assert_eq!(reference_sources.len(), 2);
    let disabled = reference_sources
        .iter()
        .find(|source| source.source_id == "disabled_chainlink")
        .expect("disabled source should not be omitted");
    assert_eq!(
        disabled.status,
        BoltV3RuntimeFeedAnnouncementStatus::Disabled
    );
    assert!(!disabled.enabled);
    assert!(!disabled.runtime_available);
    let unsupported = reference_sources
        .iter()
        .find(|source| source.source_id == "unsupported_reference")
        .expect("unsupported source should not be omitted");
    assert_eq!(
        unsupported.status,
        BoltV3RuntimeFeedAnnouncementStatus::Unsupported
    );
    assert!(unsupported.enabled);
    assert!(!unsupported.runtime_available);
}

#[test]
fn reject_observer_health_distinguishes_not_configured_nominal_and_read_error() {
    assert_eq!(
        BoltV3RejectObserverHealth::not_configured().status,
        BoltV3OperatorHealthStatus::NotConfigured
    );

    let nominal =
        BoltV3RejectObserverHealth::from_snapshot(&BoltV3OrderRejectObserverHealthSnapshot {
            active_episode_count: 0,
            total_retry_count: 0,
            oldest_episode_first_ns: None,
            latest_client_order_id: None,
        });
    assert_eq!(nominal.status, BoltV3OperatorHealthStatus::Nominal);

    let degraded =
        BoltV3RejectObserverHealth::from_snapshot(&BoltV3OrderRejectObserverHealthSnapshot {
            active_episode_count: 1,
            total_retry_count: 3,
            oldest_episode_first_ns: Some(1_000),
            latest_client_order_id: Some("C-001".to_string()),
        });
    assert_eq!(degraded.status, BoltV3OperatorHealthStatus::Degraded);

    let read_error =
        BoltV3RejectObserverHealth::read_error("order reject observer feed lock poisoned");
    assert_eq!(read_error.status, BoltV3OperatorHealthStatus::Degraded);
    assert_eq!(
        read_error.read_error.as_deref(),
        Some("order reject observer feed lock poisoned")
    );
}

#[test]
fn venue_truth_health_distinguishes_not_configured_unobserved_nominal_and_suspended() {
    assert_eq!(
        BoltV3VenueTruthHealth::not_configured().status,
        BoltV3OperatorHealthStatus::NotConfigured
    );

    let unobserved = BoltV3VenueTruthHealth::from_configured_kill_switch_and_capital_state(
        &KillSwitchState::Armed,
        None,
    );
    assert_eq!(unobserved.status, BoltV3OperatorHealthStatus::Unobserved);
    assert_eq!(unobserved.kill_switch_state, "Armed");

    let nominal_state = capital_state_with_source("nt_capital_admission_state");
    let nominal = BoltV3VenueTruthHealth::from_configured_kill_switch_and_capital_state(
        &KillSwitchState::Armed,
        Some(&nominal_state),
    );
    assert_eq!(nominal.status, BoltV3OperatorHealthStatus::Nominal);
    assert!(!nominal.venue_truth_capture_suspended);

    let suspended_state = capital_state_with_source(VENUE_TRUTH_CAPTURE_FAILURE_RESERVATION_SOURCE);
    let suspended = BoltV3VenueTruthHealth::from_configured_kill_switch_and_capital_state(
        &KillSwitchState::Armed,
        Some(&suspended_state),
    );
    assert_eq!(suspended.status, BoltV3OperatorHealthStatus::Degraded);
    assert!(suspended.venue_truth_capture_suspended);
}

#[test]
fn venue_truth_health_renders_divergence_trigger_as_halted() {
    let state = KillSwitchState::Halted {
        halt_id: "halt-001".to_string(),
        trigger: KillSwitchHaltTrigger::venue_truth_divergence(
            "polymarket_venue_truth_rest",
            1_000,
            "venue truth divergence: collateral_balance",
        ),
    };

    let health = BoltV3VenueTruthHealth::from_kill_switch_and_capital_state(&state, None);

    assert_eq!(health.status, BoltV3OperatorHealthStatus::Halted);
    assert_eq!(health.kill_switch_state, "Halted");
    assert_eq!(
        health.divergence.as_ref().map(|divergence| {
            (
                divergence.source.as_str(),
                divergence.source_timestamp_unix_nanos,
            )
        }),
        Some(("polymarket_venue_truth_rest", 1_000))
    );
}

#[test]
fn venue_truth_health_renders_non_divergence_latch_as_halted() {
    let state = KillSwitchState::FailedManualIntervention {
        halt_id: "halt-002".to_string(),
        reason: "runtime failure latch".to_string(),
    };

    let health =
        BoltV3VenueTruthHealth::from_configured_kill_switch_and_capital_state(&state, None);

    assert_eq!(health.status, BoltV3OperatorHealthStatus::Halted);
    assert_eq!(health.kill_switch_state, "FailedManualIntervention");
    assert!(health.divergence.is_none());
}

fn capital_state_with_source(source: &str) -> NtDerivedCapitalAdmissionState {
    NtDerivedCapitalAdmissionState {
        source: source.to_string(),
        observed_at_ns: 1_000,
        portfolio: PortfolioCapitalAdmissionSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-A".to_string(),
            collateral_currency: "USD".to_string(),
            free_collateral: Decimal::new(100, 0),
            total_equity: Decimal::new(100, 0),
        },
        venue_spendability: VenueSpendabilitySnapshot {
            source: "operator_venue_spendability".to_string(),
            observed_at_ns: 1_000,
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-A".to_string(),
            collateral_currency: "USD".to_string(),
            spendable_collateral: Decimal::new(100, 0),
            collateral_allowance: Decimal::new(100, 0),
        },
        order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns: 1_000,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "nt_prediction_market_snapshot".to_string(),
                observed_at_ns: 1_000,
                yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                no_instrument_id: "instrument-no.VENUE-A".to_string(),
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::new(100, 0),
                conditional_token_allowance: Decimal::new(100, 0),
                collateral_coupled_group_id: "group-1".to_string(),
            },
        ),
        reservation_snapshot: ReservationLedgerSnapshot {
            source: "bolt_reservation_ledger".to_string(),
            observed_at_ns: 1_000,
            all_live_reservations_attributed: true,
        },
        loss_snapshot: None,
    }
}

fn valid_iv_toml() -> &'static str {
    r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "smile", "surface", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredDuplicateOptionGreeks"] } }
derived_inputs = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.projection_policies]]
policy_id = "configured-projection-policy"
projection_kind = "mean"
basis_selection = "preserve_input_basis"
source_eligibility = ["configured-greeks-source"]
strike_selection = "all_configured_strikes"
tenor_selection = "all_configured_tenors"
evidence_mapping = "preserve_evidence_kind"
minimum_points = 1
max_projection_input_skew_ns = 10
output_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv"] } }
fallback_policy_ref = "configured-fallback-policy"
interpolation_policy_ref = "configured-interpolation-policy"
quorum_policy_ref = "configured-quorum-policy"

[[profiles.interpolation_policies]]
policy_id = "configured-interpolation-policy"
method = "linear"
strike_axis = "strike"
tenor_axis = "expiry"
minimum_points = 2
eligible_sources = ["configured-greeks-source"]
extrapolation = "reject"

[[profiles.fallback_policies]]
policy_id = "configured-fallback-policy"
candidate_order = ["configured-primary-candidate", "configured-backup-candidate"]
eligible_sources = ["configured-greeks-source"]
maximum_timestamp_skew_ns = 10
required_provenance_fields = ["raw_event_id"]

[[profiles.quorum_policies]]
policy_id = "configured-quorum-policy"
minimum_sources = 2
eligible_sources = ["configured-greeks-source", "configured-chain-source"]
agreement_band = 0.05
tie_break = "mean"

[[profiles.helper_policies]]
helper_policy_id = "configured-helper-policy"
nt_helper_symbol = "imply_vol_and_greeks"
parameter_signature = "s,r,b,is_call,k,t,price"
allowed_outputs = ["iv_and_greeks"]
input_policy_ref = "configured-derived-input-policy"
failure_policy = "reject_invalid_helper_output"
minimum_valid_iv_output = 0.000001
max_input_timestamp_skew_ns = 10
max_operator_input_age_ns = 100

[profiles.helper_policies.convention_policy]
allowed_conventions = ["configured-convention"]

[profiles.helper_policies.output_bounds]
finite_required = true
positive_required = true
inclusive_min = 0.0
inclusive_max = 5.0
exclusive_min = 0.0
exclusive_max = 6.0
unit = "unitless"

[profiles.helper_policies.output_bounds.allowed_conventions]
allowed_conventions = ["configured-convention"]

[[profiles.derived_input_policies]]
input_policy_id = "configured-derived-input-policy"
helper_policy_ref = "configured-helper-policy"
required_fields = ["option_price", "underlying_price", "strike", "option_side", "time_to_expiry_years", "rate", "carry"]
freshness_ns = 100
max_input_skew_ns = 10
operator_value_refresh_policy = "reject_expired_operator_values"

[profiles.derived_input_policies.bounds]
option_price = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 10000.0, unit = "price", allowed_conventions = { allowed_conventions = [] } }
underlying_price = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 99.0, unit = "price", allowed_conventions = { allowed_conventions = [] } }
strike = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 10000.0, unit = "strike", allowed_conventions = { allowed_conventions = [] } }
time_to_expiry_years = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 100.0, unit = "time_to_expiry", allowed_conventions = { allowed_conventions = [] } }
rate = { finite_required = true, positive_required = false, inclusive_min = -1.0, inclusive_max = 1.0, unit = "rate", allowed_conventions = { allowed_conventions = [] } }
carry = { finite_required = true, positive_required = false, inclusive_min = -1.0, inclusive_max = 1.0, unit = "carry", allowed_conventions = { allowed_conventions = [] } }

[profiles.derived_input_policies.convention_policy]
allowed_conventions = ["configured-convention"]

[[profiles.derived_input_policies.field_sources]]
field = "option_price"
allowed_source_kinds = ["query_supplied"]

[[profiles.derived_input_policies.field_sources]]
field = "underlying_price"
allowed_source_kinds = ["query_supplied", "profile_source_ref"]

[[profiles.derived_input_policies.field_sources]]
field = "strike"
allowed_source_kinds = ["query_supplied", "instrument_metadata"]

[[profiles.derived_input_policies.field_sources]]
field = "option_side"
allowed_source_kinds = ["query_supplied", "instrument_metadata"]

[[profiles.derived_input_policies.field_sources]]
field = "time_to_expiry_years"
allowed_source_kinds = ["query_supplied", "instrument_metadata"]

[[profiles.derived_input_policies.field_sources]]
field = "rate"
allowed_source_kinds = ["operator_configured"]

[[profiles.derived_input_policies.field_sources]]
field = "carry"
allowed_source_kinds = ["operator_configured"]

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["iv_point", "source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"

[[profiles.sources]]
source_id = "configured-chain-source"
selector_fingerprint = "configured-chain-selector"
source_kind = "option_chain"
client_id = "configured-client"
subscription_generation = 8
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionChain"

[profiles.sources.selector]
selector_kind = "source_option_chain"
series_ids = ["configured-series"]
strike_range_policy = "atm_relative:1:1"

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#
}
