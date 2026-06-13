use anyhow::Result;
use nautilus_core::Params;
use nautilus_model::{
    identifiers::{ClientId, ClientOrderId, PositionId},
    orders::OrderAny,
};
use nautilus_trading::Strategy;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_decision_evidence::{BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence},
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState},
};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderExecutionMode {
    Live,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3OrderExecutionPolicy {
    mode: BoltV3OrderExecutionMode,
}

impl BoltV3OrderExecutionPolicy {
    pub const fn from_mode(mode: BoltV3OrderExecutionMode) -> Self {
        Self { mode }
    }

    pub const fn live() -> Self {
        Self::from_mode(BoltV3OrderExecutionMode::Live)
    }

    pub const fn shadow() -> Self {
        Self::from_mode(BoltV3OrderExecutionMode::Shadow)
    }

    pub const fn mode(self) -> BoltV3OrderExecutionMode {
        self.mode
    }

    pub const fn allows_venue_mutation(self) -> bool {
        matches!(self.mode, BoltV3OrderExecutionMode::Live)
    }

    pub fn route_submit<S>(
        self,
        routing: BoltV3SubmitRoutingRequest<'_>,
        sink: &mut S,
        order: OrderAny,
        context: BoltV3SubmitContext,
    ) -> Result<BoltV3SubmitRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        routing
            .decision_evidence
            .record_order_intent(&routing.intent)?;
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                let _permit = routing.submit_admission.admit(&routing.request)?;
                sink.submit_order_via_nt(order, context)?;
                Ok(BoltV3SubmitRoutingOutcome::Submitted)
            }
            BoltV3OrderExecutionMode::Shadow => {
                routing
                    .submit_admission
                    .evaluate_and_record_without_consuming_capacity(&routing.request)?;
                log::info!(
                    "bolt-v3 submit skipped by execution policy: mode=shadow strategy_id={} client_order_id={}",
                    routing.request.strategy_id,
                    routing.request.client_order_id,
                );
                Ok(BoltV3SubmitRoutingOutcome::SkippedByPolicy)
            }
        }
    }

    pub fn route_cancel<S>(
        self,
        sink: &mut S,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<BoltV3CancelRoutingOutcome>
    where
        S: BoltV3NtVenueMutationSink + ?Sized,
    {
        match self.mode {
            BoltV3OrderExecutionMode::Live => {
                sink.cancel_order_via_nt(client_order_id, client_id, params)?;
                Ok(BoltV3CancelRoutingOutcome::Canceled)
            }
            BoltV3OrderExecutionMode::Shadow => {
                log::info!(
                    "bolt-v3 cancel skipped by execution policy: mode=shadow client_order_id={client_order_id}"
                );
                Ok(BoltV3CancelRoutingOutcome::SkippedByPolicy)
            }
        }
    }
}

pub struct BoltV3SubmitRoutingRequest<'a> {
    decision_evidence: &'a dyn BoltV3DecisionEvidenceWriter,
    submit_admission: &'a BoltV3SubmitAdmissionState,
    intent: BoltV3OrderIntentEvidence,
    request: BoltV3SubmitAdmissionRequest,
}

impl<'a> BoltV3SubmitRoutingRequest<'a> {
    pub fn new(
        decision_evidence: &'a dyn BoltV3DecisionEvidenceWriter,
        submit_admission: &'a BoltV3SubmitAdmissionState,
        intent: BoltV3OrderIntentEvidence,
        request: BoltV3SubmitAdmissionRequest,
    ) -> Self {
        Self {
            decision_evidence,
            submit_admission,
            intent,
            request,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoltV3SubmitContext {
    client_id: Option<ClientId>,
    position_id: Option<PositionId>,
    params: Option<Params>,
}

impl BoltV3SubmitContext {
    pub fn from_parts(
        client_id: Option<ClientId>,
        position_id: Option<PositionId>,
        params: Option<Params>,
    ) -> Self {
        Self {
            client_id,
            position_id,
            params,
        }
    }

    pub fn with_client_id(client_id: ClientId) -> Self {
        Self::from_parts(Some(client_id), None, None)
    }

    pub fn with_client_id_and_position_id(client_id: ClientId, position_id: PositionId) -> Self {
        Self::from_parts(Some(client_id), Some(position_id), None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3SubmitRoutingOutcome {
    Submitted,
    SkippedByPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CancelRoutingOutcome {
    Canceled,
    SkippedByPolicy,
}

pub trait BoltV3NtVenueMutationSink {
    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()>;

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()>;
}

impl<T> BoltV3NtVenueMutationSink for T
where
    T: Strategy,
{
    fn submit_order_via_nt(&mut self, order: OrderAny, context: BoltV3SubmitContext) -> Result<()> {
        self.submit_order(
            order,
            context.position_id,
            context.client_id,
            context.params,
        )
    }

    fn cancel_order_via_nt(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: Option<ClientId>,
        params: Option<Params>,
    ) -> Result<()> {
        self.cancel_order(client_order_id, client_id, params)
    }
}
