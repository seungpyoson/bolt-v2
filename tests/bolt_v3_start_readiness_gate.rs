mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
    bolt_v3_start_readiness::{
        BoltV3StartReadinessGateError, check_bolt_v3_start_readiness_gate,
        require_bolt_v3_start_readiness_gate,
    },
};
use nautilus_core::{Params, UnixNanos};
use nautilus_live::node::NodeState;
use nautilus_model::{
    enums::AssetClass,
    identifiers::InstrumentId,
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Price, Quantity},
};
use serde_json::json;
use tempfile::TempDir;
use ustr::Ustr;

fn polymarket_updown_option(
    instrument_id: &str,
    token_id: &str,
    condition_id: &str,
    question_id: &str,
    market_slug: &str,
    outcome: &str,
    start_ms: u64,
    end_ms: u64,
) -> InstrumentAny {
    let instrument_id = InstrumentId::from(instrument_id);
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    let mut info = Params::new();
    info.insert("token_id".to_string(), json!(token_id));
    info.insert("condition_id".to_string(), json!(condition_id));
    info.insert("question_id".to_string(), json!(question_id));
    info.insert("market_slug".to_string(), json!(market_slug));

    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        instrument_id.symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(start_ms * 1_000_000),
        UnixNanos::from(end_ms * 1_000_000),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        Some(Ustr::from(outcome)),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(info),
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

#[test]
fn start_readiness_gate_blocks_missing_instruments_before_nt_start() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml");
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&root_path).expect("multi-strategy fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register configured strategies");

    let report = check_bolt_v3_start_readiness_gate(&node, &loaded, 601_000)
        .expect("start readiness gate should return report");

    assert_eq!(node.state(), NodeState::Idle);
    assert!(!report.is_ready(), "empty NT cache must block start");
    assert_eq!(report.instrument_readiness.facts.len(), 2);

    let error = require_bolt_v3_start_readiness_gate(&node, &loaded, 601_000)
        .expect_err("missing selected-market instruments must reject production start");
    match error {
        BoltV3StartReadinessGateError::Blocked(blocked_report) => {
            assert_eq!(blocked_report, report);
        }
        BoltV3StartReadinessGateError::MarketIdentity(error) => {
            panic!("unexpected market identity error: {error}")
        }
    }
}

#[test]
fn start_readiness_gate_accepts_loaded_selected_market_before_nt_start() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml");
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&root_path).expect("strategy fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register configured strategy");
    let cache = node.kernel().cache();
    cache
        .borrow_mut()
        .add_instrument(polymarket_updown_option(
            "0xcurrent-111.POLYMARKET",
            "111",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Up",
            600_000,
            900_000,
        ))
        .unwrap();
    cache
        .borrow_mut()
        .add_instrument(polymarket_updown_option(
            "0xcurrent-222.POLYMARKET",
            "222",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Down",
            600_000,
            900_000,
        ))
        .unwrap();

    let report = check_bolt_v3_start_readiness_gate(&node, &loaded, 601_000)
        .expect("start readiness gate should return report");

    assert_eq!(node.state(), NodeState::Idle);
    assert!(report.is_ready(), "loaded selected market should pass gate");
    assert_eq!(report.instrument_readiness.facts.len(), 1);

    let required = require_bolt_v3_start_readiness_gate(&node, &loaded, 601_000)
        .expect("loaded selected-market instruments should permit production start");
    assert_eq!(required, report);
}

#[test]
fn start_readiness_gate_wiring_has_no_start_run_order_or_subscription_calls() {
    let path = support::repo_path("src/bolt_v3_start_readiness.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    for forbidden in [
        ".start(",
        ".run(",
        "submit_order",
        "submit_order_list",
        "order_factory",
        "subscribe_book",
        "subscribe_quotes",
        "subscribe_trades",
        "subscribe_instruments",
    ] {
        assert!(
            !source.contains(forbidden),
            "bolt-v3 start readiness gate must not call `{forbidden}`"
        );
    }
}
