use std::{cell::RefCell, collections::BTreeMap, rc::Rc, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_common::{cache::Cache, clock::TestClock};
use nautilus_core::UnixNanos;
use nautilus_model::{
    identifiers::{InstrumentId, TraderId},
    instruments::InstrumentAny,
    types::Price,
};
use nautilus_portfolio::portfolio::Portfolio;
use rust_decimal::{Decimal, prelude::FromPrimitive};
use serde::{Deserialize, Serialize};
use toml::Value;

use crate::{
    bolt_v3_book_sizing::OutcomeBookState,
    bolt_v3_decision_evidence::{
        BoltV3ReadinessGateEvidenceSnapshot, validate_readiness_gate_evidence_snapshot,
    },
    bolt_v3_numeric::{MIDPOINT_DIVISOR_F64, is_non_negative_finite, is_positive_finite},
    bolt_v3_operator_artifacts::EntryReadinessGateSession,
    bolt_v3_price_to_beat::price_to_beat_from_readiness_session,
    bolt_v3_taker_pricing::FastSpotObservation,
    bolt_v3_volatility::RealizedVolEstimator,
    strategies::registry::{FeeProvider, StrategyBuildContext},
};

use super::{
    BinaryOracleEdgeTaker, BinaryOracleEdgeTakerBuilder, NANOS_PER_MILLI_U64, realized_vol_config,
    selection::{SelectionState, selection_snapshot_from_entry_decision_source},
};

pub const ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION: u32 = 3;
pub const ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND: &str =
    "bolt_v3.binary_oracle_entry_decision_source.v3";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryDecisionEvidenceSource {
    pub schema_version: u32,
    pub record_kind: String,
    pub market_selection_timestamp_ms: u64,
    pub decision_timestamp_ms: u64,
    pub readiness_session: EntryReadinessGateSession,
    pub warmup_count: u64,
    pub reference_quote: BinaryOracleEntryReferenceQuoteSource,
    pub signal_quote: BinaryOracleEntrySignalQuoteSource,
    pub realized_volatility: BinaryOracleEntryRealizedVolatilitySource,
    pub fees: BinaryOracleEntryFeeSource,
    pub books: BinaryOracleEntryBooksSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryReferenceQuoteSource {
    pub venue: String,
    pub price: f64,
    pub observed_ts_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntrySignalQuoteSource {
    pub venue: String,
    pub price: f64,
    pub observed_ts_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryRealizedVolatilitySource {
    pub value: f64,
    pub ready_ts_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct BinaryOracleReferenceQuoteObservationSource<'a> {
    pub data_client_id: &'a str,
    pub instrument_id: &'a str,
    pub bid_price: f64,
    pub ask_price: f64,
    pub ts_event_unix_nanos: u64,
}

#[derive(Debug, Clone)]
pub struct BinaryOracleEntryReferenceProofSources {
    pub reference_quote: BinaryOracleEntryReferenceQuoteSource,
    pub realized_volatility: BinaryOracleEntryRealizedVolatilitySource,
}

pub fn derive_entry_reference_proofs_from_quote_observations(
    raw_config: &Value,
    observations: &[BinaryOracleReferenceQuoteObservationSource<'_>],
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
) -> Result<BinaryOracleEntryReferenceProofSources> {
    if decision_timestamp_ms < market_selection_timestamp_ms {
        anyhow::bail!("entry reference proof decision timestamp precedes market selection");
    }

    let config = BinaryOracleEdgeTakerBuilder::parse_config(raw_config)?;
    let reference_venue = config.reference_venue.as_ref().ok_or_else(|| {
        anyhow::anyhow!("reference quote observation source requires configured reference_venue")
    })?;
    let reference_instrument_id = config.reference_instrument_id.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "reference quote observation source requires configured reference_instrument_id"
        )
    })?;
    let mut sorted_observations = observations.to_vec();
    sorted_observations.sort_by_key(|observation| observation.ts_event_unix_nanos);
    let mut estimator = RealizedVolEstimator::from_config(&realized_vol_config(&config));
    let mut latest_quote = None;
    let mut latest_ready_volatility = None;

    for observation in sorted_observations {
        if observation.data_client_id != *reference_venue
            || observation.instrument_id != *reference_instrument_id
        {
            continue;
        }
        if observation.ts_event_unix_nanos == 0
            || !is_positive_finite(observation.bid_price)
            || !is_positive_finite(observation.ask_price)
        {
            anyhow::bail!("reference quote observation source contains invalid quote data");
        }
        let observed_ts_ms = observation.ts_event_unix_nanos / NANOS_PER_MILLI_U64;
        if observed_ts_ms > decision_timestamp_ms {
            continue;
        }
        let midpoint = (observation.bid_price + observation.ask_price) / MIDPOINT_DIVISOR_F64;
        if !is_positive_finite(midpoint) {
            anyhow::bail!("reference quote observation source midpoint is invalid");
        }
        let quote = FastSpotObservation {
            venue: reference_venue.clone(),
            price: midpoint,
            observed_ts_ms,
        };
        if let Some(value) = estimator.observe(&quote.venue, quote.price, quote.observed_ts_ms)
            && observed_ts_ms >= market_selection_timestamp_ms
        {
            latest_ready_volatility = Some(BinaryOracleEntryRealizedVolatilitySource {
                value,
                ready_ts_ms: observed_ts_ms,
            });
        }
        if observed_ts_ms >= market_selection_timestamp_ms {
            latest_quote = Some(BinaryOracleEntryReferenceQuoteSource {
                venue: reference_venue.clone(),
                price: midpoint,
                observed_ts_ms,
            });
        }
    }

    let reference_quote = latest_quote.ok_or_else(|| {
        anyhow::anyhow!(
            "reference quote observation source did not produce a configured reference quote"
        )
    })?;
    let realized_volatility = latest_ready_volatility.ok_or_else(|| {
        anyhow::anyhow!(
            "reference quote observation source did not produce ready realized volatility"
        )
    })?;
    Ok(BinaryOracleEntryReferenceProofSources {
        reference_quote,
        realized_volatility,
    })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryFeeSource {
    pub fee_bps_by_instrument_id: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryBooksSource {
    pub price_precision: u8,
    pub up: BinaryOracleEntryBookSideSource,
    pub down: BinaryOracleEntryBookSideSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryBookSideSource {
    pub best_bid: f64,
    pub bid_quantity: f64,
    pub best_ask: f64,
    pub ask_quantity: f64,
    pub liquidity_available: f64,
}

#[derive(Debug, Clone)]
struct SourceFeeProvider {
    fee_bps_by_instrument_id: BTreeMap<String, Decimal>,
}

impl FeeProvider for SourceFeeProvider {
    fn fee_bps(&self, instrument_id: InstrumentId) -> Option<Decimal> {
        self.fee_bps_by_instrument_id
            .get(&instrument_id.to_string())
            .copied()
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

pub fn record_entry_decision_evidence_from_source(
    raw_config: &Value,
    decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
    trader_id: TraderId,
    source: &BinaryOracleEntryDecisionEvidenceSource,
    instruments: &[InstrumentAny],
) -> Result<()> {
    validate_entry_decision_source(source)?;
    if instruments.is_empty() {
        anyhow::bail!("entry decision evidence source requires at least one instrument");
    }

    let fee_provider = Arc::new(SourceFeeProvider {
        fee_bps_by_instrument_id: source_fee_bps_by_instrument_id(source)?,
    });
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(
            decision_evidence.clone(),
        ),
    );
    let readiness_evidence = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(
        &source.readiness_session,
    );
    let price_to_beat = price_to_beat_from_readiness_session(&source.readiness_session)?;
    // Execution venue for this replay is the venue of the stored selection's outcome instruments
    // (the strategy only ever trades the venue its selected market is on). This offline evidence
    // path validates against that stored selection and never reads the live cache, so the venue is
    // used only for context completeness; it is still derived from the source rather than assumed.
    let execution_venue = source
        .readiness_session
        .selected_market
        .instrument_ids
        .iter()
        .find_map(|instrument_id| InstrumentId::from_str(instrument_id.as_str()).ok())
        .map(|instrument_id| instrument_id.venue)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "entry decision evidence source selected market is missing a parseable instrument required for execution-venue resolution"
            )
        })?;
    let context = StrategyBuildContext::new(
        fee_provider,
        decision_evidence,
        submit_admission,
        execution_venue,
    )
    .with_readiness_evidence(readiness_evidence);
    let mut strategy = BinaryOracleEdgeTaker::new(
        BinaryOracleEdgeTakerBuilder::parse_config(raw_config)?,
        context,
    );
    register_source_replay_strategy(&mut strategy, trader_id, source, instruments)?;

    let mut selection =
        selection_snapshot_from_entry_decision_source(&strategy.config, source, instruments);
    let SelectionState::Active { market } = &mut selection.decision.state else {
        anyhow::bail!("entry decision evidence source did not select an active configured market");
    };
    market.price_to_beat = Some(price_to_beat);
    selection.published_at_ms = source.market_selection_timestamp_ms;
    strategy.apply_selection_snapshot(selection);
    strategy.observe_reference_quote(&FastSpotObservation {
        venue: source.reference_quote.venue.clone(),
        price: source.reference_quote.price,
        observed_ts_ms: source.reference_quote.observed_ts_ms,
    });
    strategy.observe_signal_quote(&FastSpotObservation {
        venue: source.signal_quote.venue.clone(),
        price: source.signal_quote.price,
        observed_ts_ms: source.signal_quote.observed_ts_ms,
    });
    strategy.active.warmup_count = source.warmup_count;
    // The replay source owns this ready RV value; a missing venue deliberately
    // clears stale attribution and lets source reporting fall back to the
    // just-observed signal quote when that quote is still selected.
    strategy.pricing.seed_ready_realized_vol(
        None,
        source.realized_volatility.value,
        source.realized_volatility.ready_ts_ms,
    );
    strategy.refresh_fee_readiness();
    apply_entry_decision_source_books(&mut strategy, &source.books)?;

    match strategy.try_submit_entry_order(source.decision_timestamp_ms) {
        Err(error) => Err(error),
        Ok(Some(_client_order_id)) => Ok(()),
        Ok(None) => {
            let decision = strategy.entry_submission_decision_at(source.decision_timestamp_ms);
            anyhow::bail!(
                "entry decision evidence source did not produce an entry order: blocked_reason={:?} gate_blocked_by={:?} pricing_blocked_by={:?} selected_side={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} min_worst_case_ev_bps={:?} sized_notional={:?}",
                decision.blocked_reason,
                decision.evaluation.gate.blocked_by,
                decision.evaluation.pricing_blocked_by,
                decision.evaluation.selected_side,
                decision.evaluation.up_worst_case_ev_bps,
                decision.evaluation.down_worst_case_ev_bps,
                decision.evaluation.min_worst_case_ev_bps,
                decision.evaluation.sized_notional,
            )
        }
    }
}

fn validate_entry_decision_source(source: &BinaryOracleEntryDecisionEvidenceSource) -> Result<()> {
    anyhow::ensure!(
        source.schema_version == ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION,
        "entry decision evidence source schema_version is invalid"
    );
    anyhow::ensure!(
        source.record_kind == ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND,
        "entry decision evidence source record_kind is invalid"
    );
    anyhow::ensure!(
        source.decision_timestamp_ms >= source.market_selection_timestamp_ms,
        "entry decision evidence source decision_timestamp_ms precedes market selection"
    );
    let readiness_evidence = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(
        &source.readiness_session,
    );
    validate_readiness_gate_evidence_snapshot(&readiness_evidence)?;
    price_to_beat_from_readiness_session(&source.readiness_session)?;
    anyhow::ensure!(
        is_positive_finite(source.reference_quote.price),
        "entry decision evidence source reference quote price is invalid"
    );
    anyhow::ensure!(
        is_positive_finite(source.signal_quote.price),
        "entry decision evidence source signal quote price is invalid"
    );
    anyhow::ensure!(
        is_positive_finite(source.realized_volatility.value),
        "entry decision evidence source realized volatility is invalid"
    );
    Ok(())
}

fn source_fee_bps_by_instrument_id(
    source: &BinaryOracleEntryDecisionEvidenceSource,
) -> Result<BTreeMap<String, Decimal>> {
    let mut fees = BTreeMap::new();
    for (instrument_id, fee_bps) in &source.fees.fee_bps_by_instrument_id {
        anyhow::ensure!(
            !instrument_id.trim().is_empty(),
            "entry decision evidence source fee instrument id is required"
        );
        anyhow::ensure!(
            is_non_negative_finite(*fee_bps),
            "entry decision evidence source fee bps is invalid"
        );
        let fee_bps = Decimal::from_f64(*fee_bps)
            .ok_or_else(|| anyhow::anyhow!("entry decision evidence source fee bps is invalid"))?;
        fees.insert(instrument_id.clone(), fee_bps);
    }
    Ok(fees)
}

fn register_source_replay_strategy(
    strategy: &mut BinaryOracleEdgeTaker,
    trader_id: TraderId,
    source: &BinaryOracleEntryDecisionEvidenceSource,
    instruments: &[InstrumentAny],
) -> Result<()> {
    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock.borrow_mut().set_time(UnixNanos::from(
        source
            .decision_timestamp_ms
            .saturating_mul(NANOS_PER_MILLI_U64),
    ));
    let cache = Rc::new(RefCell::new(Cache::new(None, None)));
    let portfolio = Rc::new(RefCell::new(Portfolio::new(
        cache.clone(),
        clock.clone(),
        None,
    )));
    strategy
        .core
        .register(trader_id, clock, cache.clone(), portfolio)
        .context("failed to register source replay strategy core")?;
    let mut cache = cache.borrow_mut();
    for instrument in instruments {
        cache
            .add_instrument(instrument.clone())
            .context("failed to add source replay instrument to cache")?;
    }
    Ok(())
}

fn apply_entry_decision_source_books(
    strategy: &mut BinaryOracleEdgeTaker,
    books: &BinaryOracleEntryBooksSource,
) -> Result<()> {
    apply_entry_decision_source_book(
        &mut strategy.active.books.up,
        &books.up,
        books.price_precision,
    )
    .context("entry decision evidence up book source is invalid")?;
    apply_entry_decision_source_book(
        &mut strategy.active.books.down,
        &books.down,
        books.price_precision,
    )
    .context("entry decision evidence down book source is invalid")?;
    Ok(())
}

fn apply_entry_decision_source_book(
    book: &mut OutcomeBookState,
    source: &BinaryOracleEntryBookSideSource,
    price_precision: u8,
) -> Result<()> {
    let instrument_id = book
        .instrument_id
        .ok_or_else(|| anyhow::anyhow!("entry decision evidence book is missing instrument id"))?;
    anyhow::ensure!(
        is_positive_finite(source.best_bid)
            && is_positive_finite(source.best_ask)
            && is_positive_finite(source.bid_quantity)
            && is_positive_finite(source.ask_quantity)
            && is_positive_finite(source.liquidity_available),
        "entry decision evidence book contains non-positive values"
    );
    anyhow::ensure!(
        source.best_bid <= source.best_ask,
        "entry decision evidence book best_bid exceeds best_ask"
    );
    book.last_observed_instrument_id = Some(instrument_id);
    book.bid_levels.clear();
    book.ask_levels.clear();
    let best_bid = Price::new_checked(source.best_bid, price_precision)
        .map_err(|err| anyhow::anyhow!("entry decision evidence book bid is invalid: {err}"))?;
    let best_ask = Price::new_checked(source.best_ask, price_precision)
        .map_err(|err| anyhow::anyhow!("entry decision evidence book ask is invalid: {err}"))?;
    book.bid_levels.insert(best_bid, source.bid_quantity);
    book.ask_levels.insert(best_ask, source.ask_quantity);
    book.best_bid = Some(source.best_bid);
    book.best_ask = Some(source.best_ask);
    book.liquidity_available = Some(source.liquidity_available);
    Ok(())
}

#[cfg(test)]
mod tests {
    use toml::Value;

    use nautilus_model::identifiers::InstrumentId;

    use crate::bolt_v3_book_sizing::OutcomeBookState;

    use super::{
        BinaryOracleEntryBookSideSource, BinaryOracleReferenceQuoteObservationSource,
        apply_entry_decision_source_book, derive_entry_reference_proofs_from_quote_observations,
    };

    fn source_proof_raw_config() -> Value {
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
            cadence_slug_token = "configuredwindow"
            market_selection_rule = "active_or_next"
            retry_interval_seconds = 5
            blocked_after_seconds = 60
            reference_venue = "reference_data_client"
            reference_instrument_id = "REFERENCE.SOURCE"
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
            risk_lambda = 0.5
            edge_threshold_basis_points = -20
            exit_hysteresis_bps = 5
            vol_window_secs = 60
            vol_gap_reset_secs = 10
            vol_min_observations = 1
            vol_bridge_valid_secs = 10
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
            order_type = "limit"
            time_in_force = "fok"
            is_post_only = false
            is_reduce_only = false
            is_quote_quantity = false

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

    #[test]
    fn reference_proof_derivation_uses_latest_configured_quote_before_decision() {
        let observations = vec![
            BinaryOracleReferenceQuoteObservationSource {
                data_client_id: "reference_data_client",
                instrument_id: "REFERENCE.SOURCE",
                bid_price: 3_299.0,
                ask_price: 3_301.0,
                ts_event_unix_nanos: 2_000_000_000,
            },
            BinaryOracleReferenceQuoteObservationSource {
                data_client_id: "signal_data_client",
                instrument_id: "REFERENCE.SOURCE",
                bid_price: 9_000.0,
                ask_price: 9_002.0,
                ts_event_unix_nanos: 3_600_000_000,
            },
            BinaryOracleReferenceQuoteObservationSource {
                data_client_id: "reference_data_client",
                instrument_id: "REFERENCE.SOURCE",
                bid_price: 3_305.0,
                ask_price: 3_307.0,
                ts_event_unix_nanos: 3_000_000_000,
            },
            BinaryOracleReferenceQuoteObservationSource {
                data_client_id: "reference_data_client",
                instrument_id: "OTHER.SOURCE",
                bid_price: 8_000.0,
                ask_price: 8_002.0,
                ts_event_unix_nanos: 3_500_000_000,
            },
            BinaryOracleReferenceQuoteObservationSource {
                data_client_id: "reference_data_client",
                instrument_id: "REFERENCE.SOURCE",
                bid_price: 9_998.0,
                ask_price: 10_000.0,
                ts_event_unix_nanos: 5_000_000_000,
            },
        ];

        let proofs = derive_entry_reference_proofs_from_quote_observations(
            &source_proof_raw_config(),
            &observations,
            2_000,
            4_000,
        )
        .expect("configured reference observations should derive replay proofs");

        assert_eq!(proofs.reference_quote.venue, "reference_data_client");
        assert_eq!(proofs.reference_quote.price, 3_306.0);
        assert_eq!(proofs.reference_quote.observed_ts_ms, 3_000);
        assert_eq!(proofs.realized_volatility.ready_ts_ms, 3_000);
        assert!(proofs.realized_volatility.value > 0.0);
    }

    #[test]
    fn entry_decision_source_book_rejects_crossed_best_prices() {
        let mut book = OutcomeBookState::from_instrument_id(InstrumentId::from("UP.POLYMARKET"));
        let source = BinaryOracleEntryBookSideSource {
            best_bid: 0.55,
            bid_quantity: 100.0,
            best_ask: 0.54,
            ask_quantity: 100.0,
            liquidity_available: 200.0,
        };

        let error = apply_entry_decision_source_book(&mut book, &source, 2)
            .expect_err("crossed source book should be rejected");

        assert!(
            error
                .to_string()
                .contains("entry decision evidence book best_bid exceeds best_ask"),
            "unexpected error: {error}"
        );
        assert!(book.best_bid.is_none());
        assert!(book.best_ask.is_none());
    }
}
