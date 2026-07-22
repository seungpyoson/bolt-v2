use std::process::Command;

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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
fn shadow_pnl_report_rejects_blank_lines_before_later_corruption() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    // Current evidence is strict JSONL. A blank record is corruption in its own
    // right, so the reader must stop at line 1 rather than filtering it away.
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
        stderr.contains("blank current decision evidence line 1"),
        "blank current evidence must fail at its original line: {stderr}"
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 4,
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
            "duplicate submit-linked strategy-input snapshot decision evidence for client_order_id client-order-collision"
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
    let client_order_id = "client-order-no-intent";
    let mut lines = Vec::new();
    push_trade_lines(
        &mut lines,
        TradeFixture {
            recorded_at_utc_ns: 1,
            client_order_id,
            market_id: "market-btc",
            instrument_id: "BTC-UP.POLYMARKET",
            selected_side: "up",
            expected_edge_basis_points: "150",
            fee_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    lines.remove(1);
    let evidence = lines.join("\n") + "\n";
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
            recorded_at_utc_ns: 1,
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
            recorded_at_utc_ns: 4,
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

#[derive(Clone, Copy)]
struct TradeFixture {
    recorded_at_utc_ns: i64,
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
    let mut snapshot_record = current_fixture(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bolt_v3/current_evidence/positive/submit_linked_strategy_input_snapshot.jsonl"
    )));
    snapshot_record["recorded_at_utc_ns"] = serde_json::json!(trade.recorded_at_utc_ns);
    snapshot_record["snapshot"]["details"]["selected_side"] =
        serde_json::json!(trade.selected_side);
    snapshot_record["snapshot"]["details"]["expected_edge_basis_points"] =
        serde_json::json!(trade.expected_edge_basis_points);
    snapshot_record["snapshot"]["details"]["fee_rate_basis_points"] =
        serde_json::json!(trade.fee_rate_basis_points);
    snapshot_record["snapshot"]["submission"]["client_order_id"] =
        serde_json::json!(trade.client_order_id);
    snapshot_record["snapshot"]["submission"]["instrument_id"] =
        serde_json::json!(trade.instrument_id);
    snapshot_record["snapshot"]["submission"]["price"] = serde_json::json!(trade.price);
    snapshot_record["snapshot"]["submission"]["quantity"] = serde_json::json!(trade.quantity);
    if let Some(market_id) = market_id {
        snapshot_record["snapshot"]["details"]["market_id"] = serde_json::json!(market_id);
    } else {
        snapshot_record["snapshot"]["details"]
            .as_object_mut()
            .expect("strategy-input details must be an object")
            .remove("market_id");
    }
    lines.push(serde_json::to_string(&snapshot_record).expect("snapshot fixture should serialize"));

    let mut intent_record = current_fixture(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bolt_v3/current_evidence/positive/entry_order_intent.jsonl"
    )));
    intent_record["recorded_at_utc_ns"] = serde_json::json!(trade.recorded_at_utc_ns + 1);
    intent_record["order_intent"]["instrument_id"] = serde_json::json!(trade.instrument_id);
    intent_record["order_intent"]["client_order_id"] = serde_json::json!(trade.client_order_id);
    intent_record["order_intent"]["price"] = serde_json::json!(trade.price);
    intent_record["order_intent"]["quantity"] = serde_json::json!(trade.quantity);
    lines.push(serde_json::to_string(&intent_record).expect("intent fixture should serialize"));

    let mut admission_record = current_fixture(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bolt_v3/current_evidence/positive/admitted_entry_admission.jsonl"
    )));
    admission_record["recorded_at_utc_ns"] = serde_json::json!(trade.recorded_at_utc_ns + 2);
    admission_record["decision"]["client_order_id"] = serde_json::json!(trade.client_order_id);
    admission_record["decision"]["instrument_id"] = serde_json::json!(trade.instrument_id);
    lines.push(
        serde_json::to_string(&admission_record).expect("admission fixture should serialize"),
    );
}

fn current_fixture(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw.trim()).expect("committed current evidence fixture must decode")
}
