use std::{path::PathBuf, sync::Arc};

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
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        BoltV3SubmitLifecyclePolicy,
    },
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
    time_in_force: TimeInForce,
    post_only: bool,
    reduce_only: bool,
    quote_quantity: bool,
    order_intent: CanaryProofOrderIntentArtifact,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
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
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.executor_strategy_id.as_str())),
                ..Default::default()
            }),
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
        let Some(top_of_book) = self.submit_time_top_of_book(
            instrument_id,
            self.config.order_intent.order_side,
            observed_book,
        )?
        else {
            return Ok(());
        };
        if !submit_time_book_supports_limit(
            self.config.order_intent.order_side,
            price_decimal,
            self.config.order_intent.quantity,
            top_of_book,
        ) {
            return Ok(());
        }
        let price = Price::from_decimal_dp(price_decimal, instrument.price_precision())
            .context("canary proof order price does not fit selected instrument precision")?;
        let quantity = Quantity::from_decimal_dp(
            self.config.order_intent.quantity,
            instrument.size_precision(),
        )
        .context("canary proof order quantity does not fit selected instrument precision")?;
        let client_order_id = self.core.order_factory().generate_client_order_id();
        let order = self.core.order_factory().limit(
            instrument_id,
            order_side,
            quantity,
            price,
            Some(self.config.time_in_force),
            None,
            Some(self.config.post_only),
            Some(self.config.reduce_only),
            Some(self.config.quote_quantity),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(client_order_id),
        );
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
                client_order_id: order.client_order_id().to_string(),
                instrument_id: order.instrument_id().to_string(),
                notional: self.config.order_intent.notional,
                intent_kind: BoltV3SubmitIntentKind::Entry,
                lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
                canary_proof_claim: Some(CANARY_PROOF_CLAIM.to_string()),
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
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.unsubscribe_book_deltas(self.proof_instrument_id(), None, None);
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
    let executor_strategy_id = StrategyId::from(proof_policy.executor_strategy_id.as_str());
    node.add_strategy(CanaryProofExecutor::new(CanaryProofExecutorConfig {
        executor_strategy_id: proof_policy.executor_strategy_id.clone(),
        execution_client_id: proof_policy.execution_client_id.clone(),
        book_type: proof_policy_book_type_to_nt(proof_policy.book_type),
        time_in_force: proof_policy_time_in_force_to_nt(proof_policy.time_in_force),
        post_only: proof_policy.post_only,
        reduce_only: proof_policy.reduce_only,
        quote_quantity: proof_policy.quote_quantity,
        order_intent,
        decision_evidence,
        submit_admission,
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
    }))
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

    use super::{SubmitTimeTopOfBook, submit_time_book_supports_limit};
    use crate::bolt_v3_canary_proof_policy::CanaryProofOrderSide;

    #[test]
    fn submit_time_book_supports_buy_only_when_top_ask_can_fill_exact_order() {
        assert!(submit_time_book_supports_limit(
            CanaryProofOrderSide::Buy,
            Decimal::new(25, 2),
            Decimal::new(20, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(25, 2),
                available_quantity: Decimal::new(20, 0),
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Buy,
            Decimal::new(25, 2),
            Decimal::new(20, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(26, 2),
                available_quantity: Decimal::new(20, 0),
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Buy,
            Decimal::new(25, 2),
            Decimal::new(20, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(25, 2),
                available_quantity: Decimal::new(1999, 2),
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
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Sell,
            Decimal::new(75, 2),
            Decimal::new(10, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(74, 2),
                available_quantity: Decimal::new(10, 0),
            },
        ));

        assert!(!submit_time_book_supports_limit(
            CanaryProofOrderSide::Sell,
            Decimal::new(75, 2),
            Decimal::new(10, 0),
            SubmitTimeTopOfBook {
                price: Decimal::new(75, 2),
                available_quantity: Decimal::new(999, 2),
            },
        ));
    }
}
