#[test]
fn gross_intent_evaluation_log_keeps_field_order_aligned_with_arguments() {
    let source = std::fs::read_to_string("src/strategies/binary_oracle_edge_taker/mod.rs")
        .expect("strategy source should be readable");
    let format_sequence = "market_id={:?} selected_side={:?} gross_edge_bps={:?} \
         sized_notional={:?} submission_blocked_reason={:?}";
    assert_eq!(
        source.matches(format_sequence).count(),
        1,
        "the gross-intent evaluation log must expose decision fields in one stable order"
    );

    let normalized_source = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let argument_sequence = [
        "fields.market_id,",
        "fields.selected_side,",
        "fields.sized_worst_case_ev_bps,",
        "fields.sized_notional,",
        "fields.submission_blocked_reason,",
    ]
    .join(" ");
    assert_eq!(
        normalized_source.matches(&argument_sequence).count(),
        1,
        "gross-intent evaluation log arguments must match the format-field order"
    );
}
