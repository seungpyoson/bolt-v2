use std::sync::{Arc, Mutex};

use nautilus_model::{
    data::{IndexPriceUpdate, QuoteTick, TradeTick},
    identifiers::{ClientId, InstrumentId, Venue},
    types::Currency,
};

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
use crate::bolt_v3_current_evidence::{DecisionEvidenceRecorder, StrategyEvidenceHandles};
use crate::{
    bolt_v3_current_evidence::{
        BookingRecoveryFacts, EdgeTakerEvidence, MakerEvidence, OrderExecutionEvidence,
        SettlementRecoveryFacts,
    },
    bolt_v3_operator_health::BoltV3SettlementHealthTransitionEmitter,
    bolt_v3_order_execution::{BoltV3OrderEconomicsHandle, BoltV3OrderExecutionPolicy},
    bolt_v3_realized_volatility::RealizedVolSnapshot,
    bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime,
    bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSinkHandle,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    bolt_v3_timestamp_domain::NtStrategyClockMs,
};

#[derive(Clone)]
pub struct RealizedVolatilityCapability {
    runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
}

impl RealizedVolatilityCapability {
    fn new(runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>) -> Self {
        Self { runtime }
    }
}

#[derive(Clone, Default)]
pub struct SettlementCapability {
    runtime_sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    settlement_recovery: Option<Arc<SettlementRecoveryFacts>>,
    booking_recovery: Option<Arc<BookingRecoveryFacts>>,
    account_id: Option<String>,
    currency: Option<Currency>,
    health_transition_emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
}

impl SettlementCapability {
    pub fn runtime_sink(&self) -> Option<BoltV3SettlementRuntimeSinkHandle> {
        self.runtime_sink.clone()
    }

    pub fn settlement_recovery(&self) -> Option<&SettlementRecoveryFacts> {
        self.settlement_recovery.as_deref()
    }

    pub fn booking_recovery(&self) -> Option<&BookingRecoveryFacts> {
        self.booking_recovery.as_deref()
    }

    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    pub fn currency(&self) -> Option<Currency> {
        self.currency
    }

    pub fn health_transition_emitter(&self) -> Option<&BoltV3SettlementHealthTransitionEmitter> {
        self.health_transition_emitter.as_ref()
    }
}

#[derive(Clone)]
pub struct StrategyBuildContext {
    order_economics: BoltV3OrderEconomicsHandle,
    decision_evidence: StrategyDecisionEvidence,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    order_execution_policy: BoltV3OrderExecutionPolicy,
    execution_venue: Venue,
    realized_volatility: Option<RealizedVolatilityCapability>,
    settlement: Option<SettlementCapability>,
}

pub enum StrategyDecisionEvidence {
    EdgeTaker {
        evidence: EdgeTakerEvidence,
        order_execution: OrderExecutionEvidence,
    },
    Maker {
        evidence: MakerEvidence,
        order_execution: OrderExecutionEvidence,
    },
    None,
}

impl Clone for StrategyDecisionEvidence {
    fn clone(&self) -> Self {
        match self {
            Self::EdgeTaker {
                evidence,
                order_execution,
            } => Self::EdgeTaker {
                evidence: evidence.reissue(),
                order_execution: order_execution.reissue(),
            },
            Self::Maker {
                evidence,
                order_execution,
            } => Self::Maker {
                evidence: evidence.reissue(),
                order_execution: order_execution.reissue(),
            },
            Self::None => Self::None,
        }
    }
}

impl StrategyDecisionEvidence {
    pub fn edge_taker(
        evidence: EdgeTakerEvidence,
        order_execution: OrderExecutionEvidence,
    ) -> Self {
        Self::EdgeTaker {
            evidence,
            order_execution,
        }
    }

    pub fn maker(evidence: MakerEvidence, order_execution: OrderExecutionEvidence) -> Self {
        Self::Maker {
            evidence,
            order_execution,
        }
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    pub fn maker_for_test(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        let handles = StrategyEvidenceHandles::from(recorder);
        Self::maker(handles.maker(), handles.order_execution())
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl From<Arc<DecisionEvidenceRecorder>> for StrategyDecisionEvidence {
    fn from(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        let handles = StrategyEvidenceHandles::from(recorder);
        Self::edge_taker(handles.edge_taker(), handles.order_execution())
    }
}

impl StrategyBuildContext {
    /// `execution_venue` is the venue of the strategy's configured execution client
    /// (`root.clients[execution_client_id].venue`). It is a REQUIRED constructor argument — not an
    /// optional builder field — so a production build can never forget to scope market selection to
    /// the venue that orders actually route to. The strategy uses it to fail closed unless the
    /// selected market's venue equals this one (a wrong-venue selection from the shared NT cache
    /// would otherwise be possible once a second venue's instruments coexist in the cache).
    pub fn new(
        order_economics: BoltV3OrderEconomicsHandle,
        decision_evidence: impl Into<StrategyDecisionEvidence>,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        order_execution_policy: BoltV3OrderExecutionPolicy,
        execution_venue: Venue,
    ) -> Self {
        Self {
            order_economics,
            decision_evidence: decision_evidence.into(),
            submit_admission,
            order_execution_policy,
            execution_venue,
            realized_volatility: None,
            settlement: None,
        }
    }

    pub fn order_economics(&self) -> &BoltV3OrderEconomicsHandle {
        &self.order_economics
    }

    #[cfg(test)]
    pub fn with_realized_volatility_surfaces(
        self,
        surfaces: std::collections::BTreeMap<
            String,
            crate::bolt_v3_realized_volatility::RealizedVolEngineConfig,
        >,
    ) -> Self {
        self.with_realized_volatility_runtime(Arc::new(Mutex::new(
            RealizedVolSurfaceRuntime::from_configs(surfaces)
                .expect("validated realized-volatility surfaces should build runtime"),
        )))
    }

    pub fn with_realized_volatility_runtime(
        mut self,
        runtime: Arc<Mutex<RealizedVolSurfaceRuntime>>,
    ) -> Self {
        self.realized_volatility = Some(RealizedVolatilityCapability::new(runtime));
        self
    }

    pub fn with_settlement_runtime_sink(
        mut self,
        sink: Option<BoltV3SettlementRuntimeSinkHandle>,
    ) -> Self {
        self.settlement.get_or_insert_default().runtime_sink = sink;
        self
    }

    pub fn with_settlement_recovery(
        mut self,
        recovery: Option<Arc<SettlementRecoveryFacts>>,
    ) -> Self {
        self.settlement.get_or_insert_default().settlement_recovery = recovery;
        self
    }

    pub fn with_booking_recovery(mut self, recovery: Option<Arc<BookingRecoveryFacts>>) -> Self {
        self.settlement.get_or_insert_default().booking_recovery = recovery;
        self
    }

    pub fn with_settlement_account_id(mut self, account_id: Option<String>) -> Self {
        self.settlement.get_or_insert_default().account_id = account_id;
        self
    }

    pub fn with_settlement_currency(mut self, currency: Option<Currency>) -> Self {
        self.settlement.get_or_insert_default().currency = currency;
        self
    }

    pub fn with_settlement_health_transition_emitter(
        mut self,
        emitter: Option<BoltV3SettlementHealthTransitionEmitter>,
    ) -> Self {
        self.settlement
            .get_or_insert_default()
            .health_transition_emitter = emitter;
        self
    }

    pub(crate) fn edge_taker_evidence(&self) -> Option<EdgeTakerEvidence> {
        match &self.decision_evidence {
            StrategyDecisionEvidence::EdgeTaker { evidence, .. } => Some(evidence.reissue()),
            StrategyDecisionEvidence::Maker { .. } | StrategyDecisionEvidence::None => None,
        }
    }

    pub(crate) fn maker_evidence(&self) -> Option<MakerEvidence> {
        match &self.decision_evidence {
            StrategyDecisionEvidence::Maker { evidence, .. } => Some(evidence.reissue()),
            StrategyDecisionEvidence::EdgeTaker { .. } | StrategyDecisionEvidence::None => None,
        }
    }

    pub(crate) fn order_execution_evidence(&self) -> Option<OrderExecutionEvidence> {
        match &self.decision_evidence {
            StrategyDecisionEvidence::EdgeTaker {
                order_execution, ..
            }
            | StrategyDecisionEvidence::Maker {
                order_execution, ..
            } => Some(order_execution.reissue()),
            StrategyDecisionEvidence::None => None,
        }
    }

    pub fn submit_admission(&self) -> &BoltV3SubmitAdmissionState {
        self.submit_admission.as_ref()
    }

    pub fn submit_admission_arc(&self) -> Arc<BoltV3SubmitAdmissionState> {
        self.submit_admission.clone()
    }

    pub fn order_execution_policy(&self) -> BoltV3OrderExecutionPolicy {
        self.order_execution_policy
    }

    #[cfg(test)]
    pub fn with_order_execution_policy(mut self, policy: BoltV3OrderExecutionPolicy) -> Self {
        self.order_execution_policy = policy;
        self
    }

    /// Venue of the configured execution client. Market selection must be scoped to this venue so a
    /// real order can only ever fire against an instrument on the venue it routes to.
    pub fn execution_venue(&self) -> Venue {
        self.execution_venue
    }

    pub fn realized_volatility_capability(&self) -> Option<&RealizedVolatilityCapability> {
        self.realized_volatility.as_ref()
    }

    pub fn settlement_capability(&self) -> Option<&SettlementCapability> {
        self.settlement.as_ref()
    }

    pub fn settlement_runtime_sink(&self) -> Option<BoltV3SettlementRuntimeSinkHandle> {
        self.settlement
            .as_ref()
            .and_then(|capability| capability.runtime_sink.clone())
    }

    pub fn settlement_recovery(&self) -> Option<&SettlementRecoveryFacts> {
        self.settlement
            .as_ref()
            .and_then(|capability| capability.settlement_recovery.as_deref())
    }

    pub fn booking_recovery(&self) -> Option<&BookingRecoveryFacts> {
        self.settlement
            .as_ref()
            .and_then(|capability| capability.booking_recovery.as_deref())
    }

    pub fn settlement_account_id(&self) -> Option<&str> {
        self.settlement
            .as_ref()
            .and_then(|capability| capability.account_id.as_deref())
    }

    pub fn settlement_currency(&self) -> Option<Currency> {
        self.settlement
            .as_ref()
            .and_then(|capability| capability.currency)
    }

    pub fn settlement_health_transition_emitter(
        &self,
    ) -> Option<&BoltV3SettlementHealthTransitionEmitter> {
        self.settlement
            .as_ref()
            .and_then(|capability| capability.health_transition_emitter.as_ref())
    }

    /// Subscription requests scoped to a single configured surface. A strategy must use this
    /// (with its configured `realized_volatility_surface_id`) so it only subscribes the RV
    /// feeds it prices against, even when the root config defines many surfaces.
    pub fn realized_volatility_quote_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        let Some(capability) = self.realized_volatility.as_ref() else {
            return Vec::new();
        };
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .quote_subscription_requests_for_surface(surface_id)
    }

    pub fn realized_volatility_trade_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        let Some(capability) = self.realized_volatility.as_ref() else {
            return Vec::new();
        };
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .trade_subscription_requests_for_surface(surface_id)
    }

    pub fn realized_volatility_index_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        let Some(capability) = self.realized_volatility.as_ref() else {
            return Vec::new();
        };
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .index_subscription_requests_for_surface(surface_id)
    }

    pub fn observe_realized_volatility_quote(&self, quote: &QuoteTick) -> Vec<RealizedVolSnapshot> {
        let Some(capability) = self.realized_volatility.as_ref() else {
            return Vec::new();
        };
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .observe_quote(quote)
    }

    pub fn observe_realized_volatility_trade(&self, trade: &TradeTick) -> Vec<RealizedVolSnapshot> {
        let Some(capability) = self.realized_volatility.as_ref() else {
            return Vec::new();
        };
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .observe_trade(trade)
    }

    pub fn observe_realized_volatility_index_price(
        &self,
        update: &IndexPriceUpdate,
    ) -> Vec<RealizedVolSnapshot> {
        let Some(capability) = self.realized_volatility.as_ref() else {
            return Vec::new();
        };
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .observe_index_price(update)
    }

    pub fn refresh_realized_volatility_snapshot_at(
        &self,
        surface_id: &str,
        now_ms: u64,
    ) -> Option<RealizedVolSnapshot> {
        let capability = self.realized_volatility.as_ref()?;
        capability
            .runtime
            .lock()
            .expect("realized-volatility runtime lock should not be poisoned")
            .refresh_surface_at(surface_id, NtStrategyClockMs::new(now_ms))
    }
}
