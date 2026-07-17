import re

with open('src/strategies/binary_oracle_edge_taker/mod.rs', 'r') as f:
    content = f.read()

# Fix the compile errors in apply_robust_sizing_and_probe invocation and method

content = content.replace("receive_context,\n                        selected_side,", "*receive_context,\n                        selected_side,")

call_str = """                    self.apply_robust_sizing_and_probe(
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
                    );"""

new_call_str = """                    self.apply_robust_sizing_and_probe(
                        &mut evaluation,
                        selected_side,
                        fair_probability_up,
                        order_side,
                        now_ms,
                        &receive_context,
                        &pricing_inputs,
                        book_impact_cap_notional,
                        sized_notional,
                        fee_uncertainty_bps,
                        sized_executable_edge,
                    );"""

content = content.replace(call_str, new_call_str)

with open('src/strategies/binary_oracle_edge_taker/mod.rs', 'w') as f:
    f.write(content)
