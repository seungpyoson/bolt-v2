#![cfg(test)]

use std::{cell::RefCell, rc::Rc};

use super::*;
pub(super) use crate::bolt_v3_current_evidence::{
    AdmissionRejectionReason, CurrentFact, EntrySkipReason, ExitBlockedReason, ExitTriggerSource,
    RealizedVolAggregation, RealizedVolPricingComponent, RvGateResult,
};
use crate::bolt_v3_position_authority_feed::{
    BoltV3PositionAuthorityCapability, BoltV3PositionAuthorityFeed,
};
use nautilus_common::{
    actor::DataActorNative,
    messages::data::DataCommand,
    msgbus::TypedIntoHandler,
    runner::{DataCommandSender, get_data_cmd_sender, replace_data_cmd_sender},
};
use nautilus_model::{
    enums::{OmsType, PositionSideSpecified},
    identifiers::{AccountId, ClientId},
    reports::PositionStatusReport,
};
use nautilus_trading::Strategy;

pub(super) const TEST_TRADE_PRICE_PRECISION: u8 = 2;
pub(super) const TEST_TRADE_SIZE_PRECISION: u8 = 0;
const TEST_IDENTIFIER_TOKEN_LIMIT: usize = 8;

static NEXT_LOG_CAPTURE_STRATEGY_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(super) fn unique_log_capture_strategy_id(prefix: &str) -> String {
    let id = NEXT_LOG_CAPTURE_STRATEGY_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("BINARYORACLEEDGETAKER-{prefix}-{id}")
}

pub(super) fn with_captured_strategy_logs<R>(
    strategy_id: &str,
    action: impl FnOnce() -> R,
) -> (R, Vec<(log::Level, String)>) {
    let (result, records) = crate::bolt_v3_test_log_capture::with_captured_logs(action);
    let matching = records
        .into_iter()
        .filter(|(_, message)| message.contains(strategy_id))
        .collect::<Vec<_>>();
    (result, matching)
}

pub(super) fn find_error<'a>(
    errors: &'a [ValidationError],
    field: &str,
    code: &'static str,
) -> &'a ValidationError {
    errors
        .iter()
        .find(|e| e.field == field && e.code == code)
        .unwrap_or_else(|| panic!("missing error {field} / {code}: {errors:?}"))
}

pub(super) fn probability(value: f64) -> Probability {
    Probability::new(value).expect("valid probability")
}

pub(super) fn valid_raw_config() -> Value {
    toml::toml! {
        strategy_id = "BINARYORACLEEDGETAKER-001"
        order_id_tag = "001"
        oms_type = "netting"
        client_id = "POLYMARKET"
        configured_target_id = "configured_updown_target"
        target_kind = "rotating_market"
        rotating_market_family = "updown"
        underlying_asset = "CONFIGURED_ASSET"
        cadence_seconds = 300
        cadence_slug_token = "5m"
        market_selection_rule = "active_or_next"
        retry_interval_seconds = 5
        blocked_after_seconds = 60
        signal_venue = "signal_data_client"
        signal_instrument_id = "SIGNAL.SOURCE"
        use_uuid_client_order_ids = true
        use_hyphens_in_client_order_ids = false
        external_order_claims = ["AUXILIARY.SOURCE"]
        manage_contingent_orders = true
        manage_gtd_expiry = true
        manage_stop = true
        market_exit_interval_ms = 250
        market_exit_max_attempts = 7
        log_events = false
        log_commands = false
        log_rejected_due_post_only_as_warning = false
        warmup_tick_count = 20
        reentry_cooldown_secs = 30
        order_notional_target = 1000.0
        maximum_position_notional = 1000.0
        book_impact_cap_bps = 15
        vwap_depth_limit_bps = 15
        slippage_buffer_bps = 15
        risk_lambda = 0.5
        sizing_ev_reference_bps = 500
        edge_threshold_basis_points = 0
        exit_hysteresis_bps = 5
        realized_volatility_surface_id = "<surface_id>"
        realized_volatility_max_source_age_ms = 500
        trade_flow_window_secs = 30
        trade_flow_max_samples = 100
        spike_guard_return_threshold = 0.05
        spike_guard_cooldown_secs = 5
        price_to_beat_source = "chainlink_data_streams.report_at_boundary"
        pricing_kurtosis = 0.0
        theta_decay_factor = 0.0
        forced_flat_stale_reference_ms = 1500
        forced_flat_thin_book_min_liquidity = 100.0
        lead_agreement_min_corr = 0.8
        lead_jitter_max_ms = 250

        [entry_order]
        side = "buy"
        position_side = "long"
        order_type = "market"
        time_in_force = "fok"
        is_post_only = false
        is_reduce_only = false
        is_quote_quantity = true

        [forced_exit_order]
        side = "sell"
        position_side = "long"
        order_type = "market"
        time_in_force = "ioc"
        is_post_only = false
        is_reduce_only = false
        is_quote_quantity = false

        [exit_order]
        side = "sell"
        position_side = "long"
        order_type = "market"
        time_in_force = "ioc"
        is_post_only = false
        is_reduce_only = false
        is_quote_quantity = false
    }
    .into()
}

pub(super) fn recording_decision_evidence()
-> Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder> {
    Arc::new(crate::bolt_v3_current_evidence::DecisionEvidenceRecorder::recording())
}

fn fixture_position_authority_capability_with_cache(
    cache: Rc<RefCell<Cache>>,
) -> BoltV3PositionAuthorityCapability {
    let execution_client_id = ClientId::from("POLYMARKET");
    let account_id = AccountId::from("TEST-ACCOUNT");
    let feed = BoltV3PositionAuthorityFeed::try_new_with_cache(
        [(account_id, execution_client_id, fixture_execution_venue())],
        cache,
    )
    .expect("fixture position authority attribution should build");
    BoltV3PositionAuthorityCapability::new(feed, execution_client_id, account_id, OmsType::Netting)
}

pub(super) fn fixture_position_authority_capability(
    strategy: &BinaryOracleEdgeTaker,
) -> BoltV3PositionAuthorityCapability {
    fixture_position_authority_capability_with_cache(strategy.cache_rc())
}

pub(super) fn failing_decision_evidence()
-> Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder> {
    let recorder = recording_decision_evidence();
    recorder.fail_machine_writes_for_test();
    recorder.fail_observation_writes();
    recorder
}

pub(super) fn sync_failing_decision_evidence()
-> Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder> {
    let recorder = recording_decision_evidence();
    recorder.fail_machine_sync_for_test();
    recorder
}

pub(super) fn failing_observation_evidence()
-> Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder> {
    let recorder = recording_decision_evidence();
    recorder.fail_observation_writes();
    recorder
}

pub(super) fn recording_evidence_failing_blocked_attempt(
    attempt: usize,
) -> Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder> {
    let recorder = recording_decision_evidence();
    recorder.fail_purpose_on_attempt_for_test(
        crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::BlockedStrategyInputObservation,
        attempt,
    );
    recorder
}

#[derive(Debug, Default)]
pub(super) struct RecordingSettlementRuntimeSink {
    loss_observations: Mutex<Vec<crate::bolt_v3_loss_protection::PositionRealizedPnlObservation>>,
}

impl RecordingSettlementRuntimeSink {
    pub(super) fn loss_observations(
        &self,
    ) -> Vec<crate::bolt_v3_loss_protection::PositionRealizedPnlObservation> {
        self.loss_observations
            .lock()
            .expect("recording settlement sink loss mutex poisoned")
            .clone()
    }
}

impl crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSink
    for RecordingSettlementRuntimeSink
{
    fn record_loss_governor_position_realized_pnl(
        &self,
        observation: crate::bolt_v3_loss_protection::PositionRealizedPnlObservation,
    ) -> Result<()> {
        self.loss_observations
            .lock()
            .expect("recording settlement sink loss mutex poisoned")
            .push(observation);
        Ok(())
    }
}

pub(super) fn attach_settlement_runtime_sink(
    strategy: &mut BinaryOracleEdgeTaker,
    sink: std::rc::Rc<RecordingSettlementRuntimeSink>,
) {
    let sink: crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSinkHandle = sink;
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_runtime_sink(Some(sink));
}

/// Execution venue of the binary-option market fixtures these tests trade against (their
/// outcome instruments are `...POLYMARKET`). Production resolves the execution venue from
/// config — `root.clients[execution_client_id].venue` — and is venue-agnostic (a HIP-4 or any
/// other execution client works with no code change); these unit tests build the
/// `StrategyBuildContext` directly without a root TOML, so they pin the venue to their fixtures
/// in ONE place here rather than scattering the literal. A different-venue test would supply
/// that venue plus matching instrument fixtures.
pub(super) fn fixture_execution_venue() -> Venue {
    Venue::from("POLYMARKET")
}

pub(super) fn fixture_settlement_account_id() -> String {
    fixture_settlement_identity().0
}

pub(super) fn fixture_settlement_currency() -> Currency {
    fixture_settlement_identity().1
}

pub(super) fn noop_settlement_health_transition_emitter()
-> crate::bolt_v3_operator_health::BoltV3SettlementHealthTransitionEmitter {
    Arc::new(|_| Ok(()))
}

pub(super) fn attach_recording_settlement_health_transitions(
    strategy: &mut BinaryOracleEdgeTaker,
) -> Arc<Mutex<Vec<crate::bolt_v3_operator_health::BoltV3SettlementHealthTransition>>> {
    let transitions = Arc::new(Mutex::new(Vec::new()));
    let recorded = transitions.clone();
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_health_transition_emitter(Some(Arc::new(move |transition| {
            recorded
                .lock()
                .expect("recording settlement health transition mutex poisoned")
                .push(transition);
            Ok(())
        })));
    transitions
}

fn fixture_settlement_identity() -> (String, Currency) {
    let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("bolt-v3 fixture root should load for settlement identity");
    let pool = loaded
        .root
        .risk
        .capital_pools
        .as_ref()
        .and_then(|pools| {
            pools
                .iter()
                .find(|pool| pool.venue_id == fixture_execution_venue().as_str())
        })
        .expect("bolt-v3 fixture root should declare a capital pool for the fixture venue");
    (
        pool.account_id.to_string(),
        crate::bolt_v3_strategy_registration::settlement_currency_from_config_code(
            pool.collateral_currency.as_str(),
        )
        .expect("bolt-v3 fixture capital pool should declare a registered settlement currency"),
    )
}

pub(crate) fn fixture_order_economics() -> crate::bolt_v3_order_execution::BoltV3OrderEconomicsHandle
{
    crate::bolt_v3_economics_test_support::fixture_order_economics()
}

fn fee_free_fixture_order_economics() -> crate::bolt_v3_order_execution::BoltV3OrderEconomicsHandle
{
    crate::bolt_v3_economics_test_support::fee_free_fixture_order_economics()
}

pub(super) fn use_fee_bearing_economics(strategy: &mut BinaryOracleEdgeTaker) {
    strategy.context = strategy
        .context
        .clone()
        .with_order_economics_for_test(fixture_order_economics());
}

pub(super) fn test_strategy() -> BinaryOracleEdgeTaker {
    test_strategy_with_decision_evidence(recording_decision_evidence())
}

pub(super) fn register_test_strategy(strategy: &mut BinaryOracleEdgeTaker) -> Rc<RefCell<Cache>> {
    let (cache, _clock) = register_test_strategy_with_clock(strategy);
    cache
}

pub(super) fn register_test_strategy_with_clock(
    strategy: &mut BinaryOracleEdgeTaker,
) -> (Rc<RefCell<Cache>>, Rc<RefCell<TestClock>>) {
    install_test_data_command_sender();
    if strategy.is_registered() {
        let cache = strategy.cache_rc();
        let clock = registered_test_clock_for_cache(&cache);
        return (cache, clock);
    }

    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_200_u64 * NANOS_PER_MILLI_U64));
    let cache = Rc::new(RefCell::new(Cache::default()));
    let cache_handle = cache.clone();
    let clock_handle = clock.clone();
    let portfolio = Rc::new(RefCell::new(Portfolio::new(
        clock.clone(),
        cache.clone(),
        None,
    )));
    strategy
        .core
        .register(
            TraderId::from("TRADER-001"),
            clock,
            cache.clone(),
            portfolio,
        )
        .expect("test strategy should register with NT core");
    strategy.context = strategy
        .context
        .clone()
        .with_position_authority(fixture_position_authority_capability_with_cache(cache));
    record_registered_test_clock(&cache_handle, &clock_handle);
    (cache_handle, clock_handle)
}

#[derive(Debug)]
struct RecordingDataCommandSender;

impl DataCommandSender for RecordingDataCommandSender {
    fn execute(&self, command: DataCommand) {
        TEST_DATA_COMMANDS.with(|commands| {
            commands
                .lock()
                .expect("recording data command sender lock should not be poisoned")
                .push(command);
        });
    }
}

thread_local! {
    static TEST_DATA_COMMANDS: std::sync::Arc<std::sync::Mutex<Vec<DataCommand>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    static REGISTERED_TEST_CLOCKS: RefCell<std::collections::HashMap<usize, Rc<RefCell<TestClock>>>> =
        RefCell::new(std::collections::HashMap::new());
}

fn test_cache_key(cache: &Rc<RefCell<Cache>>) -> usize {
    Rc::as_ptr(cache) as usize
}

fn record_registered_test_clock(cache: &Rc<RefCell<Cache>>, clock: &Rc<RefCell<TestClock>>) {
    REGISTERED_TEST_CLOCKS.with(|clocks| {
        clocks
            .borrow_mut()
            .insert(test_cache_key(cache), Rc::clone(clock));
    });
}

fn registered_test_clock_for_cache(cache: &Rc<RefCell<Cache>>) -> Rc<RefCell<TestClock>> {
    REGISTERED_TEST_CLOCKS.with(|clocks| {
        clocks
            .borrow()
            .get(&test_cache_key(cache))
            .cloned()
            .expect("registered test strategy should retain its TestClock")
    })
}

fn install_test_data_command_sender() {
    msgbus::register_data_command_endpoint(
        MessagingSwitchboard::data_engine_queue_execute(),
        TypedIntoHandler::from(|command: DataCommand| {
            get_data_cmd_sender().execute(command);
        }),
    );
    replace_data_cmd_sender(std::sync::Arc::new(RecordingDataCommandSender));
    TEST_DATA_COMMANDS.with(|commands| {
        commands
            .lock()
            .expect("recording data command sender lock should not be poisoned")
            .clear();
    });
}

pub(super) fn recorded_data_commands() -> Vec<DataCommand> {
    TEST_DATA_COMMANDS.with(|commands| {
        commands
            .lock()
            .expect("recording data command sender lock should not be poisoned")
            .clone()
    })
}

pub(super) fn register_test_strategy_with_active_instruments(strategy: &mut BinaryOracleEdgeTaker) {
    let cache = register_test_strategy(strategy);
    add_active_instruments_to_cache(strategy, &cache);
}

pub(super) fn register_test_strategy_with_instrument(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: &InstrumentId,
) {
    let cache = register_test_strategy(strategy);
    cache
        .borrow_mut()
        .add_instrument(updown_binary_option(
            &instrument_id.to_string(),
            "test-market-entry",
            "test-market",
            "Up",
            1_000,
            1_300,
        ))
        .expect("test cache should accept selected instrument");
}

pub(super) fn add_active_instruments_to_cache(
    strategy: &BinaryOracleEdgeTaker,
    cache: &Rc<RefCell<Cache>>,
) {
    let up_instrument_id = strategy
        .active
        .books
        .up
        .instrument_id
        .expect("test strategy must have active up instrument");
    let down_instrument_id = strategy
        .active
        .books
        .down
        .instrument_id
        .expect("test strategy must have active down instrument");
    let up_instrument_id = up_instrument_id.to_string();
    let down_instrument_id = down_instrument_id.to_string();
    let mut cache = cache.borrow_mut();
    cache
        .add_instrument(updown_binary_option(
            &up_instrument_id,
            "test-market-up",
            "test-market",
            "Up",
            1_000,
            1_300,
        ))
        .expect("test cache should accept active up instrument");
    cache
        .add_instrument(updown_binary_option(
            &down_instrument_id,
            "test-market-down",
            "test-market",
            "Down",
            1_000,
            1_300,
        ))
        .expect("test cache should accept active down instrument");
}

pub(super) fn test_strategy_with_decision_evidence(
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) -> BinaryOracleEdgeTaker {
    test_strategy_with_decision_evidence_and_submit_admission(
        decision_evidence,
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(
                recording_decision_evidence(),
            ),
        ),
    )
}

pub(super) fn test_strategy_with_decision_evidence_and_submit_admission(
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
    submit_admission: Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState>,
) -> BinaryOracleEdgeTaker {
    let mut strategy = BinaryOracleEdgeTaker::new(
        BinaryOracleEdgeTakerConfig {
            strategy_id: "BINARYORACLEEDGETAKER-001".to_string(),
            order_id_tag: "001".to_string(),
            oms_type: "netting".to_string(),
            client_id: "POLYMARKET".to_string(),
            configured_target_id: "configured_updown_target".to_string(),
            target_kind: "rotating_market".to_string(),
            rotating_market_family: "updown".to_string(),
            underlying_asset: "CONFIGURED_ASSET".to_string(),
            cadence_seconds: 300,
            cadence_slug_token: "5m".to_string(),
            market_selection_rule: "active_or_next".to_string(),
            retry_interval_seconds: 5,
            blocked_after_seconds: 60,
            signal_venue: Some("signal_data_client".to_string()),
            signal_instrument_id: Some("SIGNAL.SOURCE".to_string()),
            resolution_client_id: Some("CHAINLINK_DATA_STREAMS".to_string()),
            resolution_instrument_id: Some("CONFIGURED_ASSET-USD.CHAINLINK".to_string()),
            realized_volatility_surface_id: "<surface_id>".to_string(),
            realized_volatility_max_source_age_ms: 500,
            static_condition_id: None,
            static_yes_outcome: None,
            static_no_outcome: None,
            static_fair_probability_source: None,
            use_uuid_client_order_ids: true,
            use_hyphens_in_client_order_ids: false,
            external_order_claims: vec!["AUXILIARY.SOURCE".to_string()],
            manage_contingent_orders: true,
            manage_gtd_expiry: true,
            manage_stop: true,
            market_exit_interval_ms: 250,
            market_exit_max_attempts: 7,
            log_events: false,
            log_commands: false,
            log_rejected_due_post_only_as_warning: false,
            entry_order: BinaryOracleEdgeTakerOrderConfig {
                side: "buy".to_string(),
                position_side: "long".to_string(),
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Fok,
                expire_time_unix_nanos: None,
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                is_post_only: false,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
            exit_order: BinaryOracleEdgeTakerOrderConfig {
                side: "sell".to_string(),
                position_side: "long".to_string(),
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                expire_time_unix_nanos: None,
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                is_post_only: false,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
            forced_exit_order: BinaryOracleEdgeTakerOrderConfig {
                side: "sell".to_string(),
                position_side: "long".to_string(),
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                expire_time_unix_nanos: None,
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                is_post_only: false,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
            warmup_tick_count: 20,
            reentry_cooldown_secs: 30,
            order_notional_target: 1000.0,
            maximum_position_notional: 1000.0,
            book_impact_cap_bps: 15,
            vwap_depth_limit_bps: 15,
            slippage_buffer_bps: 15,
            risk_lambda: 0.5,
            sizing_ev_reference_bps: 500,
            edge_threshold_basis_points: 0,
            exit_hysteresis_bps: 5,
            trade_flow_window_secs: 30,
            trade_flow_max_samples: 100,
            spike_guard_return_threshold: 0.05,
            spike_guard_cooldown_secs: 5,
            price_to_beat_source: "chainlink_data_streams.report_at_boundary".to_string(),
            pricing_kurtosis: 0.0,
            theta_decay_factor: 0.0,
            forced_flat_stale_reference_ms: 1500,
            forced_flat_thin_book_min_liquidity: 100.0,
            lead_agreement_min_corr: 0.8,
            lead_jitter_max_ms: 250,
            reference_current_price: None,
        },
        StrategyBuildContext::new(
            fee_free_fixture_order_economics(),
            decision_evidence,
            submit_admission,
            crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
            fixture_execution_venue(),
        )
        .with_settlement_account_id(Some(fixture_settlement_account_id()))
        .with_settlement_currency(Some(fixture_settlement_currency()))
        .with_settlement_health_transition_emitter(Some(
            noop_settlement_health_transition_emitter(),
        )),
    );
    register_test_strategy(&mut strategy);
    // Every producer that keys on an episode runs after market selection in
    // production, so a fixture with no bound identity is not a realistic
    // strategy -- it is one in which episode evidence is unattributable and
    // therefore not recorded at all. Bound here so the recording tests exercise
    // the path they mean to.
    strategy.active.evidence_identity = Some(fixture_evidence_identity());
    strategy
}

/// The bound market identity every episode-keyed fixture shares.
pub(super) fn fixture_evidence_identity() -> SelectedMarketEvidenceIdentity {
    SelectedMarketEvidenceIdentity::try_new(
        "fixture-market".to_string(),
        "fixture-condition".to_string(),
        "fixture-question".to_string(),
        false,
        [
            SelectedMarketEvidenceOutcome {
                index: 0,
                normalized_outcome: "up".to_string(),
                clob_token_id: "fixture-up".to_string(),
            },
            SelectedMarketEvidenceOutcome {
                index: 1,
                normalized_outcome: "down".to_string(),
                clob_token_id: "fixture-down".to_string(),
            },
        ],
    )
    .expect("fixture evidence identity must be valid")
}

pub(super) fn quote_tick(instrument_id: &str, bid: f64, ask: f64, ts_ms: u64) -> QuoteTick {
    quote_tick_with_stamps(instrument_id, bid, ask, ts_ms, ts_ms)
}

pub(super) fn quote_tick_with_stamps(
    instrument_id: &str,
    bid: f64,
    ask: f64,
    ts_event_ms: u64,
    ts_init_ms: u64,
) -> QuoteTick {
    QuoteTick::new_checked(
        InstrumentId::from(instrument_id),
        Price::new(bid, 2),
        Price::new(ask, 2),
        Quantity::new(1.0, 0),
        Quantity::new(1.0, 0),
        nautilus_core::UnixNanos::from(ts_event_ms.saturating_mul(NANOS_PER_MILLI_U64)),
        nautilus_core::UnixNanos::from(ts_init_ms.saturating_mul(NANOS_PER_MILLI_U64)),
    )
    .expect("test quote tick should be valid")
}

pub(super) fn invalid_quote_tick(instrument_id: &str, ts_ms: u64) -> QuoteTick {
    invalid_quote_tick_with_stamps(instrument_id, ts_ms, ts_ms)
}

pub(super) fn invalid_quote_tick_with_stamps(
    instrument_id: &str,
    ts_event_ms: u64,
    ts_init_ms: u64,
) -> QuoteTick {
    let invalid_price = Price::from_raw(nautilus_model::types::PRICE_ERROR, 0);
    QuoteTick::new_checked(
        InstrumentId::from(instrument_id),
        invalid_price,
        invalid_price,
        Quantity::new(1.0, 0),
        Quantity::new(1.0, 0),
        nautilus_core::UnixNanos::from(ts_event_ms.saturating_mul(NANOS_PER_MILLI_U64)),
        nautilus_core::UnixNanos::from(ts_init_ms.saturating_mul(NANOS_PER_MILLI_U64)),
    )
    .expect("test invalid quote tick should preserve sentinel prices")
}

pub(super) fn trade_tick(
    instrument_id: &str,
    price: f64,
    ts_ms: u64,
) -> nautilus_model::data::TradeTick {
    trade_tick_with_aggressor(
        instrument_id,
        price,
        1.0,
        nautilus_model::enums::AggressorSide::Buy,
        ts_ms,
    )
}

pub(super) fn trade_tick_with_aggressor(
    instrument_id: &str,
    price: f64,
    size: f64,
    aggressor: nautilus_model::enums::AggressorSide,
    ts_ms: u64,
) -> nautilus_model::data::TradeTick {
    trade_tick_with_aggressor_ns(
        instrument_id,
        price,
        size,
        aggressor,
        ts_ms.saturating_mul(NANOS_PER_MILLI_U64),
    )
}

pub(super) fn trade_tick_with_aggressor_ns(
    instrument_id: &str,
    price: f64,
    size: f64,
    aggressor: nautilus_model::enums::AggressorSide,
    ts_ns: u64,
) -> nautilus_model::data::TradeTick {
    let trade_id = format!(
        "{}{ts_ns}",
        test_identifier_token(std::any::type_name::<nautilus_model::data::TradeTick>())
    );
    nautilus_model::data::TradeTick::new_checked(
        InstrumentId::from(instrument_id),
        Price::new(price, TEST_TRADE_PRICE_PRECISION),
        Quantity::new(size, TEST_TRADE_SIZE_PRECISION),
        aggressor,
        nautilus_model::identifiers::TradeId::from(trade_id.as_str()),
        nautilus_core::UnixNanos::from(ts_ns),
        nautilus_core::UnixNanos::from(ts_ns),
    )
    .expect("test trade tick should be valid")
}

pub(super) fn test_identifier_token(raw: &str) -> String {
    raw.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(TEST_IDENTIFIER_TOKEN_LIMIT)
        .collect()
}

pub(super) fn submit_admission_with_provider_cap(
    max_notional_per_order: Decimal,
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) -> Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
    let mut limits = BTreeMap::new();
    limits.insert(
        "POLYMARKET".to_string(),
        crate::bolt_v3_submit_admission::BoltV3LiveSubmitApprovalLimits {
            max_order_count: 1,
            max_order_notional: max_notional_per_order,
        },
    );
    Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_with_live_submit_limits(
            decision_evidence,
            limits,
        ),
    )
}

pub(super) fn submit_admission_with_canonical_position(
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
    yes_instrument_id: InstrumentId,
    no_instrument_id: InstrumentId,
    yes_position: Decimal,
) -> Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
    submit_admission_with_limits_and_canonical_position(
        decision_evidence,
        BTreeMap::new(),
        yes_instrument_id,
        no_instrument_id,
        yes_position,
        Decimal::ZERO,
    )
}

#[expect(clippy::too_many_arguments)]
pub(super) fn submit_admission_with_provider_cap_and_canonical_position(
    max_notional_per_order: Decimal,
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
    yes_instrument_id: InstrumentId,
    no_instrument_id: InstrumentId,
    yes_position: Decimal,
    no_position: Decimal,
) -> Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
    let mut limits = BTreeMap::new();
    limits.insert(
        "POLYMARKET".to_string(),
        crate::bolt_v3_submit_admission::BoltV3LiveSubmitApprovalLimits {
            max_order_count: 1,
            max_order_notional: max_notional_per_order,
        },
    );
    submit_admission_with_limits_and_canonical_position(
        decision_evidence,
        limits,
        yes_instrument_id,
        no_instrument_id,
        yes_position,
        no_position,
    )
}

#[expect(clippy::too_many_arguments)]
fn submit_admission_with_limits_and_canonical_position(
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
    live_submit_approval_limits: BTreeMap<
        String,
        crate::bolt_v3_submit_admission::BoltV3LiveSubmitApprovalLimits,
    >,
    yes_instrument_id: InstrumentId,
    no_instrument_id: InstrumentId,
    yes_position: Decimal,
    no_position: Decimal,
) -> Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
    use crate::{
        bolt_v3_capital_admission::{
            CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
            ProductAdmissionSnapshot, ProductKind,
        },
        bolt_v3_capital_admission_state::{
            OrderLifecycleCapitalAdmissionSnapshot,
            POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE,
            PortfolioCapitalAdmissionSnapshot, ProviderCollateralAllowanceSnapshot,
        },
        bolt_v3_capital_reservation::CapitalPoolSnapshot,
        bolt_v3_submit_admission::{
            BoltV3SubmitAdmissionState, BoltV3SubmitCapitalAdmissionConfig,
            BoltV3SubmitCapitalAdmissionNtComponents,
        },
    };

    let venue_id = fixture_execution_venue().to_string();
    let account_id = fixture_settlement_account_id();
    let collateral_currency = fixture_settlement_currency().to_string();
    let admission = Arc::new(
        BoltV3SubmitAdmissionState::new_with_live_submit_limits_and_optional_controls(
            decision_evidence,
            live_submit_approval_limits,
            None,
            Some(BoltV3SubmitCapitalAdmissionConfig {
                venue_id: venue_id.clone(),
                account_id: account_id.clone(),
                product_kind: ProductKind::PredictionMarketBinary,
                collateral_currency: collateral_currency.clone(),
                capital_pool: CapitalPoolSnapshot {
                    source: "test-capital-pool".to_string(),
                    observed_at_ns: 0,
                    pool_id: "test-capital-pool".to_string(),
                    max_pool_liability: Decimal::new(100_000, 0),
                    committed_liability: Decimal::ZERO,
                    max_snapshot_age_ns: u64::MAX,
                },
                policy: CapitalAdmissionPolicy {
                    min_remaining_pool_balance: None,
                    fee_slippage_policy: Some(FeeSlippagePolicy {
                        max_fee_liability: Decimal::new(100_000, 0),
                        max_slippage_liability: Decimal::new(100_000, 0),
                    }),
                },
            }),
        ),
    );
    admission.update_capital_admission_nt_components(BoltV3SubmitCapitalAdmissionNtComponents {
        source: "nt-capital-admission-state".to_string(),
        observed_at_ns: 0,
        portfolio: PortfolioCapitalAdmissionSnapshot {
            source: "nt-portfolio-snapshot".to_string(),
            observed_at_ns: 0,
            venue_id: venue_id.clone(),
            account_id: account_id.clone(),
            collateral_currency: collateral_currency.clone(),
            free_collateral: Decimal::new(100_000, 0),
            total_equity: Decimal::new(100_000, 0),
        },
        provider_collateral_allowance: ProviderCollateralAllowanceSnapshot {
            source: POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string(),
            observed_at_ns: 0,
            venue_id,
            account_id,
            collateral_currency,
            collateral_allowance: Decimal::new(100_000, 0),
        },
        order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
            source: "nt-open-order-cache".to_string(),
            observed_at_ns: 0,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "nt-prediction-market-snapshot".to_string(),
                observed_at_ns: 0,
                yes_instrument_id: yes_instrument_id.to_string(),
                no_instrument_id: no_instrument_id.to_string(),
                yes_position,
                no_position,
                collateral_allowance: Decimal::new(100_000, 0),
                collateral_coupled_group_id: "test-market".to_string(),
            },
        ),
        loss_snapshot: None,
    });
    let rebuild = admission
        .rebuild_capital_admission_open_order_reservations_for_test(Vec::new(), 1)
        .expect("capital admission fixture rebuild should preserve invariants");
    assert!(rebuild.accepted, "canonical test position should reconcile");
    admission
}

pub(super) fn configure_supported_market_quote_entry_order(strategy: &mut BinaryOracleEdgeTaker) {
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.expire_time_unix_nanos = None;
    strategy.config.entry_order.trigger_price = None;
    strategy.config.entry_order.activation_price = None;
    strategy.config.entry_order.trigger_type = None;
    strategy.config.entry_order.trigger_instrument_id = None;
    strategy.config.entry_order.trailing_offset = None;
    strategy.config.entry_order.trailing_offset_type = None;
    strategy.config.entry_order.is_post_only = false;
    strategy.config.entry_order.is_reduce_only = false;
    strategy.config.entry_order.is_quote_quantity = true;
}

pub(super) fn configure_limit_base_entry_order(strategy: &mut BinaryOracleEdgeTaker) {
    strategy.config.entry_order.order_type = OrderType::Limit;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.expire_time_unix_nanos = None;
    strategy.config.entry_order.trigger_price = None;
    strategy.config.entry_order.activation_price = None;
    strategy.config.entry_order.trigger_type = None;
    strategy.config.entry_order.trigger_instrument_id = None;
    strategy.config.entry_order.trailing_offset = None;
    strategy.config.entry_order.trailing_offset_type = None;
    strategy.config.entry_order.is_post_only = false;
    strategy.config.entry_order.is_reduce_only = false;
    strategy.config.entry_order.is_quote_quantity = false;
}

pub(super) fn ready_to_trade_strategy() -> BinaryOracleEdgeTaker {
    let mut strategy = test_strategy();
    configure_supported_market_quote_entry_order(&mut strategy);
    strategy.config.warmup_tick_count = 2;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    strategy.active.price_to_beat = Some(3_100.0);
    strategy.active.interval_open = Some(3_100.0);
    strategy.active.warmup_count = 2;
    strategy.active.last_reference_ts_ms = Some(1_200);
    strategy.active.books.up.last_observed_instrument_id = strategy.active.books.up.instrument_id;
    strategy
        .active
        .books
        .up
        .bid_levels
        .insert(Price::new(0.43, 2), 5_000.0);
    strategy
        .active
        .books
        .up
        .ask_levels
        .insert(Price::new(0.45, 2), 5_000.0);
    strategy.active.books.up.best_bid = Some(0.43);
    strategy.active.books.up.best_ask = Some(0.45);
    strategy.active.books.up.liquidity_available = Some(5_000.0);
    strategy.active.books.down.last_observed_instrument_id =
        strategy.active.books.down.instrument_id;
    strategy
        .active
        .books
        .down
        .bid_levels
        .insert(Price::new(0.43, 2), 5_000.0);
    strategy
        .active
        .books
        .down
        .ask_levels
        .insert(Price::new(0.45, 2), 5_000.0);
    strategy.active.books.down.best_bid = Some(0.43);
    strategy.active.books.down.best_ask = Some(0.45);
    strategy.active.books.down.liquidity_available = Some(5_000.0);
    strategy.active.fast_venue_incoherent = false;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_100.5, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    strategy.pricing.last_lead_gap_probability = Some(probability(0.0));
    strategy.pricing.last_jitter_penalty_probability = Some(probability(0.0));
    strategy
}

pub(super) fn ready_to_trade_strategy_with_bound_economics() -> BinaryOracleEdgeTaker {
    let mut strategy = test_strategy();
    configure_supported_market_quote_entry_order(&mut strategy);
    strategy.config.warmup_tick_count = 2;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    strategy.active.price_to_beat = Some(3_100.0);
    strategy.active.interval_open = Some(3_100.0);
    strategy.active.warmup_count = 2;
    strategy.active.last_reference_ts_ms = Some(1_200);
    strategy.active.books.up.last_observed_instrument_id = strategy.active.books.up.instrument_id;
    strategy
        .active
        .books
        .up
        .bid_levels
        .insert(Price::new(0.50, 2), 5_000.0);
    strategy
        .active
        .books
        .up
        .ask_levels
        .insert(Price::new(0.50, 2), 5_000.0);
    strategy.active.books.up.best_bid = Some(0.50);
    strategy.active.books.up.best_ask = Some(0.50);
    strategy.active.books.up.liquidity_available = Some(5_000.0);
    strategy.active.books.down.last_observed_instrument_id =
        strategy.active.books.down.instrument_id;
    strategy
        .active
        .books
        .down
        .bid_levels
        .insert(Price::new(0.48, 2), 5_000.0);
    strategy
        .active
        .books
        .down
        .ask_levels
        .insert(Price::new(0.49, 2), 5_000.0);
    strategy.active.books.down.best_bid = Some(0.48);
    strategy.active.books.down.best_ask = Some(0.49);
    strategy.active.books.down.liquidity_available = Some(5_000.0);
    strategy.active.fast_venue_incoherent = false;
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_100.5, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    strategy.pricing.last_lead_gap_probability = Some(probability(0.0));
    strategy.pricing.last_jitter_penalty_probability = Some(probability(0.0));
    strategy
}

pub(super) fn ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
    decision_evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
    submit_admission: Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState>,
) -> BinaryOracleEdgeTaker {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    strategy.context = StrategyBuildContext::new(
        fee_free_fixture_order_economics(),
        decision_evidence,
        submit_admission,
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        fixture_execution_venue(),
    )
    .with_position_authority(fixture_position_authority_capability(&strategy))
    .with_settlement_account_id(Some(fixture_settlement_account_id()))
    .with_settlement_currency(Some(fixture_settlement_currency()))
    .with_settlement_health_transition_emitter(Some(noop_settlement_health_transition_emitter()));
    strategy.config.edge_threshold_basis_points = 1;
    strategy.active.price_to_beat = Some(3_100.0);
    strategy
}

pub(super) fn set_shadow_order_execution_policy(strategy: &mut BinaryOracleEdgeTaker) {
    strategy.context = strategy.context.clone().with_order_execution_policy(
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::shadow(),
    );
}

pub(super) fn selected_entry_instrument(strategy: &BinaryOracleEdgeTaker) -> InstrumentId {
    strategy
        .entry_evaluation_at(1_200)
        .selected_side
        .and_then(|side| strategy.instrument_id_for_side(side))
        .or_else(|| configured_outcome_instruments(strategy).into_iter().next())
        .expect("ready-to-trade fixture should expose a configured instrument")
}

pub(super) fn configured_position_probe(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
) -> OpenPositionState {
    let book = if strategy.active.books.up.instrument_id == Some(instrument_id) {
        strategy.active.books.up.clone()
    } else if strategy.active.books.down.instrument_id == Some(instrument_id) {
        strategy.active.books.down.clone()
    } else {
        OutcomeBookState::from_instrument_id(instrument_id)
    };
    OpenPositionState {
        lifecycle: active_fixture_lifecycle_for_instrument(strategy, instrument_id),
        instrument_id,
        position_id: PositionId::from("P-SIDE-PROBE"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(1.0, 2),
        avg_px_open: 0.450,
        book,
    }
}

fn active_fixture_lifecycle_for_instrument(
    strategy: &BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
) -> BoltV3PositionMarketLifecycle {
    let outcome_side = if strategy.active.books.up.instrument_id == Some(instrument_id) {
        Some(OutcomeSide::Up)
    } else if strategy.active.books.down.instrument_id == Some(instrument_id) {
        Some(OutcomeSide::Down)
    } else {
        None
    };
    BoltV3PositionMarketLifecycle::from_entry_context(
        strategy.active.market_id.clone(),
        outcome_side,
        strategy.active.price_to_beat,
        strategy.active.interval_open,
        strategy.active.interval_end_ms,
        strategy.active.selection_published_at_ms,
        strategy.active.seconds_to_expiry_at_selection,
    )
}

fn seed_managed_position_lifecycle_from_active_fixture(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
) {
    let lifecycle = active_fixture_lifecycle_for_instrument(strategy, instrument_id);
    if let Some(managed) = strategy.exposure.managed_position_context_mut() {
        managed.lifecycle = lifecycle;
    }
    strategy.sync_exposure_context_from_active();
}

pub(super) fn configured_book_for_instrument(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
) -> OutcomeBookState {
    configured_position_probe(strategy, instrument_id).book
}

pub(super) fn set_active_books_best_prices(
    strategy: &mut BinaryOracleEdgeTaker,
    bid: f64,
    ask: f64,
) {
    let mut updates = Vec::new();
    for instrument_id in configured_outcome_instruments(strategy) {
        let book = configured_book_for_instrument(strategy, instrument_id);
        let bid_size = book
            .bid_levels
            .last_key_value()
            .map(|(_, size)| *size)
            .expect("configured fixture book should expose bid liquidity");
        let ask_size = book
            .ask_levels
            .first_key_value()
            .map(|(_, size)| *size)
            .expect("configured fixture book should expose ask liquidity");
        updates.push((instrument_id, bid_size, ask_size));
    }

    for (instrument_id, bid_size, ask_size) in updates {
        assert!(
            strategy.active.books.update_from_deltas(&book_deltas(
                instrument_id,
                &[
                    (BookAction::Clear, OrderSide::Buy, bid, bid_size),
                    (BookAction::Add, OrderSide::Buy, bid, bid_size),
                    (BookAction::Add, OrderSide::Sell, ask, ask_size),
                ],
            )),
            "configured fixture book should accept price update"
        );
    }
}

pub(super) fn set_configured_books_depth(
    strategy: &mut BinaryOracleEdgeTaker,
    deltas: &[(BookAction, OrderSide, f64, f64)],
) {
    for instrument_id in configured_outcome_instruments(strategy) {
        assert!(
            strategy
                .active
                .books
                .update_from_deltas(&book_deltas(instrument_id, deltas)),
            "configured fixture book should accept depth update"
        );
    }
}

pub(super) fn pending_entry_state(
    strategy: &mut BinaryOracleEdgeTaker,
    client_order_id: ClientOrderId,
) -> PendingEntryState {
    let instrument_id = selected_entry_instrument(strategy);
    let probe = configured_position_probe(strategy, instrument_id);
    let outcome_side = probe
        .lifecycle
        .outcome_side()
        .expect("configured instrument should materialize with an outcome side");
    let book = probe.book;
    PendingEntryState {
        client_order_id,
        submitted_at_ms: Some(1_000),
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(outcome_side),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id,
        book,
    }
}

pub(super) fn configured_instrument_except(
    strategy: &BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
) -> InstrumentId {
    configured_outcome_instruments(strategy)
        .into_iter()
        .find(|configured_instrument_id| *configured_instrument_id != instrument_id)
        .expect("fixture should expose a second configured outcome instrument")
}

pub(super) fn foreign_venue_instrument_id(
    strategy: &BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
) -> InstrumentId {
    let foreign_instrument_id = InstrumentId::new(instrument_id.symbol, Venue::from("HYPERLIQUID"));
    assert_ne!(
        foreign_instrument_id.venue,
        strategy.context.execution_venue(),
        "foreign fixture instrument must not be on the execution venue",
    );
    foreign_instrument_id
}

pub(super) fn seed_nt_open_position(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    avg_px_open: f64,
) {
    seed_nt_open_position_with_details(
        strategy,
        instrument_id,
        position_id,
        quantity,
        avg_px_open,
        OrderSide::Buy,
    );
}

pub(super) fn seed_nt_open_position_with_details(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    avg_px_open: f64,
    entry_order_side: OrderSide,
) {
    let cache = register_test_strategy(strategy);
    let instrument = cache
        .borrow()
        .instrument(&instrument_id)
        .cloned()
        .unwrap_or_else(|| {
            updown_binary_option(
                instrument_id.to_string().as_str(),
                "test-position-market",
                strategy
                    .active
                    .market_id
                    .as_deref()
                    .unwrap_or("test-market"),
                if strategy.active.books.down.instrument_id == Some(instrument_id) {
                    "Down"
                } else {
                    "Up"
                },
                strategy.active.interval_start_ms.unwrap_or(1_000),
                strategy.active.interval_end_ms.unwrap_or(301_000),
            )
        });
    let mut fill = order_filled_event_with_details(
        ClientOrderId::from(format!("ENTRY-{position_id}").as_str()),
        instrument_id,
        Some(position_id),
        entry_order_side,
    );
    fill.strategy_id = StrategyId::from(strategy.config.strategy_id.as_str());
    fill.last_qty = quantity;
    fill.last_px = Price::new(avg_px_open, instrument.price_precision());
    let position = Position::new(&instrument, fill);
    let mut cache = cache.borrow_mut();
    if cache.instrument(&instrument_id).is_none() {
        cache
            .add_instrument(instrument)
            .expect("test cache should accept position instrument");
    }
    cache
        .add_position(&position, NtOmsType::Netting)
        .expect("test cache should accept authoritative open position");
}

pub(super) fn close_nt_position(strategy: &mut BinaryOracleEdgeTaker, position_id: PositionId) {
    let cache = register_test_strategy(strategy);
    let mut position = cache
        .borrow()
        .position_owned(&position_id)
        .expect("test cache should contain the position being closed");
    let close_side = match position.side {
        PositionSide::Long => OrderSide::Sell,
        PositionSide::Short => OrderSide::Buy,
        PositionSide::Flat | PositionSide::NoPositionSide => {
            panic!("test position must be open before close")
        }
    };
    let mut fill = order_filled_event_with_details(
        ClientOrderId::from("CLOSE-TEST"),
        position.instrument_id,
        Some(position_id),
        close_side,
    );
    fill.strategy_id = StrategyId::from(strategy.config.strategy_id.as_str());
    fill.trade_id = nautilus_model::identifiers::TradeId::from("TRADE-CLOSE-TEST");
    fill.last_qty = position.quantity;
    position.apply(&fill);
    assert!(
        !position.is_open(),
        "test close fill must flatten the position"
    );
    cache
        .borrow_mut()
        .update_position(&position)
        .expect("test cache should accept the closed position");
}

pub(super) fn materialize_configured_position(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    avg_px_open: f64,
) -> OpenPositionState {
    let book = configured_book_for_instrument(strategy, instrument_id);
    let client_order_id = ClientOrderId::from(format!("FIXTURE-ENTRY-{position_id}").as_str());
    set_pending_entry(
        strategy,
        PendingEntryState {
            client_order_id,
            submitted_at_ms: Some(1_000),
            lifecycle: active_fixture_lifecycle_for_instrument(strategy, instrument_id),
            instrument_id,
            book,
        },
    );
    seed_nt_open_position(strategy, instrument_id, position_id, quantity, avg_px_open);
    strategy.materialize_position_from_event(
        PositionMaterializationSpec {
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity,
            avg_px_open,
        },
        0,
    );
    seed_managed_position_lifecycle_from_active_fixture(strategy, instrument_id);
    let position = managed_position_snapshot(strategy)
        .expect("configured position should materialize as managed exposure");
    position
}

pub(super) fn configured_outcome_instruments(
    strategy: &BinaryOracleEdgeTaker,
) -> Vec<InstrumentId> {
    let instrument_ids = [
        strategy.active.books.up.instrument_id,
        strategy.active.books.down.instrument_id,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    assert!(
        !instrument_ids.is_empty(),
        "ready-to-trade fixture should expose configured outcome instruments"
    );
    instrument_ids
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FixturePositionLineage {
    CurrentProcess,
    RestartObserved,
}

pub(super) fn set_pending_entry(strategy: &mut BinaryOracleEdgeTaker, pending: PendingEntryState) {
    strategy.exposure.set_pending_entry_for_test(pending);
}

pub(super) fn set_entry_reconcile_pending(
    strategy: &mut BinaryOracleEdgeTaker,
    pending: PendingEntryState,
    reason: EntryReconcileReason,
) {
    strategy
        .exposure
        .set_entry_reconcile_for_test(pending, reason);
}

pub(super) fn set_entry_reconcile_pending_after_fill(
    strategy: &mut BinaryOracleEdgeTaker,
    pending: PendingEntryState,
    reason: EntryReconcileReason,
) {
    strategy
        .exposure
        .set_entry_reconcile_for_test(pending, reason);
}

pub(super) fn set_managed_position(
    strategy: &mut BinaryOracleEdgeTaker,
    position: OpenPositionState,
    lineage: FixturePositionLineage,
) {
    seed_nt_open_position(
        strategy,
        position.instrument_id,
        position.position_id,
        position.quantity,
        position.avg_px_open,
    );
    match lineage {
        FixturePositionLineage::CurrentProcess => strategy
            .exposure
            .set_managed_for_test(managed_position_context(position)),
        FixturePositionLineage::RestartObserved => strategy.exposure.set_blind_recovery_for_test(
            BlindRecoveryReason::RestartOpenPosition {
                instrument_id: position.instrument_id,
                position_id: position.position_id,
            },
        ),
    }
}

pub(super) fn set_managed_position_with_pending_entry(
    strategy: &mut BinaryOracleEdgeTaker,
    position: OpenPositionState,
    lineage: FixturePositionLineage,
    pending_entry: PendingEntryState,
) {
    seed_nt_open_position(
        strategy,
        position.instrument_id,
        position.position_id,
        position.quantity,
        position.avg_px_open,
    );
    match lineage {
        FixturePositionLineage::CurrentProcess => strategy.exposure.set_entry_remainder_for_test(
            pending_entry,
            EntryRemainderPosition::Supported(managed_position_context(position)),
        ),
        FixturePositionLineage::RestartObserved => strategy.exposure.set_blind_recovery_for_test(
            BlindRecoveryReason::RestartOpenPosition {
                instrument_id: position.instrument_id,
                position_id: position.position_id,
            },
        ),
    }
}

pub(super) fn materialize_managed_position_with_resting_pending_entry(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
) -> (InstrumentId, ClientOrderId) {
    let client_order_id = ClientOrderId::from(format!("ENTRY-WORKING-{instrument_id}").as_str());
    let book = configured_book_for_instrument(strategy, instrument_id);
    let pending = PendingEntryState {
        client_order_id,
        submitted_at_ms: Some(1_000),
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            None,
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id,
        book: book.clone(),
    };
    let avg_px_open = book
        .best_ask
        .expect("ready-to-trade fixture should expose an ask");
    set_pending_entry(strategy, pending);
    seed_nt_open_position(strategy, instrument_id, position_id, quantity, avg_px_open);
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        quantity,
        avg_px_open,
    ));
    (instrument_id, client_order_id)
}

pub(super) fn set_exit_pending(
    strategy: &mut BinaryOracleEdgeTaker,
    position: OpenPositionState,
    client_order_id: ClientOrderId,
    lineage: FixturePositionLineage,
) {
    seed_nt_open_position(
        strategy,
        position.instrument_id,
        position.position_id,
        position.quantity,
        position.avg_px_open,
    );
    let lease = strategy
        .context
        .position_authority()
        .expect("fixture strategy should have position authority")
        .acquire_for_position(position.position_id, position.instrument_id)
        .expect("fixture exit authority lease should acquire");
    seed_nt_working_exit_order(
        strategy,
        client_order_id,
        position.instrument_id,
        position.position_id,
        position.quantity,
    );
    if lineage == FixturePositionLineage::RestartObserved {
        strategy
            .exposure
            .set_blind_recovery_for_test(BlindRecoveryReason::RestartOpenPosition {
                instrument_id: position.instrument_id,
                position_id: position.position_id,
            });
        return;
    }
    let authority = BoltV3ExitOrderAuthorityHandle::locally_submitted_for_test(
        client_order_id,
        position.instrument_id,
        position.position_id,
        position.quantity.as_decimal(),
        position.side.as_specified(),
        position.quantity,
        lease,
    )
    .expect("fixture local exit authority should build");
    strategy
        .exposure
        .set_exit_pending_for_test(ExitPendingState {
            pending_exit: PendingExitState {
                client_order_id,
                submitted_at_ms: Some(1_000),
                market_id: position.lifecycle.market_id_owned(),
                position_id: Some(position.position_id),
            },
            position: Some(managed_position_context(position)),
            authority,
        });
}

fn seed_nt_working_exit_order(
    strategy: &mut BinaryOracleEdgeTaker,
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
) -> nautilus_model::orders::OrderAny {
    let order = recovered_exit_order(client_order_id, instrument_id, quantity);
    seed_nt_working_order(strategy, order, position_id)
}

pub(super) fn seed_nt_working_order(
    strategy: &mut BinaryOracleEdgeTaker,
    order: nautilus_model::orders::OrderAny,
    position_id: PositionId,
) -> nautilus_model::orders::OrderAny {
    let client_order_id = order.client_order_id();
    let instrument_id = order.instrument_id();
    let account_id = AccountId::from(fixture_settlement_account_id().as_str());
    let cache = register_test_strategy(strategy);
    let mut cache = cache.borrow_mut();
    cache
        .add_order(
            order.clone(),
            Some(position_id),
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept working exit order");
    cache
        .update_order(&nautilus_model::events::OrderEventAny::Submitted(
            nautilus_model::events::OrderSubmitted::new(
                order.trader_id(),
                order.strategy_id(),
                instrument_id,
                client_order_id,
                account_id,
                nautilus_core::UUID4::new(),
                UnixNanos::from(900_u64),
                UnixNanos::from(900_u64),
            ),
        ))
        .expect("test cache should submit working exit order");
    cache
        .update_order(&nautilus_model::events::OrderEventAny::Accepted(
            nautilus_model::events::OrderAccepted::new(
                order.trader_id(),
                order.strategy_id(),
                instrument_id,
                client_order_id,
                nautilus_model::identifiers::VenueOrderId::from("V-EXIT-001"),
                account_id,
                nautilus_core::UUID4::new(),
                UnixNanos::from(950_u64),
                UnixNanos::from(950_u64),
                false,
            ),
        ))
        .expect("test cache should accept working exit order");
    cache
        .order_owned(&client_order_id)
        .expect("test cache should retain working exit order")
}

pub(super) fn apply_exit_order_event_to_nt_cache(
    strategy: &mut BinaryOracleEdgeTaker,
    event: nautilus_model::events::OrderEventAny,
) {
    register_test_strategy(strategy)
        .borrow_mut()
        .update_order(&event)
        .expect("test cache should apply exit order event");
}

pub(super) fn observe_position_authority_report(
    strategy: &BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    position_side: PositionSideSpecified,
    quantity: Quantity,
    ts_last_ns: u64,
) {
    strategy
        .context
        .position_authority()
        .expect("fixture strategy should have position authority")
        .observe_for_test(&PositionStatusReport::new(
            AccountId::from("TEST-ACCOUNT"),
            instrument_id,
            position_side,
            quantity,
            UnixNanos::from(ts_last_ns),
            UnixNanos::from(ts_last_ns),
            None,
            None,
            None,
        ))
        .expect("fixture position authority report should be accepted");
}

pub(super) fn recovered_exit_order(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    quantity: Quantity,
) -> nautilus_model::orders::OrderAny {
    nautilus_model::orders::OrderAny::Market(
        nautilus_model::orders::MarketOrder::new_checked(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from("BINARYORACLEEDGETAKER-001"),
            instrument_id,
            client_order_id,
            OrderSide::Sell,
            quantity,
            TimeInForce::Ioc,
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_u64),
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("fixture recovered exit order should construct"),
    )
}

pub(super) fn set_blind_recovery(
    strategy: &mut BinaryOracleEdgeTaker,
    reason: BlindRecoveryReason,
) {
    strategy.exposure.set_blind_recovery_for_test(reason);
}

pub(super) fn set_unsupported_observed(
    strategy: &mut BinaryOracleEdgeTaker,
    observed: OpenPositionState,
    reason: UnsupportedObservedReason,
) {
    seed_nt_open_position_with_details(
        strategy,
        observed.instrument_id,
        observed.position_id,
        observed.quantity,
        observed.avg_px_open,
        observed.entry_order_side,
    );
    strategy
        .exposure
        .set_unsupported_for_test(UnsupportedObservedState {
            context: managed_position_context(observed),
            reason,
        });
}

pub(super) fn managed_position_snapshot(
    strategy: &BinaryOracleEdgeTaker,
) -> Option<OpenPositionState> {
    strategy.managed_position().map(|managed| managed.position)
}

pub(super) fn tracked_position_snapshot(
    strategy: &BinaryOracleEdgeTaker,
) -> Option<OpenPositionState> {
    strategy.tracked_observed_position()
}

pub(super) fn pending_exit_snapshot(strategy: &BinaryOracleEdgeTaker) -> Option<PendingExitState> {
    strategy
        .exposure
        .exit_pending_snapshot()
        .map(|exit_pending| exit_pending.pending_exit)
}

pub(super) fn assert_foreign_venue_blind_recovery(strategy: &BinaryOracleEdgeTaker) {
    assert!(
        matches!(
            strategy.exposure.blind_recovery_reason(),
            Some(BlindRecoveryReason::ForeignVenuePosition { .. })
        ),
        "foreign-venue terminal event must be quarantined to blind recovery, got {:?}",
        strategy.exposure,
    );
}

pub(super) fn active_snapshot(market_id: &str) -> RuntimeSelectionSnapshot {
    active_snapshot_with_start(market_id, 0)
}

pub(super) fn active_snapshot_with_start(
    market_id: &str,
    interval_start_ms: u64,
) -> RuntimeSelectionSnapshot {
    selection_snapshot(
        interval_start_ms,
        SelectionState::Active {
            market: Box::new(candidate_market(market_id, interval_start_ms)),
        },
    )
}

pub(super) fn freeze_snapshot_with_start(
    market_id: &str,
    interval_start_ms: u64,
) -> RuntimeSelectionSnapshot {
    selection_snapshot(
        interval_start_ms,
        SelectionState::Freeze {
            market: Box::new(candidate_market(market_id, interval_start_ms)),
            reason: "freeze window".to_string(),
        },
    )
}

pub(super) fn selection_snapshot(
    interval_start_ms: u64,
    state: SelectionState,
) -> RuntimeSelectionSnapshot {
    RuntimeSelectionSnapshot {
        ruleset_id: "BINARYORACLEEDGETAKER".to_string(),
        decision: SelectionDecision {
            ruleset_id: "BINARYORACLEEDGETAKER".to_string(),
            state,
        },
        eligible_candidates: Vec::new(),
        published_at_ms: interval_start_ms,
    }
}

pub(super) fn candidate_market(market_id: &str, interval_start_ms: u64) -> CandidateMarket {
    let condition_id = format!("condition-{market_id}");
    let up_token_id = format!("{market_id}-UP");
    let down_token_id = format!("{market_id}-DOWN");
    let up_instrument_id = format!("{condition_id}-{up_token_id}.POLYMARKET");
    let down_instrument_id = format!("{condition_id}-{down_token_id}.POLYMARKET");
    CandidateMarket {
        market_id: market_id.to_string(),
        instrument_id: up_instrument_id.clone(),
        up: CandidateOutcome {
            instrument_id: up_instrument_id,
        },
        down: CandidateOutcome {
            instrument_id: down_instrument_id,
        },
        evidence_identity: SelectedMarketEvidenceIdentity::try_new(
            market_id.to_string(),
            condition_id.clone(),
            format!("question-{market_id}"),
            false,
            [
                SelectedMarketEvidenceOutcome {
                    index: 0,
                    normalized_outcome: "up".to_string(),
                    clob_token_id: up_token_id,
                },
                SelectedMarketEvidenceOutcome {
                    index: 1,
                    normalized_outcome: "down".to_string(),
                    clob_token_id: down_token_id,
                },
            ],
        )
        .expect("candidate fixture evidence identity must be valid"),
        source_identity: SelectedMarketSourceIdentity {
            condition_id,
            market_slug: format!("slug-{market_id}"),
            question_id: format!("question-{market_id}"),
        },
        selection_outcome: MarketSelectionOutcome::Current,
        price_to_beat: None,
        start_ts_ms: interval_start_ms,
        expiration_ts_ms: interval_start_ms.saturating_add(300 * MILLIS_PER_SECOND_U64),
        seconds_to_end: 300,
    }
}

pub(super) fn updown_binary_option(
    instrument_id: &str,
    market_slug: &str,
    market_id: &str,
    outcome: &str,
    activation_ms: u64,
    expiration_ms: u64,
) -> InstrumentAny {
    let mut info = Params::new();
    info.insert(
        "market_slug".to_string(),
        serde_json::Value::String(market_slug.to_string()),
    );
    info.insert(
        "market_id".to_string(),
        serde_json::Value::String(market_id.to_string()),
    );
    info.insert(
        "condition_id".to_string(),
        serde_json::Value::String(format!("condition-{market_id}")),
    );
    info.insert(
        "question_id".to_string(),
        serde_json::Value::String(format!("question-{market_id}")),
    );
    // Production instruments carry this; selection now refuses a market whose
    // evidence identity cannot be completed without it.
    info.insert("neg_risk".to_string(), serde_json::Value::Bool(false));
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from(instrument_id),
        Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
        AssetClass::Alternative,
        Currency::USDC(),
        (activation_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
        (expiration_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
        3,
        2,
        Price::from("0.001"),
        Quantity::from("0.01"),
        Some(ustr::Ustr::from(outcome)),
        None,
        None,
        None,
        None,
        None,
        // max_price: a binary option's structural payout ceiling. Mirrors the
        // upstream NT Polymarket adapter (MAX_PRICE = "0.999") so the fixture
        // declares the same hard price bound production instruments carry —
        // the only price a market-style order (BUY or SELL, entry or exit) can
        // ever fill or settle at, which the market-style admission valuation
        // uses as the universal worst case.
        Some(Price::from("0.999")),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(info),
        1.into(),
        1.into(),
    ))
}

pub(super) fn reference_tick(timestamp_ms: u64, price: f64) -> ReferenceSnapshot {
    ReferenceSnapshot {
        ts_ms: timestamp_ms,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(price),
        confidence: 1.0,
        venues: Vec::new(),
    }
}

pub(super) fn orderbook_venue(
    venue_name: &str,
    effective_weight: f64,
    price: f64,
    observed_ts_ms: u64,
) -> EffectiveVenueState {
    EffectiveVenueState {
        venue_name: venue_name.to_string(),
        base_weight: effective_weight,
        effective_weight,
        stale: false,
        health: VenueHealth::Healthy,
        observed_ts_ms: Some(observed_ts_ms),
        venue_kind: VenueKind::Orderbook,
        observed_price: Some(price),
        observed_bid: Some(price - 0.01),
        observed_ask: Some(price + 0.01),
    }
}

pub(super) fn oracle_venue(
    venue_name: &str,
    effective_weight: f64,
    price: f64,
    observed_ts_ms: u64,
) -> EffectiveVenueState {
    EffectiveVenueState {
        venue_name: venue_name.to_string(),
        base_weight: effective_weight,
        effective_weight,
        stale: false,
        health: VenueHealth::Healthy,
        observed_ts_ms: Some(observed_ts_ms),
        venue_kind: VenueKind::Oracle,
        observed_price: Some(price),
        observed_bid: None,
        observed_ask: None,
    }
}

pub(super) fn fast_spot(venue_name: &str, price: f64, observed_ts_ms: u64) -> FastSpotObservation {
    fast_spot_received(venue_name, price, observed_ts_ms, None)
}

/// Like [`fast_spot`] but with an explicit `received_ts_ms`. Use this for
/// expectations compared against observations produced by paths that record the
/// receive time (signal quotes carry `ts_init`; reference-price quotes carry the
/// upstream receive timestamp). The bare [`fast_spot`] defaults `received_ts_ms`
/// to `None`, which matches lead-arbitration outputs and directly-seeded spots.
pub(super) fn fast_spot_received(
    venue_name: &str,
    price: f64,
    observed_ts_ms: u64,
    received_ts_ms: Option<u64>,
) -> FastSpotObservation {
    FastSpotObservation {
        venue: venue_name.to_string(),
        price,
        observed_ts_ms,
        received_ts_ms,
    }
}

pub(super) fn book_deltas(
    instrument_id: InstrumentId,
    deltas: &[(BookAction, OrderSide, f64, f64)],
) -> nautilus_model::data::OrderBookDeltas {
    book_deltas_with_stamps(instrument_id, deltas, 0, 0)
}

pub(super) fn book_deltas_with_stamps(
    instrument_id: InstrumentId,
    deltas: &[(BookAction, OrderSide, f64, f64)],
    ts_event_ms: u64,
    ts_init_ms: u64,
) -> nautilus_model::data::OrderBookDeltas {
    let deltas = deltas
        .iter()
        .map(|(action, side, price, size)| {
            nautilus_model::data::OrderBookDelta::new_checked(
                instrument_id,
                *action,
                nautilus_model::data::BookOrder::new(
                    *side,
                    Price::new(*price, 2),
                    Quantity::new(*size, 2),
                    0,
                ),
                0,
                0,
                UnixNanos::from(ts_event_ms.saturating_mul(NANOS_PER_MILLI_U64)),
                UnixNanos::from(ts_init_ms.saturating_mul(NANOS_PER_MILLI_U64)),
            )
            .expect("test order book delta should build")
        })
        .collect();

    nautilus_model::data::OrderBookDeltas::new(instrument_id, deltas)
}

pub(super) fn lead_signal(
    venue_name: &str,
    age_ms: u64,
    jitter_ms: u64,
    agreement_corr: f64,
    effective_weight: f64,
    lead_gap_probability: f64,
) -> LeadVenueSignal {
    LeadVenueSignal {
        venue_name: venue_name.to_string(),
        price: Some(3_100.0),
        observed_ts_ms: Some(1_000),
        age_ms,
        jitter_ms,
        agreement_corr: probability(agreement_corr),
        effective_weight,
        lead_gap_probability: probability(lead_gap_probability),
    }
}

pub(super) fn position_opened_event(
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    avg_px_open: f64,
) -> nautilus_model::events::PositionOpened {
    position_opened_event_with_details(
        instrument_id,
        position_id,
        quantity,
        avg_px_open,
        OrderSide::Buy,
        PositionSide::Long,
    )
}

pub(super) fn position_opened_event_with_details(
    instrument_id: InstrumentId,
    position_id: PositionId,
    quantity: Quantity,
    avg_px_open: f64,
    entry: OrderSide,
    side: PositionSide,
) -> nautilus_model::events::PositionOpened {
    nautilus_model::events::PositionOpened {
        trader_id: nautilus_model::identifiers::TraderId::from("TRADER-001"),
        strategy_id: StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        position_id,
        account_id: nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
        opening_order_id: ClientOrderId::from("ENTRY-001"),
        entry,
        side,
        signed_qty: quantity.as_f64(),
        quantity,
        last_qty: quantity,
        last_px: Price::new(avg_px_open, 3),
        currency: nautilus_model::types::Currency::USDC(),
        avg_px_open,
        realized_pnl: None,
        event_id: nautilus_core::UUID4::new(),
        ts_event: nautilus_core::UnixNanos::from(1_u64),
        ts_init: nautilus_core::UnixNanos::from(1_u64),
    }
}

pub(super) fn order_filled_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    position_id: PositionId,
) -> nautilus_model::events::OrderFilled {
    order_filled_event_with_details(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Buy,
    )
}

pub(super) fn order_filled_event_with_details(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    position_id: Option<PositionId>,
    order_side: OrderSide,
) -> nautilus_model::events::OrderFilled {
    let mut fill = nautilus_model::events::OrderFilled::new(
        nautilus_model::identifiers::TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_model::identifiers::VenueOrderId::from("V-ORDER-001"),
        nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
        nautilus_model::identifiers::TradeId::from("TRADE-001"),
        order_side,
        nautilus_model::enums::OrderType::Limit,
        Quantity::new(10.0, 2),
        Price::new(0.450, 3),
        nautilus_model::types::Currency::USDC(),
        nautilus_model::enums::LiquiditySide::Taker,
        nautilus_core::UUID4::new(),
        nautilus_core::UnixNanos::from(1_000_u64),
        nautilus_core::UnixNanos::from(1_000_u64),
        false,
        None,
        Some(nautilus_model::types::Money::new(
            0.0,
            nautilus_model::types::Currency::USDC(),
        )),
        None,
    );
    fill.position_id = position_id;
    fill
}

pub(super) fn order_canceled_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> nautilus_model::events::OrderCanceled {
    nautilus_model::events::OrderCanceled::new(
        nautilus_model::identifiers::TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_core::UUID4::new(),
        nautilus_core::UnixNanos::from(1_000_u64),
        nautilus_core::UnixNanos::from(1_000_u64),
        false,
        Some(nautilus_model::identifiers::VenueOrderId::from(
            "V-ORDER-001",
        )),
        Some(nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT")),
    )
}

pub(super) fn order_fill_voided_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    position_id: PositionId,
    trade_id: nautilus_model::identifiers::TradeId,
    voided_qty: Quantity,
    ts_event_ns: u64,
) -> nautilus_model::events::OrderFillVoided {
    nautilus_model::events::order::spec::OrderFillVoidedSpec::builder()
        .trader_id(nautilus_model::identifiers::TraderId::from("TRADER-001"))
        .strategy_id(StrategyId::from("BINARYORACLEEDGETAKER-001"))
        .instrument_id(instrument_id)
        .client_order_id(client_order_id)
        .venue_order_id(nautilus_model::identifiers::VenueOrderId::from(
            "V-ORDER-001",
        ))
        .account_id(AccountId::from("TEST-ACCOUNT"))
        .trade_id(trade_id)
        .voided_qty(voided_qty)
        .commission_voided(nautilus_model::types::Money::new(
            0.0,
            nautilus_model::types::Currency::USDC(),
        ))
        .order_side(OrderSide::Sell)
        .order_type(OrderType::Limit)
        .last_px(Price::new(0.45, 2))
        .currency(nautilus_model::types::Currency::USDC())
        .liquidity_side(nautilus_model::enums::LiquiditySide::Taker)
        .position_id(position_id)
        .is_reopened(true)
        .ts_event(UnixNanos::from(ts_event_ns))
        .ts_init(UnixNanos::from(ts_event_ns))
        .build()
}

pub(super) fn order_rejected_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> nautilus_model::events::OrderRejected {
    order_rejected_event_with_reason(client_order_id, instrument_id, "rejected")
}

pub(super) fn order_rejected_event_with_reason(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    reason: &'static str,
) -> nautilus_model::events::OrderRejected {
    nautilus_model::events::OrderRejected::new(
        nautilus_model::identifiers::TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
        reason.into(),
        nautilus_core::UUID4::new(),
        nautilus_core::UnixNanos::from(1_000_u64),
        nautilus_core::UnixNanos::from(1_000_u64),
        false,
        false,
    )
}

pub(super) fn order_denied_event_with_reason(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    reason: &'static str,
) -> nautilus_model::events::OrderDenied {
    nautilus_model::events::OrderDenied::new(
        nautilus_model::identifiers::TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        reason.into(),
        nautilus_core::UUID4::new(),
        nautilus_core::UnixNanos::from(1_000_u64),
        nautilus_core::UnixNanos::from(1_000_u64),
    )
}

pub(super) fn order_expired_event(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> nautilus_model::events::OrderExpired {
    nautilus_model::events::OrderExpired::new(
        nautilus_model::identifiers::TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        client_order_id,
        nautilus_core::UUID4::new(),
        nautilus_core::UnixNanos::from(1_000_u64),
        nautilus_core::UnixNanos::from(1_000_u64),
        false,
        Some(nautilus_model::identifiers::VenueOrderId::from(
            "V-ORDER-001",
        )),
        Some(nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT")),
    )
}

pub(super) fn position_closed_event(
    instrument_id: InstrumentId,
    position_id: PositionId,
) -> nautilus_model::events::PositionClosed {
    nautilus_model::events::PositionClosed {
        trader_id: nautilus_model::identifiers::TraderId::from("TRADER-001"),
        strategy_id: StrategyId::from("BINARYORACLEEDGETAKER-001"),
        instrument_id,
        position_id,
        account_id: nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
        opening_order_id: ClientOrderId::from("ENTRY-001"),
        closing_order_id: Some(ClientOrderId::from("EXIT-001")),
        entry: OrderSide::Buy,
        side: PositionSide::Long,
        signed_qty: 0.0,
        quantity: Quantity::zero(2),
        peak_quantity: Quantity::new(10.0, 2),
        last_qty: Quantity::new(10.0, 2),
        last_px: Price::new(0.550, 3),
        currency: nautilus_model::types::Currency::USDC(),
        avg_px_open: 0.450,
        avg_px_close: Some(0.550),
        realized_return: 0.0,
        realized_pnl: None,
        unrealized_pnl: nautilus_model::types::Money::new(
            0.0,
            nautilus_model::types::Currency::USDC(),
        ),
        duration: nautilus_core::nanos::DurationNanos::from(1_u64),
        event_id: nautilus_core::UUID4::new(),
        ts_opened: nautilus_core::UnixNanos::from(1_u64),
        ts_closed: Some(nautilus_core::UnixNanos::from(2_u64)),
        ts_event: nautilus_core::UnixNanos::from(2_u64),
        ts_init: nautilus_core::UnixNanos::from(2_u64),
    }
}

/// Single source of truth for the binary-oracle-taker top-level config field
/// set: the serde-derived `deny_unknown_fields` deserializer for
/// `BinaryOracleEdgeTakerConfig`. When an unknown key is fed, serde's error
/// enumerates every field the struct accepts (`expected one of `a`, `b`,
/// ...`). This is the ONLY authoritative list of valid field names, generated
/// directly from the struct definition.
pub(super) fn serde_known_top_level_config_fields() -> std::collections::BTreeSet<String> {
    let mut raw = valid_raw_config();
    let sentinel = "definitely_not_a_real_field_sentinel";
    raw.as_table_mut()
        .expect("valid config must be a table")
        .insert(sentinel.to_string(), Value::Boolean(true));

    let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect_err("config with an unknown key must fail serde deny_unknown_fields");
    let message = format!("{err:#}");

    let marker = "expected one of ";
    let list_start = message.find(marker).unwrap_or_else(|| {
        panic!("serde error must enumerate the expected field list, got: {message}")
    }) + marker.len();
    let list = &message[list_start..];

    let fields: std::collections::BTreeSet<String> = list
        .split('`')
        .filter(|segment| {
            // The backtick-delimited segments alternate between field names and
            // separators (", "); keep only the identifier-shaped segments.
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .map(|segment| segment.to_string())
        .collect();

    assert!(
        !fields.is_empty(),
        "serde SSOT field extraction produced no fields from: {message}"
    );
    fields
}

/// Returns the set of top-level field names the runtime `validate_table`
/// allowlist accepts, probed behaviorally: a name is accepted iff inserting it
/// into an otherwise-valid config does not produce an `unknown_field` error.
pub(super) fn validate_table_accepted_top_level_fields(
    candidates: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let mut accepted = std::collections::BTreeSet::new();
    for name in candidates {
        let mut raw = valid_raw_config();
        let table = raw.as_table_mut().expect("valid config must be a table");
        if !table.contains_key(name.as_str()) {
            // Use a string value: order-table fields are validated separately,
            // but for the allowlist gate only the key presence matters.
            table.insert(name.clone(), Value::String("probe".to_string()));
        }
        let mut errors = Vec::new();
        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        let flagged_unknown = errors.iter().any(|e| {
            e.field == format!("strategies[0].config.{name}") && e.code == "unknown_field"
        });
        if !flagged_unknown {
            accepted.insert(name.clone());
        }
    }
    accepted
}

pub(super) fn assert_limit_gtc_post_only_order(
    order: OrderAny,
    expected_side: OrderSide,
    expected_price: Price,
) {
    let OrderAny::Limit(order) = order else {
        panic!("maker order should be built as an NT limit order");
    };
    assert_eq!(order.order_side(), expected_side);
    assert_eq!(order.order_type(), OrderType::Limit);
    assert_eq!(order.time_in_force(), TimeInForce::Gtc);
    assert_eq!(order.price(), Some(expected_price));
    assert!(order.is_post_only());
    assert!(!order.is_reduce_only());
    assert!(!order.is_quote_quantity());
    assert_eq!(order.expire_time(), None);
}
