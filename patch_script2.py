import re

with open('src/strategies/binary_oracle_edge_taker/mod.rs', 'r') as f:
    content = f.read()

# Fix types in apply_robust_sizing_and_probe signature
# receive_context: &EntryEvaluationReceiveContext
# pricing_inputs: &EntryPricingInputs
# fair_probability_up: Probability
# fee_uncertainty_bps: f64 (resized_probe.fee_bps is f64 based on the error msg)

old_signature = """    fn apply_robust_sizing_and_probe(
        &self,
        evaluation: &mut EntryEvaluation,
        selected_side: OutcomeSide,
        fair_probability_up: f64,
        order_side: OrderSide,
        now_ms: u64,
        receive_context: &ProviderReceiveContext,
        pricing_inputs: &TakerPricingInputs,
        book_impact_cap_notional: f64,
        sized_notional: f64,
        fee_uncertainty_bps: u32,
        sized_executable_edge: BinaryOutcomeEdgeResult,
    ) {"""

new_signature = """    fn apply_robust_sizing_and_probe(
        &self,
        evaluation: &mut EntryEvaluation,
        selected_side: OutcomeSide,
        fair_probability_up: Probability,
        order_side: OrderSide,
        now_ms: u64,
        receive_context: &EntryEvaluationReceiveContext,
        pricing_inputs: &EntryPricingInputs,
        book_impact_cap_notional: f64,
        sized_notional: f64,
        fee_uncertainty_bps: f64,
        sized_executable_edge: BinaryOutcomeEdgeResult,
    ) {"""

content = content.replace(old_signature, new_signature)

with open('src/strategies/binary_oracle_edge_taker/mod.rs', 'w') as f:
    f.write(content)
