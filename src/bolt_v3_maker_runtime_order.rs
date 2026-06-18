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
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        dispatch_maker_order_command,
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

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeLegOrderDispatchOutcome {
    pub dispatch: Option<MakerOrderDispatchOutcome>,
    pub blocked_by: Option<MakerRuntimeOrderDispatchBlockReason>,
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
    ) -> Result<MakerOrderDispatchOutcome>,
) -> Result<MakerRuntimeOrderDispatchOutcome> {
    Ok(MakerRuntimeOrderDispatchOutcome {
        yes: dispatch_leg(input, &input.order_plan.yes, route_command)?,
        no: dispatch_leg(input, &input.order_plan.no, route_command)?,
    })
}

fn dispatch_leg(
    input: MakerRuntimeOrderDispatchInput<'_>,
    leg_plan: &MakerLegOrderPlan,
    route_command: &mut impl FnMut(
        &MakerCompiledOrderCommand,
        &str,
    ) -> Result<MakerOrderDispatchOutcome>,
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

    let dispatch = route_command(command, input.submit_order_prefix)?;
    Ok(MakerRuntimeLegOrderDispatchOutcome {
        dispatch: Some(dispatch),
        blocked_by: None,
    })
}

fn blocked(reason: MakerRuntimeOrderDispatchBlockReason) -> MakerRuntimeLegOrderDispatchOutcome {
    MakerRuntimeLegOrderDispatchOutcome {
        dispatch: None,
        blocked_by: Some(reason),
    }
}
