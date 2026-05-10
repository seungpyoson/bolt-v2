use std::{
    sync::{Arc, Mutex},
    thread,
};

use anyhow::{Context, Result, anyhow};
use nautilus_core::UnixNanos;

use crate::{
    bolt_v3_config::PersistenceBlock,
    bolt_v3_decision_event_context::BoltV3DecisionEventCommonContext,
    bolt_v3_decision_events::{
        BoltV3DecisionEventCatalogHandoff, BoltV3EntryEvaluationDecisionEvent,
        BoltV3EntryEvaluationFacts, BoltV3EntryOrderSubmissionDecisionEvent,
        BoltV3ExitOrderSubmissionDecisionEvent, BoltV3OrderSubmissionFacts,
    },
};

#[derive(Clone)]
pub struct BoltV3StrategyDecisionEvidence {
    common_context: BoltV3DecisionEventCommonContext,
    handoff: Arc<Mutex<BoltV3DecisionEventCatalogHandoff>>,
}

impl BoltV3StrategyDecisionEvidence {
    pub fn from_persistence_block(
        common_context: BoltV3DecisionEventCommonContext,
        persistence: &PersistenceBlock,
    ) -> Result<Self> {
        let handoff = BoltV3DecisionEventCatalogHandoff::from_persistence_block(persistence)?;
        Ok(Self {
            common_context,
            handoff: Arc::new(Mutex::new(handoff)),
        })
    }

    pub fn gate_entry_order_submission<T>(
        &self,
        decision_trace_id: &str,
        facts: BoltV3OrderSubmissionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
        submit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let common = self.common_context.common_fields(decision_trace_id)?;
        let event = BoltV3EntryOrderSubmissionDecisionEvent::entry_order_submission(
            common, facts, ts_event, ts_init,
        )?;
        self.write_entry_order_submission(event)
            .context("bolt-v3 entry order-intent handoff failed")?;
        submit()
    }

    pub fn write_entry_evaluation(
        &self,
        decision_trace_id: &str,
        facts: BoltV3EntryEvaluationFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<()> {
        let common = self.common_context.common_fields(decision_trace_id)?;
        let event =
            BoltV3EntryEvaluationDecisionEvent::entry_evaluation(common, facts, ts_event, ts_init)?;
        self.write_entry_evaluation_event(event)
            .context("bolt-v3 entry evaluation handoff failed")
    }

    pub fn gate_exit_order_submission<T>(
        &self,
        decision_trace_id: &str,
        facts: BoltV3OrderSubmissionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
        submit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let common = self.common_context.common_fields(decision_trace_id)?;
        let event = BoltV3ExitOrderSubmissionDecisionEvent::exit_order_submission(
            common, facts, ts_event, ts_init,
        )?;
        self.write_exit_order_submission(event)
            .context("bolt-v3 exit order-intent handoff failed")?;
        submit()
    }

    fn write_entry_order_submission(
        &self,
        event: BoltV3EntryOrderSubmissionDecisionEvent,
    ) -> Result<()> {
        let handoff = Arc::clone(&self.handoff);
        thread::spawn(move || {
            let mut handoff = handoff
                .lock()
                .map_err(|_| anyhow!("bolt-v3 decision-evidence handoff mutex poisoned"))?;
            handoff.write_entry_order_submission(event)
        })
        .join()
        .map_err(|_| anyhow!("bolt-v3 entry order-intent handoff thread panicked"))?
    }

    fn write_entry_evaluation_event(
        &self,
        event: BoltV3EntryEvaluationDecisionEvent,
    ) -> Result<()> {
        let handoff = Arc::clone(&self.handoff);
        thread::spawn(move || {
            let mut handoff = handoff
                .lock()
                .map_err(|_| anyhow!("bolt-v3 decision-evidence handoff mutex poisoned"))?;
            handoff.write_entry_evaluation(event)
        })
        .join()
        .map_err(|_| anyhow!("bolt-v3 entry evaluation handoff thread panicked"))?
    }

    fn write_exit_order_submission(
        &self,
        event: BoltV3ExitOrderSubmissionDecisionEvent,
    ) -> Result<()> {
        let handoff = Arc::clone(&self.handoff);
        thread::spawn(move || {
            let mut handoff = handoff
                .lock()
                .map_err(|_| anyhow!("bolt-v3 decision-evidence handoff mutex poisoned"))?;
            handoff.write_exit_order_submission(event)
        })
        .join()
        .map_err(|_| anyhow!("bolt-v3 exit order-intent handoff thread panicked"))?
    }
}
