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
        BoltV3EntryPreSubmitRejectionDecisionEvent, BoltV3ExitEvaluationDecisionEvent,
        BoltV3ExitEvaluationFacts, BoltV3ExitOrderSubmissionDecisionEvent,
        BoltV3ExitPreSubmitRejectionDecisionEvent, BoltV3OrderSubmissionFacts,
        BoltV3PreSubmitRejectionFacts,
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

    pub fn write_entry_pre_submit_rejection(
        &self,
        decision_trace_id: &str,
        facts: BoltV3PreSubmitRejectionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<()> {
        let common = self.common_context.common_fields(decision_trace_id)?;
        let event = BoltV3EntryPreSubmitRejectionDecisionEvent::entry_pre_submit_rejection(
            common, facts, ts_event, ts_init,
        )?;
        self.write_entry_pre_submit_rejection_event(event)
            .context("bolt-v3 entry pre-submit rejection handoff failed")
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

    pub fn write_exit_evaluation(
        &self,
        decision_trace_id: &str,
        facts: BoltV3ExitEvaluationFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<()> {
        let common = self.common_context.common_fields(decision_trace_id)?;
        let event =
            BoltV3ExitEvaluationDecisionEvent::exit_evaluation(common, facts, ts_event, ts_init)?;
        self.write_exit_evaluation_event(event)
            .context("bolt-v3 exit evaluation handoff failed")
    }

    pub fn write_exit_pre_submit_rejection(
        &self,
        decision_trace_id: &str,
        facts: BoltV3PreSubmitRejectionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<()> {
        let common = self.common_context.common_fields(decision_trace_id)?;
        let event = BoltV3ExitPreSubmitRejectionDecisionEvent::exit_pre_submit_rejection(
            common, facts, ts_event, ts_init,
        )?;
        self.write_exit_pre_submit_rejection_event(event)
            .context("bolt-v3 exit pre-submit rejection handoff failed")
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

    fn write_entry_pre_submit_rejection_event(
        &self,
        event: BoltV3EntryPreSubmitRejectionDecisionEvent,
    ) -> Result<()> {
        let handoff = Arc::clone(&self.handoff);
        thread::spawn(move || {
            let mut handoff = handoff
                .lock()
                .map_err(|_| anyhow!("bolt-v3 decision-evidence handoff mutex poisoned"))?;
            handoff.write_entry_pre_submit_rejection(event)
        })
        .join()
        .map_err(|_| anyhow!("bolt-v3 entry pre-submit rejection handoff thread panicked"))?
    }

    fn write_exit_evaluation_event(&self, event: BoltV3ExitEvaluationDecisionEvent) -> Result<()> {
        let handoff = Arc::clone(&self.handoff);
        thread::spawn(move || {
            let mut handoff = handoff
                .lock()
                .map_err(|_| anyhow!("bolt-v3 decision-evidence handoff mutex poisoned"))?;
            handoff.write_exit_evaluation(event)
        })
        .join()
        .map_err(|_| anyhow!("bolt-v3 exit evaluation handoff thread panicked"))?
    }

    fn write_exit_pre_submit_rejection_event(
        &self,
        event: BoltV3ExitPreSubmitRejectionDecisionEvent,
    ) -> Result<()> {
        let handoff = Arc::clone(&self.handoff);
        thread::spawn(move || {
            let mut handoff = handoff
                .lock()
                .map_err(|_| anyhow!("bolt-v3 decision-evidence handoff mutex poisoned"))?;
            handoff.write_exit_pre_submit_rejection(event)
        })
        .join()
        .map_err(|_| anyhow!("bolt-v3 exit pre-submit rejection handoff thread panicked"))?
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
