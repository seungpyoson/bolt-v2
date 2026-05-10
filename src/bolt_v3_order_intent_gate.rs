use anyhow::Result;

use crate::bolt_v3_decision_events::{
    BoltV3DecisionEventCatalogHandoff, BoltV3EntryOrderSubmissionDecisionEvent,
    BoltV3ExitOrderSubmissionDecisionEvent,
};

pub fn gate_entry_order_submission<T>(
    handoff: &mut BoltV3DecisionEventCatalogHandoff,
    event: BoltV3EntryOrderSubmissionDecisionEvent,
    submit: impl FnOnce() -> Result<T>,
) -> Result<T> {
    handoff.write_entry_order_submission(event)?;
    submit()
}

pub fn gate_exit_order_submission<T>(
    handoff: &mut BoltV3DecisionEventCatalogHandoff,
    event: BoltV3ExitOrderSubmissionDecisionEvent,
    submit: impl FnOnce() -> Result<T>,
) -> Result<T> {
    handoff.write_exit_order_submission(event)?;
    submit()
}
