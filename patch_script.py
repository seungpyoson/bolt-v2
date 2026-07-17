import re

with open('src/strategies/binary_oracle_edge_taker/mod.rs', 'r') as f:
    content = f.read()

# Define the new method string
new_method = """    fn apply_robust_sizing_and_probe(
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
    ) {
        let sized_expected_ev_per_notional = sized_executable_edge.edge_bps / BPS_DENOMINATOR;
        evaluation.expected_ev_per_notional = Some(sized_expected_ev_per_notional);

        let resized_notional = choose_robust_size(&self.robust_sizing_inputs(
            sized_expected_ev_per_notional,
            book_impact_cap_notional,
        ));

        if !is_positive_finite(resized_notional)
            || (resized_notional - sized_notional).abs() <= notional_float_tolerance(sized_notional)
        {
            return;
        }

        let resized_probe = match self.executable_entry_probe_for_side(
            selected_side,
            order_side,
            resized_notional,
        ) {
            Ok(probe) => probe,
            Err(reason) => {
                let resized_executable_edge = BinaryOutcomeEdgeResult::blocked(selected_side, reason);
                evaluation.sized_worst_case_ev_bps =
                    executable_edge_worst_case_ev_bps(Some(resized_executable_edge));
                evaluation.sized_executable_edge = Some(resized_executable_edge);
                push_executable_edge_pricing_block(
                    &mut evaluation.pricing_blocked_by,
                    selected_side,
                    Some(reason),
                );
                evaluation.selected_side = None;
                evaluation.sized_notional = None;
                evaluation.expected_ev_per_notional = None;
                return;
            }
        };

        let resized_fee_uncertainty_bps = fee_uncertainty_bps.max(resized_probe.fee_bps);

        let Some((resized_uncertainty_band, resized_adjusted_probability_up)) =
            self.adjusted_probability_up_for_fee_uncertainty(
                now_ms,
                receive_context,
                selected_side,
                fair_probability_up,
                resized_fee_uncertainty_bps,
            )
        else {
            evaluation
                .pricing_blocked_by
                .push(EntryPricingBlockReason::UncertaintyBandUnavailable);
            evaluation.selected_side = None;
            evaluation.sized_notional = None;
            evaluation.expected_ev_per_notional = None;
            return;
        };

        evaluation.uncertainty_band_probability = Some(resized_uncertainty_band);

        let resized_executable_edge = self.executable_edge_for_side(
            selected_side,
            fair_probability_up,
            resized_adjusted_probability_up,
            pricing_inputs.theta_scaled_min_edge_bps,
            resized_probe,
        );

        evaluation.sized_worst_case_ev_bps =
            executable_edge_worst_case_ev_bps(Some(resized_executable_edge));
        evaluation.sized_executable_edge = Some(resized_executable_edge);

        let final_expected_ev_per_notional = resized_executable_edge.edge_bps / BPS_DENOMINATOR;
        let final_supported_notional = choose_robust_size(&self.robust_sizing_inputs(
            final_expected_ev_per_notional,
            book_impact_cap_notional,
        ));

        let resized_notional_supported = resized_notional
            <= final_supported_notional + notional_float_tolerance(final_supported_notional);

        if resized_executable_edge.trade_allowed && resized_notional_supported {
            evaluation.sized_notional = Some(resized_notional);
            evaluation.expected_ev_per_notional = Some(final_expected_ev_per_notional);
        } else if resized_executable_edge.trade_allowed {
            evaluation
                .pricing_blocked_by
                .push(EntryPricingBlockReason::SizedNotionalUnsupported(selected_side));
            evaluation.selected_side = None;
            evaluation.sized_notional = None;
            evaluation.expected_ev_per_notional = None;
        } else {
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                selected_side,
                resized_executable_edge.block_reason,
            );
            evaluation.selected_side = None;
            evaluation.sized_notional = None;
            evaluation.expected_ev_per_notional = None;
        }
    }
"""

replacement_string = """                    self.apply_robust_sizing_and_probe(
                        &mut evaluation,
                        selected_side,
                        fair_probability_up,
                        order_side,
                        now_ms,
                        receive_context,
                        pricing_inputs,
                        book_impact_cap_notional,
                        sized_notional,
                        fee_uncertainty_bps,
                        sized_executable_edge,
                    );
"""

# Now we need to find the exact block of code to replace
start_str = """                    let sized_expected_ev_per_notional =
                        sized_executable_edge.edge_bps / BPS_DENOMINATOR;
                    evaluation.expected_ev_per_notional = Some(sized_expected_ev_per_notional);
                    let resized_notional = choose_robust_size(&self.robust_sizing_inputs(
                        sized_expected_ev_per_notional,
                        book_impact_cap_notional,
                    ));
                    if is_positive_finite(resized_notional)
                        && (resized_notional - sized_notional).abs()
                            > notional_float_tolerance(sized_notional)
                    {
                        let resized_probe = match self.executable_entry_probe_for_side("""

end_str = """                            evaluation.sized_notional = None;
                            evaluation.expected_ev_per_notional = None;
                        }
                    }"""

start_idx = content.find(start_str)
end_idx = content.find(end_str) + len(end_str)

if start_idx != -1 and end_idx != -1:
    new_content = content[:start_idx] + replacement_string + content[end_idx:]

    eval_idx = new_content.find("fn entry_evaluation_for_receive_at(")
    if eval_idx != -1:
        new_content = new_content[:eval_idx] + new_method + "\n" + new_content[eval_idx:]

        with open('src/strategies/binary_oracle_edge_taker/mod.rs', 'w') as f:
            f.write(new_content)
        print("Successfully updated src/strategies/binary_oracle_edge_taker/mod.rs")
    else:
        print("Could not find entry_evaluation_for_receive_at")
else:
    print("Could not find target string to replace")
