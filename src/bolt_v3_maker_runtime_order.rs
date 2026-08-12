//! Shared maker runtime order-plan dispatcher.
//!
//! Quote planning emits maker order intents; this module is the generic bridge
//! that compiles those intents into NT order commands and dispatches them through
//! the existing maker command sink. It owns no strategy state, venue branching,
//! market-family dispatch, clocks, or config defaults.

use anyhow::{Context, Result};

use crate::{
    bolt_v3_maker_order_compile::{
        MakerCompiledOrderCommand, MakerOrderCompileBlockReason, MakerOrderCompileInput,
        compile_maker_order_intent,
    },
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandFailure, MakerOrderCommandSink, MakerOrderDispatchInput,
        MakerOrderDispatchOutcome, dispatch_maker_order_command,
    },
    bolt_v3_maker_order_plan::{MakerLegOrderPlan, MakerOrderPlan, MakerOrderPlanBlockReason},
    bolt_v3_order_intent::NtOrderTemplate,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeOrderDispatchInput<'a> {
    pub order_plan: &'a MakerOrderPlan,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeOrderDispatchOutcome {
    pub yes: MakerRuntimeLegOrderDispatchOutcome,
    pub no: MakerRuntimeLegOrderDispatchOutcome,
}

impl MakerRuntimeOrderDispatchOutcome {
    /// The first per-leg command failure (YES then NO), if either leg failed to route
    /// its compiled command through the execution policy. A caller reconciles the
    /// identities of whichever legs *did* dispatch, then fails loud on this so a
    /// partial two-leg dispatch never silently swallows a routing failure.
    #[must_use]
    pub fn command_failure(&self) -> Option<&MakerOrderCommandFailure> {
        self.yes
            .command_failure
            .as_ref()
            .or(self.no.command_failure.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeLegOrderDispatchOutcome {
    pub dispatch: Option<MakerOrderDispatchOutcome>,
    pub blocked_by: Option<MakerRuntimeOrderDispatchBlockReason>,
    /// Set when routing this leg's compiled command through the execution policy
    /// returned an error. The leg did not dispatch (`dispatch` is `None`), but the
    /// sibling leg's outcome is still returned so the caller can reconcile the
    /// identity it *did* dispatch before failing loud, instead of orphaning it.
    pub command_failure: Option<MakerOrderCommandFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRuntimeOrderDispatchBlockReason {
    OrderPlanBlocked(MakerOrderPlanBlockReason),
    CompileBlocked(MakerOrderCompileBlockReason),
}

pub fn dispatch_maker_runtime_order_plan(
    input: MakerRuntimeOrderDispatchInput<'_>,
    sink: &mut impl MakerOrderCommandSink,
) -> Result<MakerRuntimeOrderDispatchOutcome> {
    dispatch_maker_runtime_order_plan_with_command_router(
        input,
        &mut |command, submit_order_prefix| {
            dispatch_maker_order_command(
                MakerOrderDispatchInput {
                    command,
                    submit_order_prefix,
                },
                sink,
            )
        },
    )
}

pub fn dispatch_maker_runtime_order_plan_with_command_router(
    input: MakerRuntimeOrderDispatchInput<'_>,
    route_command: &mut impl FnMut(
        &MakerCompiledOrderCommand,
        &str,
    ) -> std::result::Result<
        MakerOrderDispatchOutcome,
        MakerOrderCommandFailure,
    >,
) -> Result<MakerRuntimeOrderDispatchOutcome> {
    let yes = dispatch_leg(input, &input.order_plan.yes, route_command)?;
    // Short-circuit on a YES command failure exactly as the prior `?` did (the NO leg
    // is not attempted), but return the partial outcome instead of discarding it, so
    // the caller can reconcile/abort with both legs' state in hand.
    let no = if yes.command_failure.is_some() {
        MakerRuntimeLegOrderDispatchOutcome {
            dispatch: None,
            blocked_by: None,
            command_failure: None,
        }
    } else {
        dispatch_leg(input, &input.order_plan.no, route_command)?
    };
    Ok(MakerRuntimeOrderDispatchOutcome { yes, no })
}

fn dispatch_leg(
    input: MakerRuntimeOrderDispatchInput<'_>,
    leg_plan: &MakerLegOrderPlan,
    route_command: &mut impl FnMut(
        &MakerCompiledOrderCommand,
        &str,
    ) -> std::result::Result<
        MakerOrderDispatchOutcome,
        MakerOrderCommandFailure,
    >,
) -> Result<MakerRuntimeLegOrderDispatchOutcome> {
    if let Some(reason) = leg_plan.blocked_by {
        return Ok(blocked(
            MakerRuntimeOrderDispatchBlockReason::OrderPlanBlocked(reason),
        ));
    }

    let Some(intent) = leg_plan.intent.as_ref() else {
        return Ok(MakerRuntimeLegOrderDispatchOutcome {
            dispatch: None,
            blocked_by: None,
            command_failure: None,
        });
    };

    let compile = compile_maker_order_intent(MakerOrderCompileInput {
        intent,
        submit_template: input.submit_template,
        price_precision: input.price_precision,
        quantity_precision: input.quantity_precision,
    });
    let Some(command) = compile.command.as_ref() else {
        let reason = compile
            .blocked_by
            .context("maker order compiler returned neither command nor block reason")?;
        return Ok(blocked(
            MakerRuntimeOrderDispatchBlockReason::CompileBlocked(reason),
        ));
    };

    match route_command(command, input.submit_order_prefix) {
        Ok(dispatch) => Ok(MakerRuntimeLegOrderDispatchOutcome {
            dispatch: Some(dispatch),
            blocked_by: None,
            command_failure: None,
        }),
        // A command failure becomes per-leg data (not an early `?` abort) so a partial
        // two-leg dispatch never discards the sibling leg's dispatched identity. The
        // caller reconciles the dispatched leg, then fails loud on this error.
        Err(error) => Ok(MakerRuntimeLegOrderDispatchOutcome {
            dispatch: None,
            blocked_by: None,
            command_failure: Some(error),
        }),
    }
}

fn blocked(reason: MakerRuntimeOrderDispatchBlockReason) -> MakerRuntimeLegOrderDispatchOutcome {
    MakerRuntimeLegOrderDispatchOutcome {
        dispatch: None,
        blocked_by: Some(reason),
        command_failure: None,
    }
}
