use std::process::Command;

use rust_decimal::Decimal;

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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "50",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
    // selected_side is a closed wire enum. Any token outside that vocabulary
    // must fail at current-evidence decoding before it can reach PnL reduction.
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
        stderr.contains("malformed current payload")
            && stderr.contains("unknown variant `sideways`"),
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
            execution_component_rate_basis_points: "100",
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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

#[test]
fn shadow_pnl_report_rejects_admitted_entry_without_bound_economics() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let evidence_path = temp.path().join("order-intents.jsonl");
    let settlements_path = temp.path().join("shadow-settlements.jsonl");
    let client_order_id = "client-order-no-economics";
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
            execution_component_rate_basis_points: "100",
            price: "0.40",
            quantity: "10",
        },
    );
    let mut admission: serde_json::Value =
        serde_json::from_str(&lines[2]).expect("admission fixture should decode");
    admission["decision"]["economics"] = serde_json::Value::Null;
    lines[2] = serde_json::to_string(&admission).expect("admission fixture should encode");
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
        .arg("--config")
        .arg("tests/fixtures/bolt_v3/root.toml")
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
        stderr.contains("missing bound economics for admitted entry client-order-no-economics"),
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
            execution_component_rate_basis_points: "100",
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
            execution_component_rate_basis_points: "50",
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
    execution_component_rate_basis_points: &'static str,
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
    admission_record["decision"]["reservation"] = serde_json::Value::Null;
    let price = trade
        .price
        .parse::<Decimal>()
        .expect("fixture price must be decimal");
    let quantity = trade
        .quantity
        .parse::<Decimal>()
        .expect("fixture quantity must be decimal");
    let fee_bps = trade
        .execution_component_rate_basis_points
        .parse::<Decimal>()
        .expect("fixture fee rate must be decimal");
    let expected_edge_bps = trade
        .expected_edge_basis_points
        .parse::<Decimal>()
        .expect("fixture edge must be decimal");
    let reservation_basis = price * quantity;
    let fee_rate = fee_bps / Decimal::from(10_000);
    let core_total = Decimal::ZERO - reservation_basis * fee_rate;
    let core_edge_ratio = expected_edge_bps / Decimal::from(10_000);
    let core_net_edge = reservation_basis * core_edge_ratio;
    let economics_at_ns = u64::try_from(trade.recorded_at_utc_ns + 2)
        .expect("fixture economics timestamp must be positive");
    let native_fee_effect = serde_json::json!({
        "amount": core_total.to_string(),
        "unit": { "kind": "currency", "currency_id": "USDC" },
        "inventory_application": null,
    });
    admission_record["decision"]["economics"] = serde_json::json!({
        "decision_correlation_id": trade.client_order_id,
        "core_total": core_total.to_string(),
        "core_net_edge": core_net_edge.to_string(),
        "core_edge_ratio": core_edge_ratio.to_string(),
        "forecast_net_edge": core_net_edge.to_string(),
        "forecast_complete": true,
        "missing_forecast_component_ids": [],
        "valid_until_ns": u64::MAX,
        "forecast_valid_until_ns": u64::MAX,
        "source_snapshot_ids": ["shadow-pnl-fixture-economics"],
        "reservation_basis": reservation_basis.to_string(),
        "full_reservation_liability": (reservation_basis - core_total).to_string(),
        "components": [{
            "component_id": "shadow-pnl-fixture-protocol-fee",
            "class": "charge",
            "economic_kind": "protocol_trading",
            "scope": {
                "kind": "decision",
                "decision_correlation_id": trade.client_order_id,
            },
            "point_estimate": {
                "kind": "non_zero",
                "effect": native_fee_effect.clone(),
            },
            "point_valuation": {
                "native_effect": native_fee_effect,
                "normalized_amount": core_total.to_string(),
                "reporting_currency": "USDC",
                "route_id": null,
                "source_snapshot_ids": ["shadow-pnl-fixture-economics"],
                "valued_at_ns": economics_at_ns,
                "valid_until_ns": u64::MAX,
            },
            "debit_risk_bound": null,
            "debit_risk_bound_valuation": null,
            "treatment": { "kind": "guaranteed_conditional_on_action" },
            "calculation_factors": [{
                "factor_id": "fee_rate",
                "value": fee_rate.to_string(),
            }],
            "formula_id": "price-times-quantity-times-rate",
            "source": {
                "source_id": "shadow-pnl-fixture-provider",
                "snapshot_id": "shadow-pnl-fixture-economics",
                "source_at_ns": economics_at_ns,
                "fetched_at_ns": economics_at_ns,
                "valid_until_ns": u64::MAX,
            },
        }],
    });
    lines.push(
        serde_json::to_string(&admission_record).expect("admission fixture should serialize"),
    );
}

fn current_fixture(raw: &str) -> serde_json::Value {
    let baseline = raw
        .lines()
        .next()
        .expect("committed current evidence corpus must contain a baseline");
    serde_json::from_str(baseline).expect("committed current evidence baseline must decode")
}
