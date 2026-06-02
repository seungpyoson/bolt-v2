use std::{num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::OrderBookDeltas,
    enums::{BookType, OrderSide, TimeInForce},
    identifiers::{ClientId, InstrumentId, StrategyId},
    instruments::Instrument,
    orderbook::OrderBook,
    orders::Order,
    types::{Price, Quantity},
};
use nautilus_trading::{Strategy, StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_canary_proof_policy::{
        CANARY_PROOF_CLAIM, CANARY_PROOF_ORDER_INTENT_RECORD_KIND, CanaryProofOrderIntentArtifact,
        CanaryProofOrderSide,
    },
    bolt_v3_config::{
        DataClientReadinessProbeBookType, LiveCanaryOperatorEvidenceBlock,
        LiveCanaryProofPolicyBlock, LiveCanaryProofTimeInForce, LoadedBoltV3Config,
    },
    bolt_v3_decision_evidence::{
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
    },
    bolt_v3_operator_artifacts::{EntryReadinessGateSession, read_file_bounded},
    bolt_v3_providers::resolve_fee_provider,
    bolt_v3_secrets::ResolvedBoltV3Secrets,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        BoltV3SubmitLifecyclePolicy, admission_base_notional_from_order,
        rounded_order_admission_notional,
    },
    strategies::registry::FeeProvider,
};

const OPERATOR_EVIDENCE_CANARY_PROOF_ORDER_INTENT_PATH_FIELD: &str =
    "canary_proof_order_intent_path";
const OPERATOR_EVIDENCE_CANARY_PROOF_ORDER_INTENT_SHA256_FIELD: &str =
    "canary_proof_order_intent_sha256";

#[derive(Clone)]
struct CanaryProofExecutorConfig {
    executor_strategy_id: String,
    execution_client_id: String,
    book_type: BookType,
    book_snapshot_interval_millis: NonZeroUsize,
    time_in_force: TimeInForce,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
    order_intent: CanaryProofOrderIntentArtifact,
    fee_provider: Arc<dyn FeeProvider>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    // Maximum age, in nanoseconds, the submit-time top-of-book event may carry
    // and still be admitted. Sourced from the SAME config-owned canary
    // freshness bound (`live_canary.reference_quote_max_age_seconds`) the gate
    // validates at startup — one freshness policy, no second source of truth.
    submit_time_book_max_age_nanos: u128,
}

pub struct CanaryProofExecutor {
    core: StrategyCore,
    config: CanaryProofExecutorConfig,
    submitted: bool,
}

impl std::fmt::Debug for CanaryProofExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanaryProofExecutor")
            .field("executor_strategy_id", &self.config.executor_strategy_id)
            .field("execution_client_id", &self.config.execution_client_id)
            .field("instrument_id", &self.config.order_intent.instrument_id)
            .field("submitted", &self.submitted)
            .finish()
    }
}

impl CanaryProofExecutor {
    fn new(config: CanaryProofExecutorConfig) -> Self {
        Self {
            core: StrategyCore::new(
                StrategyConfig::builder()
                    .strategy_id(StrategyId::from(config.executor_strategy_id.as_str()))
                    .build(),
            ),
            config,
            submitted: false,
        }
    }

    fn proof_instrument_id(&self) -> InstrumentId {
        InstrumentId::from(self.config.order_intent.instrument_id.as_str())
    }

    fn try_submit_proof_order(&mut self, observed_book: Option<&OrderBook>) -> Result<()> {
        if self.submitted {
            return Ok(());
        }

        let instrument_id = self.proof_instrument_id();
        let Some(instrument) = self.cache().instrument(&instrument_id).cloned() else {
            return Ok(());
        };
        let order_side = match self.config.order_intent.order_side {
            CanaryProofOrderSide::Buy => OrderSide::Buy,
            CanaryProofOrderSide::Sell => OrderSide::Sell,
        };
        let price_decimal = self.config.order_intent.notional / self.config.order_intent.quantity;
        // Build the venue-precision Price/Quantity FIRST so every downstream
        // guard evaluates the exact order handed to the venue. Banker's rounding
        // to instrument precision can round a value UP, so the unrounded intent
        // is not a safe proxy for the submitted order's notional or size.
        let price = Price::from_decimal_dp(price_decimal, instrument.price_precision())
            .context("canary proof order price does not fit selected instrument precision")?;
        let quantity = Quantity::from_decimal_dp(
            self.config.order_intent.quantity,
            instrument.size_precision(),
        )
        .context("canary proof order quantity does not fit selected instrument precision")?;
        let rounded_price_decimal = price.as_decimal();
        let rounded_quantity_decimal = quantity.as_decimal();
        let max_fee_bps = self
            .config
            .fee_provider
            .max_entry_fee_bps(&instrument, rounded_price_decimal)
            .context("canary proof submit admission requires a max entry fee bound")?;
        anyhow::ensure!(
            max_fee_bps >= Decimal::ZERO,
            "canary proof submit admission max entry fee bound must be non-negative"
        );
        let Some(top_of_book) = self.submit_time_top_of_book(
            instrument_id,
            self.config.order_intent.order_side,
            observed_book,
        )?
        else {
            return Ok(());
        };
        // Bind the submit-time book to its event timestamp and reject if the
        // top-of-book is stale relative to the gate-approved freshness bound.
        // Fail CLOSED: a missing/zero event timestamp, a future timestamp
        // (clock skew), or an age beyond the bound all suppress the submit —
        // identical liveness-only suppression as the thin-book guard below.
        let now_nanos = self.clock().timestamp_ns().as_u64();
        if !submit_time_book_is_fresh(
            top_of_book,
            now_nanos,
            self.config.submit_time_book_max_age_nanos,
        ) {
            return Ok(());
        }
        // Evaluate the book/liquidity guard against the ROUNDED order so a
        // rounded-up quantity cannot pass a guard sized for the unrounded intent.
        if !submit_time_book_supports_limit(
            self.config.order_intent.order_side,
            rounded_price_decimal,
            rounded_quantity_decimal,
            top_of_book,
        ) {
            return Ok(());
        }
        let client_order_id = self.core.order_factory().generate_client_order_id();
        let order = self.core.order_factory().limit(
            instrument_id,
            order_side,
            quantity,
            price,
            Some(self.config.time_in_force),
            None,
            Some(self.config.is_post_only),
            Some(self.config.is_reduce_only),
            Some(self.config.is_quote_quantity),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(client_order_id),
        );
        // Single source of truth: derive the admission BASE notional from the
        // BUILT order through the SAME shared helper the production strategy uses
        // (`admission_base_notional_from_order`). For a quote-quantity order this
        // sizes by quote semantics instead of the price*quantity shortcut, which
        // understates the real cash debit. The side-appropriate top-of-book price
        // (best ask for a BUY, best bid for a SELL) is the conservative reference
        // the shared helper pulls the effective price toward.
        let quote_reference_price =
            Price::from_decimal_dp(top_of_book.price, instrument.price_precision()).context(
                "canary proof submit-time book price does not fit selected instrument precision",
            )?;
        let base_notional = admission_base_notional_from_order(
            &order,
            &instrument,
            rounded_price_decimal,
            rounded_quantity_decimal,
            Some(price),
            Some(quote_reference_price),
        )
        .context("canary proof submit admission could not size the built order's notional")?;
        // Admission sees the rounded order's notional and fails CLOSED if rounding
        // grew the order past the operator-approved intended notional
        // (cap-bypass-via-rounding guard).
        let admission_notional = rounded_order_admission_notional(
            base_notional,
            self.config.order_intent.notional,
            max_fee_bps,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        let mut intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.config.executor_strategy_id.clone(),
            BoltV3OrderIntentKind::Entry,
            price.to_string(),
            &order,
        );
        intent.canary_proof_claim = Some(CANARY_PROOF_CLAIM.to_string());
        self.config.decision_evidence.record_order_intent(&intent)?;
        self.config
            .submit_admission
            .admit(&BoltV3SubmitAdmissionRequest {
                strategy_id: self.config.executor_strategy_id.clone(),
                execution_client_id: self.config.execution_client_id.clone(),
                client_order_id: order.client_order_id().to_string(),
                instrument_id: order.instrument_id().to_string(),
                notional: admission_notional,
                order_side: order.order_side(),
                order_quantity: rounded_quantity_decimal,
                intent_kind: BoltV3SubmitIntentKind::Entry,
                lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
                canary_proof_claim: Some(CANARY_PROOF_CLAIM.to_string()),
                risk_reducing_exit_proof: None,
            })?;
        self.submit_order(
            order,
            None,
            Some(ClientId::from(self.config.execution_client_id.as_str())),
            None,
        )?;
        self.submitted = true;
        Ok(())
    }

    fn submit_time_top_of_book(
        &self,
        instrument_id: InstrumentId,
        order_side: CanaryProofOrderSide,
        observed_book: Option<&OrderBook>,
    ) -> Result<Option<SubmitTimeTopOfBook>> {
        let cache = self.cache();
        let book =
            if let Some(book) = observed_book.filter(|book| book.instrument_id == instrument_id) {
                book
            } else if let Some(book) = cache.order_book(&instrument_id) {
                book
            } else {
                return Ok(None);
            };
        submit_time_top_of_book(book, order_side)
    }
}

impl DataActor for CanaryProofExecutor {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_book_deltas(
            self.proof_instrument_id(),
            self.config.book_type,
            None,
            None,
            false,
            None,
        );
        self.subscribe_book_at_interval(
            self.proof_instrument_id(),
            self.config.book_type,
            None,
            self.config.book_snapshot_interval_millis,
            None,
            None,
        );
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.unsubscribe_book_deltas(self.proof_instrument_id(), None, None);
        self.unsubscribe_book_at_interval(
            self.proof_instrument_id(),
            self.config.book_snapshot_interval_millis,
            None,
            None,
        );
        Ok(())
    }

    fn on_book_deltas(&mut self, deltas: &OrderBookDeltas) -> Result<()> {
        if deltas.instrument_id == self.proof_instrument_id() {
            self.try_submit_proof_order(None)?;
        }
        Ok(())
    }

    fn on_book(&mut self, order_book: &OrderBook) -> Result<()> {
        if order_book.instrument_id == self.proof_instrument_id() {
            self.try_submit_proof_order(Some(order_book))?;
        }
        Ok(())
    }
}

nautilus_strategy!(CanaryProofExecutor);

pub fn register_canary_proof_executor_on_node(
    node: &mut nautilus_live::node::LiveNode,
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
) -> Result<Option<StrategyId>> {
    let Some(live_canary) = loaded.root.live_canary.as_ref() else {
        return Ok(None);
    };
    let Some(proof_policy) = live_canary
        .proof_policy
        .as_ref()
        .filter(|proof_policy| proof_policy.enabled)
    else {
        return Ok(None);
    };
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .context("live canary proof executor requires `[live_canary.operator_evidence]`")?;
    let gate_session = load_gate_session(loaded, operator_evidence)?;
    let order_intent = load_canary_proof_order_intent(loaded, operator_evidence)?;
    validate_canary_proof_order_intent(proof_policy, &gate_session, &order_intent)?;
    let fee_provider =
        resolve_fee_provider(loaded, proof_policy.execution_client_id.as_str(), resolved)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    let executor_strategy_id = StrategyId::from(proof_policy.executor_strategy_id.as_str());
    // Single source of truth for canary freshness: the gate already validates
    // `reference_quote_max_age_seconds` at startup, so the submit-time book
    // staleness bound reuses that same config-owned value (GROUP-BY-CHANGE).
    let submit_time_book_max_age_nanos =
        Duration::from_secs(live_canary.reference_quote_max_age_seconds).as_nanos();
    node.add_strategy(CanaryProofExecutor::new(CanaryProofExecutorConfig {
        executor_strategy_id: proof_policy.executor_strategy_id.clone(),
        execution_client_id: proof_policy.execution_client_id.clone(),
        book_type: proof_policy_book_type_to_nt(proof_policy.book_type),
        book_snapshot_interval_millis: proof_policy_book_snapshot_interval_to_nt(
            proof_policy.book_snapshot_interval_millis,
        )?,
        time_in_force: proof_policy_time_in_force_to_nt(proof_policy.time_in_force),
        is_post_only: proof_policy.is_post_only,
        is_reduce_only: proof_policy.is_reduce_only,
        is_quote_quantity: proof_policy.is_quote_quantity,
        order_intent,
        fee_provider,
        decision_evidence,
        submit_admission,
        submit_time_book_max_age_nanos,
    }))?;
    Ok(Some(executor_strategy_id))
}

fn load_gate_session(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<EntryReadinessGateSession> {
    let gate_session_path = operator_evidence
        .gate_session_path
        .as_deref()
        .context("live canary proof executor requires gate_session_path")?;
    let expected_sha256 = operator_evidence
        .expected_gate_session_sha256
        .as_deref()
        .context("live canary proof executor requires expected_gate_session_sha256")?;
    let path = resolve_loaded_config_path(loaded, gate_session_path);
    let bytes = read_file_bounded(&path, operator_evidence.max_operator_evidence_file_bytes)?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    anyhow::ensure!(
        actual_sha256 == expected_sha256,
        "expected_gate_session_sha256 does not match gate_session_path"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn load_canary_proof_order_intent(
    loaded: &LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<CanaryProofOrderIntentArtifact> {
    let path = operator_evidence
        .canary_proof_order_intent_path
        .as_deref()
        .with_context(|| {
            format!(
                "live canary proof executor requires {OPERATOR_EVIDENCE_CANARY_PROOF_ORDER_INTENT_PATH_FIELD}"
            )
        })?;
    let expected_sha256 = operator_evidence
        .canary_proof_order_intent_sha256
        .as_deref()
        .with_context(|| {
            format!(
                "live canary proof executor requires {OPERATOR_EVIDENCE_CANARY_PROOF_ORDER_INTENT_SHA256_FIELD}"
            )
        })?;
    let path = resolve_loaded_config_path(loaded, path);
    let bytes = read_file_bounded(&path, operator_evidence.max_operator_evidence_file_bytes)?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    anyhow::ensure!(
        actual_sha256 == expected_sha256,
        "{OPERATOR_EVIDENCE_CANARY_PROOF_ORDER_INTENT_SHA256_FIELD} does not match {OPERATOR_EVIDENCE_CANARY_PROOF_ORDER_INTENT_PATH_FIELD}"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate_canary_proof_order_intent(
    proof_policy: &LiveCanaryProofPolicyBlock,
    session: &EntryReadinessGateSession,
    artifact: &CanaryProofOrderIntentArtifact,
) -> Result<()> {
    anyhow::ensure!(
        artifact.record_kind == CANARY_PROOF_ORDER_INTENT_RECORD_KIND,
        "canary proof order intent record_kind is invalid"
    );
    anyhow::ensure!(
        artifact.proof_claim == CANARY_PROOF_CLAIM
            && artifact.proof_claim == proof_policy.proof_claim,
        "canary proof order intent proof_claim does not match live canary proof policy"
    );
    anyhow::ensure!(
        artifact.strategy_instance_id == session.strategy_instance_id
            && artifact.strategy_instance_id == proof_policy.strategy_instance_id,
        "canary proof order intent strategy_instance_id does not match gate session"
    );
    anyhow::ensure!(
        artifact.execution_client_id == proof_policy.execution_client_id,
        "canary proof order intent execution_client_id does not match live canary proof policy"
    );
    anyhow::ensure!(
        session
            .selected_market
            .instrument_ids
            .contains(&artifact.instrument_id),
        "canary proof order intent instrument_id is outside selected market"
    );
    anyhow::ensure!(
        artifact.source_refs.contains(&session.session_hash),
        "canary proof order intent source_refs does not include gate session hash"
    );
    anyhow::ensure!(
        artifact.notional > Decimal::ZERO && artifact.quantity > Decimal::ZERO,
        "canary proof order intent notional and quantity must be positive"
    );
    Ok(())
}

fn proof_policy_book_type_to_nt(book_type: DataClientReadinessProbeBookType) -> BookType {
    match book_type {
        DataClientReadinessProbeBookType::L1Mbp => BookType::L1_MBP,
        DataClientReadinessProbeBookType::L2Mbp => BookType::L2_MBP,
        DataClientReadinessProbeBookType::L3Mbo => BookType::L3_MBO,
    }
}

fn proof_policy_book_snapshot_interval_to_nt(interval_millis: u64) -> Result<NonZeroUsize> {
    let interval = usize::try_from(interval_millis)
        .context("live canary proof policy book_snapshot_interval_millis does not fit usize")?;
    NonZeroUsize::new(interval)
        .context("live canary proof policy book_snapshot_interval_millis must be positive")
}

fn proof_policy_time_in_force_to_nt(time_in_force: LiveCanaryProofTimeInForce) -> TimeInForce {
    match time_in_force {
        LiveCanaryProofTimeInForce::Fok => TimeInForce::Fok,
        LiveCanaryProofTimeInForce::Gtc => TimeInForce::Gtc,
        LiveCanaryProofTimeInForce::Ioc => TimeInForce::Ioc,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SubmitTimeTopOfBook {
    price: Decimal,
    available_quantity: Decimal,
    // UNIX nanoseconds of the last event applied to the book this top-of-book
    // was read from. Carried so the submit path can reject a stale book.
    ts_event_nanos: u64,
}

fn submit_time_top_of_book(
    book: &OrderBook,
    order_side: CanaryProofOrderSide,
) -> Result<Option<SubmitTimeTopOfBook>> {
    let top = match order_side {
        CanaryProofOrderSide::Buy => book.best_ask_price().zip(book.best_ask_size()),
        CanaryProofOrderSide::Sell => book.best_bid_price().zip(book.best_bid_size()),
    };
    let Some((price, quantity)) = top else {
        return Ok(None);
    };
    Ok(Some(SubmitTimeTopOfBook {
        price: decimal_from_display(price, "canary proof submit-time book price")?,
        available_quantity: decimal_from_display(
            quantity,
            "canary proof submit-time book quantity",
        )?,
        ts_event_nanos: book.ts_last.as_u64(),
    }))
}

/// Returns `true` only when the submit-time top-of-book event is fresh enough
/// to act on. Fail-closed: a zero/missing event timestamp and a future event
/// timestamp (clock skew) are both treated as stale, and the event age must not
/// exceed `max_age_nanos`.
fn submit_time_book_is_fresh(
    top_of_book: SubmitTimeTopOfBook,
    now_nanos: u64,
    max_age_nanos: u128,
) -> bool {
    if top_of_book.ts_event_nanos == 0 {
        return false;
    }
    let Some(age_nanos) = now_nanos.checked_sub(top_of_book.ts_event_nanos) else {
        return false;
    };
    u128::from(age_nanos) <= max_age_nanos
}

fn submit_time_book_supports_limit(
    order_side: CanaryProofOrderSide,
    limit_price: Decimal,
    quantity: Decimal,
    top_of_book: SubmitTimeTopOfBook,
) -> bool {
    if top_of_book.available_quantity < quantity {
        return false;
    }
    match order_side {
        CanaryProofOrderSide::Buy => top_of_book.price <= limit_price,
        CanaryProofOrderSide::Sell => top_of_book.price >= limit_price,
    }
}

fn decimal_from_display<T>(value: T, label: &str) -> Result<Decimal>
where
    T: std::fmt::Display,
{
    Decimal::from_str_exact(value.to_string().as_str())
        .with_context(|| format!("{label} is not a decimal"))
}

fn resolve_loaded_config_path(loaded: &LoadedBoltV3Config, configured_path: &str) -> PathBuf {
    let path = PathBuf::from(configured_path);
    if path.is_absolute() {
        path
    } else {
        loaded
            .root_path
            .parent()
            .unwrap_or(&loaded.root_path)
            .join(path)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{SubmitTimeTopOfBook, submit_time_book_is_fresh, submit_time_book_supports_limit};
    use crate::bolt_v3_canary_proof_policy::CanaryProofOrderSide;

    fn book_at(ts_event_nanos: u64) -> SubmitTimeTopOfBook {
        SubmitTimeTopOfBook {
            price: Decimal::new(25, 2),
            available_quantity: Decimal::new(20, 0),
            ts_event_nanos,
        }
    }

    #[test]
    fn submit_time_book_is_fresh_only_within_bound_and_fails_closed_on_bad_timestamps() {
        let max_age_nanos = 10_u128;
        // Exactly at the bound is admitted; one nanosecond past is rejected.
        assert!(submit_time_book_is_fresh(book_at(90), 100, max_age_nanos));
        assert!(!submit_time_book_is_fresh(book_at(89), 100, max_age_nanos));
        // A zero/missing event timestamp is treated as stale.
        assert!(!submit_time_book_is_fresh(book_at(0), 100, max_age_nanos));
        // A future event timestamp (clock skew) is treated as stale.
        assert!(!submit_time_book_is_fresh(book_at(101), 100, max_age_nanos));
        // A zero max-age admits only a book stamped at the exact current instant.
        assert!(submit_time_book_is_fresh(book_at(100), 100, 0));
        assert!(!submit_time_book_is_fresh(book_at(99), 100, 0));
    }

    #[test]
    fn submit_time_book_supports_buy_only_when_top_ask_can_fill_exact_order() {
        assert!(submit_time_book_supports_limit(
            CanaryProofOrderSide::Buy,
            Decimal::new(25, 2),
            Decimal::new(20, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(25, 2),
                available_quantity: Decimal::new(20, 0),
                ts_event_nanos: 1,
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Buy,
            Decimal::new(25, 2),
            Decimal::new(20, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(26, 2),
                available_quantity: Decimal::new(20, 0),
                ts_event_nanos: 1,
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Buy,
            Decimal::new(25, 2),
            Decimal::new(20, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(25, 2),
                available_quantity: Decimal::new(1999, 2),
                ts_event_nanos: 1,
            },
        ));
    }

    #[test]
    fn submit_time_book_supports_sell_only_when_top_bid_can_fill_exact_order() {
        assert!(submit_time_book_supports_limit(
            CanaryProofOrderSide::Sell,
            Decimal::new(75, 2),
            Decimal::new(10, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(75, 2),
                available_quantity: Decimal::new(10, 0),
                ts_event_nanos: 1,
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Sell,
            Decimal::new(75, 2),
            Decimal::new(10, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(74, 2),
                available_quantity: Decimal::new(10, 0),
                ts_event_nanos: 1,
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Sell,
            Decimal::new(75, 2),
            Decimal::new(10, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(75, 2),
                available_quantity: Decimal::new(999, 2),
                ts_event_nanos: 1,
            },
        ));
    }

    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{AssetClass, ContingencyType, OrderSide, TimeInForce},
        identifiers::{ClientOrderId, InstrumentId, StrategyId, Symbol, TraderId},
        instruments::{BinaryOption, InstrumentAny},
        orders::{LimitOrder, Order, OrderAny},
        types::{Currency, Price, Quantity},
    };

    use crate::bolt_v3_submit_admission::{
        admission_base_notional_from_order, base_quantity_admission_notional,
    };

    const NANOS_PER_MILLI_U64: u64 = 1_000_000;
    const TEST_BINARY_OPTION_INSTRUMENT_ID: &str = "TESTBINARY-UP.TESTVENUE";

    fn test_binary_option_instrument() -> InstrumentAny {
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(TEST_BINARY_OPTION_INSTRUMENT_ID),
            Symbol::from("TESTBINARY-UP"),
            AssetClass::Alternative,
            Currency::USDC(),
            (1_u64.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            (2_u64.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from("UP")),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            1.into(),
            1.into(),
        ))
    }

    /// Builds a Limit order exactly the way the canary proof executor does:
    /// `is_quote_quantity` toggled by the caller, every other flag fixed. This is
    /// the same order shape `try_submit_proof_order` hands to the shared sizing
    /// helper.
    fn test_canary_limit_order(
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        is_quote_quantity: bool,
    ) -> OrderAny {
        OrderAny::Limit(LimitOrder::new(
            TraderId::from("TESTER-001"),
            StrategyId::from("CANARYPROOF-001"),
            instrument_id,
            ClientOrderId::from("O-CANARYPROOF-0001"),
            order_side,
            quantity,
            price,
            TimeInForce::Fok,
            None,
            false,
            false,
            is_quote_quantity,
            None,
            None,
            None,
            Some(ContingencyType::NoContingency),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            UUID4::new(),
            UnixNanos::default(),
        ))
    }

    #[test]
    fn quote_quantity_buy_limit_sizes_by_quote_semantics_not_price_times_quantity() {
        // The canary proof executor builds a quote-quantity order (it passes
        // `Some(self.config.is_quote_quantity)` to the factory) but USED to size
        // admission as `rounded_price * rounded_quantity`, which UNDERSTATES the
        // real cash debit of a quote-currency-denominated order. It now derives
        // the notional from the built order through the SAME shared helper the
        // production strategy uses, sizing by quote semantics.
        //
        // BUY Limit, quote quantity 25.00 USDC, limit price 0.50, best ask 0.25
        // (the side-appropriate top-of-book the canary supplies as the reference):
        //   effective price = min(0.50, 0.25) = 0.25
        //   base quantity   = 25.00 / 0.25   = 100.00 shares
        //   notional        = 100.00 * 1 * last(0.50) = 50.00 USDC
        // The price*quantity shortcut would yield 0.50 * 25.00 = 12.50 — a 4x
        // understatement. This assertion FAILS on the pre-change inline math.
        let instrument = test_binary_option_instrument();
        let instrument_id = InstrumentId::from(TEST_BINARY_OPTION_INSTRUMENT_ID);
        let quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let best_ask = Price::new(0.25, 2);
        let order =
            test_canary_limit_order(instrument_id, OrderSide::Buy, quantity, limit_price, true);
        assert!(order.is_quote_quantity());

        let base_notional = admission_base_notional_from_order(
            &order,
            &instrument,
            limit_price.as_decimal(),
            quantity.as_decimal(),
            Some(limit_price),
            Some(best_ask),
        )
        .expect("quote-quantity order with a reference price must size");

        let price_times_quantity = limit_price.as_decimal() * quantity.as_decimal();
        assert_eq!(
            price_times_quantity,
            Decimal::from_str_exact("12.50").expect("control decimal should parse"),
            "control: the discredited price*quantity shortcut",
        );
        assert_ne!(
            base_notional, price_times_quantity,
            "quote-quantity sizing must NOT collapse to price*quantity",
        );
        assert_eq!(
            base_notional,
            Decimal::from_str_exact("50.00").expect("expected quote notional should parse"),
            "quote-quantity BUY Limit must size by NT effective quote notional",
        );
    }

    #[test]
    fn base_quantity_buy_limit_sizes_by_price_times_quantity_unchanged() {
        // Invariant guard: base-quantity orders (every shipped config has
        // is_quote_quantity=false) must size EXACTLY as before — price*quantity,
        // byte-identical to the historical canary computation.
        let instrument = test_binary_option_instrument();
        let instrument_id = InstrumentId::from(TEST_BINARY_OPTION_INSTRUMENT_ID);
        let quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let best_ask = Price::new(0.25, 2);
        let order =
            test_canary_limit_order(instrument_id, OrderSide::Buy, quantity, limit_price, false);
        assert!(!order.is_quote_quantity());

        let base_notional = admission_base_notional_from_order(
            &order,
            &instrument,
            limit_price.as_decimal(),
            quantity.as_decimal(),
            Some(limit_price),
            Some(best_ask),
        )
        .expect("base-quantity order must always size");

        assert_eq!(
            base_notional,
            limit_price.as_decimal() * quantity.as_decimal(),
            "base-quantity admission notional must remain price*quantity",
        );
        assert_eq!(
            base_notional,
            base_quantity_admission_notional(limit_price.as_decimal(), quantity.as_decimal()),
            "base-quantity sizing must come from the single shared base definition",
        );
    }

    #[test]
    fn base_quantity_sizing_ignores_reference_prices_so_callers_agree() {
        // Both submit call sites pass their own reference prices, but for a
        // base-quantity order the shared helper must ignore them entirely and
        // return price*quantity — so a strategy call site (which may have no
        // top-of-book) and the canary call site (which always has one) agree on
        // the identical base-quantity notional for identical (price, quantity).
        let instrument = test_binary_option_instrument();
        let instrument_id = InstrumentId::from(TEST_BINARY_OPTION_INSTRUMENT_ID);
        let quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let order =
            test_canary_limit_order(instrument_id, OrderSide::Buy, quantity, limit_price, false);

        let canary_like = admission_base_notional_from_order(
            &order,
            &instrument,
            limit_price.as_decimal(),
            quantity.as_decimal(),
            Some(limit_price),
            Some(Price::new(0.25, 2)),
        )
        .expect("base-quantity order must size with a reference price present");
        let strategy_like = admission_base_notional_from_order(
            &order,
            &instrument,
            limit_price.as_decimal(),
            quantity.as_decimal(),
            None,
            None,
        )
        .expect("base-quantity order must size without any reference price");

        assert_eq!(
            canary_like, strategy_like,
            "base-quantity notional must be independent of reference prices",
        );
    }
}
