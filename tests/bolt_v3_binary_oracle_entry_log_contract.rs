#[test]
fn entry_evaluation_log_keeps_sizing_field_order_aligned_with_arguments() {
    let source = std::fs::read_to_string("src/strategies/binary_oracle_edge_taker/mod.rs")
        .expect("strategy source should be readable");
    let sizing_format_sequence = "expected_ev_per_notional={:?} order_notional_target={} \
         maximum_position_notional={} risk_lambda={} sizing_ev_reference_bps={} \
         book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?}";
    assert_eq!(
        source.matches(sizing_format_sequence).count(),
        2,
        "warn and info entry-evaluation log formats must expose sizing fields in the same order"
    );

    let normalized_source = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let sizing_argument_sequence = [
        "fields.expected_ev_per_notional,",
        "fields.order_notional_target,",
        "fields.maximum_position_notional,",
        "fields.risk_lambda,",
        "fields.sizing_ev_reference_bps,",
        "fields.book_impact_cap_bps,",
        "fields.book_impact_cap_notional,",
        "fields.sized_notional,",
    ]
    .join(" ");
    assert_eq!(
        normalized_source.matches(&sizing_argument_sequence).count(),
        2,
        "warn and info entry-evaluation log argument lists must match the sizing field format order"
    );
}
