use std::process::Command;

use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3OrderIntentClampOutcome,
    BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields,
    BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitIntentKind,
    current::{
        encode_admitted_entry_admission, encode_entry_order_intent,
        encode_submit_linked_strategy_input_snapshot,
    },
};

#[test]
fn shadow_pnl_report_joins_fixture_evidence_to_settlements() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    std::fs::write(&evidence_path, fixture_evidence_jsonl())
        .expect("fixture evidence should write");
    std::fs::write(&settlements_path, fixture_settlements_jsonl())
        .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert_eq!(
        stdout,
        "day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps\n2026-06-10,BTC,2,0.5,3,0.055,2.945,175,2500\n"
    );
}

#[test]
fn shadow_pnl_report_matches_settlement_by_market_and_instrument() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-down",
            market_id: "market-btc",
            instrument_id: "BTC-DOWN.POLYMARKET",
            selected_side: "down",
            expected_edge_basis_points: "200",
            fee_rate_basis_points: "50",
            price: "0.60",
            quantity: "5",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        [
            serde_json::json!({
                "settlement_date": "2026-06-10",
                "asset": "BTC",
                "market_id": "market-btc",
                "instrument_id": "BTC-UP.POLYMARKET",
                "winning_side": "up",
                "settlement_price": "1.00"
            }),
            serde_json::json!({
                "settlement_date": "2026-06-10",
                "asset": "BTC",
                "market_id": "market-btc",
                "instrument_id": "BTC-DOWN.POLYMARKET",
                "winning_side": "up",
                "settlement_price": "0.00"
            }),
        ]
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("settlement fixture should serialize"))
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert_eq!(
        stdout,
        "day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps\n2026-06-10,BTC,1,0,-3,0.015,-3.015,200,-10000\n"
    );
}

#[test]
fn shadow_pnl_report_rejects_unrecognized_winning_side() {
    // A winning_side that names neither legal side must fail loud, EVEN when the
    // settlement_price is consistent with a loss (settlement_price <= entry_price)
    // so the settlement_price-vs-entry_price cross-check would not trip. Without
    // the legality gate the garbage side silently collapses to won=false and the
    // row is counted as a loss.
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-bad-side",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "sideways",
            "settlement_price": "0.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("settlement winning_side")
            && stderr.contains("not a recognized binary outcome side"),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_rejects_unrecognized_selected_side() {
    // selected_side is strategy-written evidence, not operator input. Any token
    // outside the canonical binary side vocabulary means the evidence chain is
    // corrupted and must fail loud before it can collapse to a loss.
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-bad-selected-side",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "sideways",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "0.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("evidence selected_side")
            && stderr.contains("not a recognized binary outcome side"),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_matches_unique_settlement_without_settlement_market_id() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-wildcard-settlement",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "1.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert_eq!(
        stdout,
        "day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps\n2026-06-10,BTC,1,1,6,0.04,5.96,150,15000\n"
    );
}

#[test]
fn shadow_pnl_report_rejects_ambiguous_settlement_without_trade_market_id() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines_with_snapshot_market_id(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-ambiguous",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
        None,
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        [
            serde_json::json!({
                "settlement_date": "2026-06-10",
                "asset": "BTC",
                "market_id": "market-btc-a",
                "instrument_id": "BTC-UP.POLYMARKET",
                "winning_side": "up",
                "settlement_price": "1.00"
            }),
            serde_json::json!({
                "settlement_date": "2026-06-11",
                "asset": "BTC",
                "market_id": "market-btc-b",
                "instrument_id": "BTC-UP.POLYMARKET",
                "winning_side": "down",
                "settlement_price": "0.00"
            }),
        ]
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("settlement fixture should serialize"))
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("ambiguous settlement for client-order-ambiguous"),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_rejects_blank_evidence_records_before_payload_decode() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    std::fs::write(&evidence_path, "\n{ not valid json\n").expect("fixture evidence should write");
    std::fs::write(&settlements_path, "").expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("blank record at line index 0"),
        "blank records must fail before a later payload is decoded: {stderr}"
    );
}

#[test]
fn shadow_pnl_report_escapes_csv_asset_fields() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-one",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC,PERP",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "1.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert_eq!(
        stdout,
        "day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps\n2026-06-10,\"BTC,PERP\",1,1,6,0.04,5.96,150,15000\n"
    );
}

#[test]
fn shadow_pnl_report_rejects_trade_with_no_matching_settlement() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-unsettled",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    // The only settlement is for a different instrument, so the BTC-UP trade has no match.
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-DOWN.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "0.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("missing settlement for client-order-unsettled"),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_rejects_duplicate_exact_market_id_settlements() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-dup-market",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    // Two settlements share the same instrument_id AND market_id: the exact-match
    // branch cannot pick one and must fail instead of choosing by file order.
    std::fs::write(
        &settlements_path,
        [
            serde_json::json!({
                "settlement_date": "2026-06-10",
                "asset": "BTC",
                "market_id": "market-btc",
                "instrument_id": "BTC-UP.POLYMARKET",
                "winning_side": "up",
                "settlement_price": "1.00"
            }),
            serde_json::json!({
                "settlement_date": "2026-06-10",
                "asset": "BTC",
                "market_id": "market-btc",
                "instrument_id": "BTC-UP.POLYMARKET",
                "winning_side": "up",
                "settlement_price": "1.00"
            }),
        ]
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("settlement fixture should serialize"))
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains(
            "ambiguous settlement for client-order-dup-market: duplicate market_id match"
        ),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_rejects_settlement_inconsistent_with_winning_side() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-inconsistent",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.60",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    // winning_side "up" marks a win for the "up" selection, but settlement_price 0.00
    // is a loss: the two fields contradict and the report must fail loud.
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "0.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("settlement inconsistency for client-order-inconsistent")
            && stderr.contains("marks a win"),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_rejects_duplicate_client_order_id() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let mut lines = Vec::new();
    // Two distinct would-be entries reuse the same client_order_id; the join must
    // refuse rather than silently overwrite one would-be trade.
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-collision",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-collision",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    std::fs::write(&evidence_path, lines.join("\n") + "\n").expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "1.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains(
            "duplicate strategy_input_snapshot decision evidence for client_order_id client-order-collision"
        ),
        "{stderr}"
    );
}

#[test]
fn shadow_pnl_report_rejects_admitted_entry_without_order_intent() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");

    // An admitted entry whose order-intent line is missing (truncated or corrupted
    // evidence log). The join is driven by the admitted entries, so a missing intent
    // MUST fail loud rather than silently drop the would-be trade from the report.
    let trade = TradeFixture {
        client_order_id: "client-order-no-intent",
        market_id: "market-btc",
        instrument_id: "BTC-UP.POLYMARKET",
        selected_side: "up",
        expected_edge_basis_points: "150",
        fee_rate_basis_points: "100",
        price: "0.40",
        quantity: "10",
    };
    let evidence = [
        record_line(
            encode_submit_linked_strategy_input_snapshot(&strategy_input_snapshot(
                &trade,
                Some(trade.market_id),
            ))
            .expect("snapshot should encode"),
        ),
        record_line(
            encode_admitted_entry_admission(&admitted_entry(&trade))
                .expect("admission should encode"),
        ),
    ]
    .join("\n")
        + "\n";
    std::fs::write(&evidence_path, evidence).expect("fixture evidence should write");
    std::fs::write(
        &settlements_path,
        serde_json::to_string(&serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "1.00"
        }))
        .expect("settlement fixture should serialize")
            + "\n",
    )
    .expect("fixture settlements should write");

    let output = Command::new(env!("CARGO_BIN_EXE_shadow_pnl_report"))
        .args([
            "--evidence-jsonl",
            evidence_path.to_str().expect("utf-8 path"),
            "--settlements-jsonl",
            settlements_path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("shadow_pnl_report should run");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("missing order intent for admitted entry client-order-no-intent"),
        "{stderr}"
    );
}

fn fixture_evidence_jsonl() -> String {
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-one",
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    push_trade_lines(
        &mut lines,
        TradeFixture {
            client_order_id: "client-order-two",
            market_id: "market-btc-next",
            instrument_id: "BTC-DOWN.POLYMARKET",
            selected_side: "down",
            expected_edge_basis_points: "200",
            fee_rate_basis_points: "50",
            price: "0.60",
            quantity: "5",
        },
    );
    lines.join("\n") + "\n"
}

fn fixture_settlements_jsonl() -> String {
    [
        serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc",
            "instrument_id": "BTC-UP.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "1.00"
        }),
        serde_json::json!({
            "settlement_date": "2026-06-10",
            "asset": "BTC",
            "market_id": "market-btc-next",
            "instrument_id": "BTC-DOWN.POLYMARKET",
            "winning_side": "up",
            "settlement_price": "0.00"
        }),
    ]
    .into_iter()
    .map(|value| serde_json::to_string(&value).expect("settlement fixture should serialize"))
    .collect::<Vec<_>>()
    .join("\n")
        + "\n"
}

struct TradeFixture {
    client_order_id: &'static str,
    market_id: &'static str,
    instrument_id: &'static str,
    selected_side: &'static str,
    expected_edge_basis_points: &'static str,
    fee_rate_basis_points: &'static str,
    price: &'static str,
    quantity: &'static str,
}

fn push_trade_lines(lines: &mut Vec<String>, trade: TradeFixture) {
    let market_id = trade.market_id;
    push_trade_lines_with_snapshot_market_id(lines, trade, Some(market_id));
}

fn push_trade_lines_with_snapshot_market_id(
    lines: &mut Vec<String>,
    trade: TradeFixture,
    market_id: Option<&str>,
) {
    let snapshot = strategy_input_snapshot(&trade, market_id);
    lines.push(record_line(
        encode_submit_linked_strategy_input_snapshot(&snapshot)
            .expect("snapshot fixture should encode"),
    ));
    lines.push(record_line(
        encode_entry_order_intent(&entry_order_intent(&trade))
            .expect("entry-intent fixture should encode"),
    ));
    lines.push(record_line(
        encode_admitted_entry_admission(&admitted_entry(&trade))
            .expect("admission fixture should encode"),
    ));
}

fn record_line(record: bolt_v2::bolt_v3_decision_evidence::sink::EncodedEvidenceRecord) -> String {
    std::str::from_utf8(record.bytes())
        .expect("current evidence bytes should be utf-8")
        .trim_end_matches('\n')
        .to_string()
}

fn strategy_input_snapshot(
    trade: &TradeFixture,
    market_id: Option<&str>,
) -> BoltV3StrategyInputEvidenceSnapshot {
    BoltV3StrategyInputEvidenceSnapshot {
        strategy_id: "strategy".into(),
        configured_target_id: "target".into(),
        market_selection_ruleset_id: "rules".into(),
        market_selection_outcome: "current".into(),
        market_id: market_id.map(str::to_string),
        polymarket_condition_id: None,
        polymarket_market_slug: None,
        polymarket_question_id: None,
        up_instrument_id: None,
        down_instrument_id: None,
        market_selection_timestamp_ms: None,
        selected_market_observed_timestamp_ms: None,
        polymarket_market_start_timestamp_ms: None,
        polymarket_market_end_timestamp_ms: None,
        price_to_beat_source: "oracle".into(),
        price_to_beat_value: "0".into(),
        reference_quote_ts_event: 1,
        spot_price: "0".into(),
        fast_venue_available: false,
        reference_current_price: None,
        reference_current_price_available: false,
        reference_current_price_source_id: None,
        reference_current_price_failed_over: None,
        realized_volatility: "0".into(),
        realized_volatility_surface_id: "surface".into(),
        realized_volatility_as_of_ms: None,
        realized_volatility_gate_result: None,
        realized_volatility_receive_watermark_ms: None,
        realized_volatility_annualized_decimal: "0".into(),
        realized_volatility_measured_annualized_decimal: "0".into(),
        realized_volatility_noise_robust_annualized_decimal: "0".into(),
        realized_volatility_continuous_annualized_decimal: "0".into(),
        realized_volatility_jump_annualized_decimal: "0".into(),
        realized_volatility_forecast_annualized_decimal: "0".into(),
        realized_volatility_pricing_component: "continuous".into(),
        realized_volatility_seconds_per_annum: "31536000".into(),
        realized_volatility_aggregation: "median".into(),
        realized_volatility_sources_used: Vec::new(),
        realized_volatility_source_diagnostics: Vec::new(),
        realized_volatility_unknown_source_rejections: Default::default(),
        realized_volatility_blockers: Vec::new(),
        realized_volatility_config_fingerprint: "config".into(),
        seconds_to_market_end: 60,
        pricing_kurtosis: "3".into(),
        theta_decay_factor: "1".into(),
        theta_scaled_min_edge_bps: "0".into(),
        fair_probability_up: "0.5".into(),
        uncertainty_band_probability: "0".into(),
        expected_edge_basis_points: trade.expected_edge_basis_points.into(),
        worst_case_edge_basis_points: trade.expected_edge_basis_points.into(),
        up_worst_case_edge_basis_points: None,
        down_worst_case_edge_basis_points: None,
        gate_blocked_by: Vec::new(),
        pricing_blocked_by: Vec::new(),
        fast_venue_name: None,
        fast_venue_age_ms: None,
        fast_venue_jitter_ms: None,
        fast_venue_incoherent: false,
        lead_agreement_corr: None,
        fee_rate_basis_points: trade.fee_rate_basis_points.into(),
        selected_side: Some(trade.selected_side.into()),
        submission_instrument_id: trade.instrument_id.into(),
        submission_order_side: "BUY".into(),
        submission_price: trade.price.into(),
        submission_quantity: trade.quantity.into(),
        client_order_id: trade.client_order_id.into(),
    }
}

fn entry_order_intent(trade: &TradeFixture) -> BoltV3OrderIntentEvidence {
    BoltV3OrderIntentEvidence {
        strategy_id: "strategy".into(),
        intent_kind: BoltV3OrderIntentKind::Entry,
        instrument_id: trade.instrument_id.into(),
        client_order_id: trade.client_order_id.into(),
        order_side: "BUY".into(),
        price: trade.price.into(),
        quantity: trade.quantity.into(),
        clamp_outcome: Some(BoltV3OrderIntentClampOutcome::WithinBounds),
        order_fields: BoltV3OrderIntentOrderFields {
            order_type: "LIMIT".into(),
            time_in_force: "GTC".into(),
            price: Some(trade.price.into()),
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            expire_time_unix_nanos: None,
            is_post_only: false,
            is_reduce_only: false,
            is_quote_quantity: false,
        },
    }
}

fn admitted_entry(trade: &TradeFixture) -> BoltV3AdmissionDecisionEvidence {
    BoltV3AdmissionDecisionEvidence {
        strategy_id: "strategy".into(),
        execution_client_id: "execution".into(),
        client_order_id: trade.client_order_id.into(),
        instrument_id: trade.instrument_id.into(),
        notional: "10".into(),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        outcome: BoltV3AdmissionOutcome::Admitted,
        loss_halt_reasons: Vec::new(),
        snapshot_present: false,
        snapshot_observed_at_ns: None,
        admission_now_ns: 1,
        snapshot_age_ns: None,
        max_snapshot_age_ns: None,
        snapshot_source: None,
        per_trade_pnl_present: false,
        daily_pnl_present: false,
        rolling_pnl_present: false,
        current_equity_present: false,
        peak_equity_present: false,
        last_account_state_observed_at_ns: None,
        last_portfolio_snapshot_observed_at_ns: None,
        last_position_event_observed_at_ns: None,
        stale_reason: None,
        loss_snapshot_observed_at_ns: None,
        loss_eval_now_ns: None,
    }
}
