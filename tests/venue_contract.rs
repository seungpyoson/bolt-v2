use std::collections::BTreeMap;
use std::fs::File;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

use crate::support;
use crate::support::fast_test_live_node;
use arrow::{
    datatypes::{Field, Schema},
    ipc::writer::StreamWriter,
};
use bolt_v2::{
    lake_batch::convert_live_spool_to_parquet,
    nt_runtime_capture,
    venue_contract::{
        BookDepthSource, CURRENT_SCHEMA_VERSION, Capability, CompletenessReport, FeeRateSource,
        MaintenancePolicy, Policy, Provenance, STATIC_FEE_BPS_ABSOLUTE_LIMIT,
        ScheduledMaintenanceWindow, SettlementKind, StreamContract, VenueContract, Weekday,
    },
};
use nautilus_common::msgbus::{
    publish_any, publish_deltas, publish_mark_price, publish_quote, publish_trade, switchboard,
};
use nautilus_model::{
    data::{BookOrder, MarkPriceUpdate, OrderBookDelta, OrderBookDeltas, QuoteTick, TradeTick},
    enums::{AggressorSide, BookAction, OrderSide},
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};
use tempfile::tempdir;
use tokio::task::LocalSet;

fn test_instrument_id() -> InstrumentId {
    InstrumentId::from("0xTEST.POLYMARKET")
}

fn venue_contract_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn assert_failure_report_only(path: &std::path::Path) -> CompletenessReport {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["completeness_report.json"]);

    let json = std::fs::read_to_string(path.join("completeness_report.json")).unwrap();
    let report: CompletenessReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report.outcome, "fail");
    report
}

fn write_empty_feather_stream(path: &std::path::Path) {
    let schema = Arc::new(Schema::new(Vec::<Field>::new()));
    let file = File::create(path).unwrap();
    let mut writer = StreamWriter::try_new(file, &schema).unwrap();
    writer.finish().unwrap();
}

fn flatten_spool_to_flat_layout(instance_root: &std::path::Path) {
    let class_dirs: Vec<_> = std::fs::read_dir(instance_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .collect();

    for class_dir in class_dirs {
        let class_name = class_dir.file_name().to_string_lossy().to_string();
        let mut feather_files = Vec::new();
        for entry in std::fs::read_dir(class_dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if path.is_dir() {
                for nested in std::fs::read_dir(path)
                    .unwrap()
                    .filter_map(|entry| entry.ok())
                {
                    let nested_path = nested.path();
                    if nested_path.extension().and_then(|ext| ext.to_str()) == Some("feather") {
                        feather_files.push(nested_path);
                    }
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("feather") {
                feather_files.push(path);
            }
        }

        for (idx, feather_path) in feather_files.iter().enumerate() {
            std::fs::rename(
                feather_path,
                instance_root.join(format!("{class_name}_{idx}.feather")),
            )
            .unwrap();
        }

        std::fs::remove_dir_all(class_dir.path()).unwrap();
    }
}

fn base_polymarket_streams() -> BTreeMap<String, StreamContract> {
    let supported = |policy: Policy| StreamContract {
        capability: Capability::Supported,
        policy,
        provenance: Provenance::Native,
        reason: None,
        derived_from: None,
    };
    let unsupported = || StreamContract {
        capability: Capability::Unsupported,
        policy: Policy::Disabled,
        provenance: Provenance::Native,
        reason: Some("n/a".to_string()),
        derived_from: None,
    };

    BTreeMap::from([
        ("quotes".to_string(), supported(Policy::Required)),
        ("trades".to_string(), supported(Policy::Required)),
        ("order_book_deltas".to_string(), supported(Policy::Required)),
        ("order_book_depths".to_string(), unsupported()),
        ("index_prices".to_string(), unsupported()),
        ("mark_prices".to_string(), unsupported()),
        ("instrument_closes".to_string(), unsupported()),
    ])
}

fn make_contract(streams: BTreeMap<String, StreamContract>) -> VenueContract {
    support::sample_contract_with_streams(streams)
}

/// Read the first shipped contract's text and remove the field or table named by
/// `prefix`, asserting the removal happened so a negative test can never silently
/// pass if the contract text drifts. Removes by key/table-header regardless of the
/// field's value, so it stays venue-agnostic, and is line-oriented so it is robust
/// to CRLF checkouts. Comment lines never match; a key `prefix` matches only a real
/// `prefix = ...` assignment (not a longer key name); and for a `[table]` prefix the
/// whole table body is excised, not just the header, so no orphan keys survive (the
/// error stays `missing field`, robust to a future `deny_unknown_fields`).
fn contract_text_without_line(prefix: &str) -> String {
    let text = std::fs::read_to_string(support::first_contract_path()).unwrap();
    let removing_table = prefix.starts_with('[');
    let mut kept = Vec::new();
    let mut removed = false;
    let mut dropping_table_body = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if dropping_table_body {
            if trimmed.starts_with('[') {
                dropping_table_body = false; // next table header: stop excising
            } else {
                continue; // drop the removed table's body lines
            }
        }
        let name_match = if removing_table {
            trimmed.starts_with(prefix)
        } else {
            trimmed
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        };
        if !removed && name_match && !trimmed.starts_with('#') {
            removed = true;
            dropping_table_body = removing_table;
            continue;
        }
        kept.push(line);
    }
    assert!(
        removed,
        "no `{prefix}` field/table in the shipped contract; negative test would silently pass"
    );
    let mut out = kept.join("\n");
    out.push('\n');
    out
}

#[test]
fn loads_polymarket_contract() {
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .expect("polymarket contract should load");

    assert_eq!(contract.venue, "polymarket");
    assert_eq!(contract.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(contract.streams.len(), 7);

    // Execution-capability facts (sourced from the NT Polymarket adapter).
    assert!(!contract.execution.supports_modify);
    assert_eq!(contract.rate_budget.clob_per_minute, 100);
    assert_eq!(contract.rate_budget.gamma_per_minute, 100);
    assert_eq!(contract.rate_budget.batch_submit_limit, 15);
    assert_eq!(
        contract.maintenance_window.policy,
        MaintenancePolicy::NoneConfigured
    );
    assert_eq!(contract.maintenance_window.pull_before_start_seconds, 0);
    assert_eq!(
        contract.depth_availability.book_depth_source,
        BookDepthSource::OrderBookDeltas
    );
    assert_eq!(
        contract.depth_availability.native_queue_position,
        Capability::Unsupported
    );
    assert_eq!(
        contract.fee_schedule.maker_fee_rate_source,
        FeeRateSource::Instrument
    );
    assert_eq!(
        contract.fee_schedule.taker_fee_rate_source,
        FeeRateSource::Instrument
    );
    assert_eq!(contract.fee_schedule.settlement_currency, "pUSD");
    assert_eq!(contract.settlement.kind, SettlementKind::Binary);
}

#[test]
fn rejects_contract_missing_stream_provenance() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-provenance.toml");
    std::fs::write(&path, contract_text_without_line("provenance")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("missing field `provenance`"),
        "stream provenance must be explicit, got: {err}"
    );
}

#[test]
fn rejects_contract_missing_stream_class() {
    let mut streams = base_polymarket_streams();
    streams.remove("quotes");
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("contract missing required stream class"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_contract_unknown_stream_class() {
    let mut streams = base_polymarket_streams();
    streams.insert(
        "funding_rates".to_string(),
        StreamContract {
            capability: Capability::Supported,
            policy: Policy::Required,
            provenance: Provenance::Native,
            reason: None,
            derived_from: None,
        },
    );
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("adapter does not implement stream class: funding_rates"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_unsupported_with_required_policy() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("mark_prices").unwrap().policy = Policy::Required;
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported capability must have disabled policy"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_derived_without_derived_from() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().provenance = Provenance::Derived;
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("derived provenance requires non-empty derived_from"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_wrong_schema_version() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.schema_version = 99;
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported contract schema_version"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_previous_schema_before_full_contract_parse() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy-v2-flat-fields.toml");
    let previous_schema_version = CURRENT_SCHEMA_VERSION - 1;
    let current_schema_line = format!("schema_version = {CURRENT_SCHEMA_VERSION}");
    let previous_schema_line = format!("schema_version = {previous_schema_version}");
    let text = std::fs::read_to_string(support::first_contract_path())
        .unwrap()
        .replacen(&current_schema_line, &previous_schema_line, 1)
        .replacen(
            "[execution]",
            "supports_modify = false\nsettlement_kind = \"binary\"\n\n[execution]",
            1,
        );
    std::fs::write(&path, text).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains(&format!(
            "unsupported contract schema_version {previous_schema_version}, expected {CURRENT_SCHEMA_VERSION}"
        )),
        "previous schema must be rejected before full contract parsing, got: {err}"
    );
}

#[test]
fn rejects_missing_schema_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-schema-version.toml");
    std::fs::write(&path, contract_text_without_line("schema_version")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("missing field `schema_version`"),
        "contract must fail closed without an explicit schema version, got: {err}"
    );
}

#[test]
fn rejects_future_schema_before_full_contract_parse() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("future-schema.toml");
    let future_schema_version = CURRENT_SCHEMA_VERSION + 1;
    let current_schema_line = format!("schema_version = {CURRENT_SCHEMA_VERSION}");
    let future_schema_line = format!("schema_version = {future_schema_version}");
    let text = std::fs::read_to_string(support::first_contract_path())
        .unwrap()
        .replacen(&current_schema_line, &future_schema_line, 1);
    std::fs::write(&path, text).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains(&format!(
            "unsupported contract schema_version {future_schema_version}, expected {CURRENT_SCHEMA_VERSION}"
        )),
        "future schema must be rejected before full contract parsing, got: {err}"
    );
}

#[test]
fn rejects_zero_rate_budget() {
    fn assert_zero_rejected(zero_field: impl FnOnce(&mut VenueContract), name: &str) {
        let mut contract = make_contract(base_polymarket_streams());
        zero_field(&mut contract);
        let err = contract.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("rate_budget.{name} must be positive")),
            "expected positivity error for {name}, got: {err}"
        );
    }

    assert_zero_rejected(|c| c.rate_budget.clob_per_minute = 0, "clob_per_minute");
    assert_zero_rejected(|c| c.rate_budget.gamma_per_minute = 0, "gamma_per_minute");
    assert_zero_rejected(
        |c| c.rate_budget.batch_submit_limit = 0,
        "batch_submit_limit",
    );
}

#[test]
fn rejects_contract_missing_execution() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-execution.toml");
    std::fs::write(&path, contract_text_without_line("[execution]")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("missing field `execution`"),
        "execution capability section must be explicit, got: {err}"
    );
}

#[test]
fn rejects_contract_missing_settlement() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-settlement.toml");
    std::fs::write(&path, contract_text_without_line("[settlement]")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("missing field `settlement`"),
        "settlement section must be explicit, got: {err}"
    );
}

#[test]
fn rejects_legacy_flat_capability_fields() {
    let cases = [
        ("supports_modify", "supports_modify = false\n\n[execution]"),
        (
            "settlement_kind",
            "settlement_kind = \"binary\"\n\n[execution]",
        ),
    ];

    for (field, replacement) in cases {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("legacy-flat-{field}.toml"));
        let text = std::fs::read_to_string(support::first_contract_path())
            .unwrap()
            .replacen("[execution]", replacement, 1);
        std::fs::write(&path, text).unwrap();

        let err = VenueContract::load_and_validate(&path).unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains(&format!("unknown field `{field}`")),
            "legacy flat capability field {field} must be rejected, got: {err}"
        );
    }
}

#[test]
fn rejects_contract_missing_rate_budget() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-rate-budget.toml");
    std::fs::write(&path, contract_text_without_line("[rate_budget]")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("missing field `rate_budget`"),
        "rate_budget must be explicit, got: {err}"
    );
}

#[test]
fn rejects_contract_missing_maintenance_window() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-maintenance-window.toml");
    std::fs::write(&path, contract_text_without_line("[maintenance_window]")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string()
            .contains("missing field `maintenance_window`"),
        "maintenance_window section must be explicit, got: {err}"
    );
}

#[test]
fn rejects_contract_missing_depth_availability() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-depth-availability.toml");
    std::fs::write(&path, contract_text_without_line("[depth_availability]")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string()
            .contains("missing field `depth_availability`"),
        "depth_availability section must be explicit, got: {err}"
    );
}

#[test]
fn rejects_contract_missing_fee_schedule() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing-fee-schedule.toml");
    std::fs::write(&path, contract_text_without_line("[fee_schedule]")).unwrap();

    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("missing field `fee_schedule`"),
        "fee_schedule section must be explicit, got: {err}"
    );
}

#[test]
fn rejects_scheduled_maintenance_without_pull_lead() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::Scheduled;
    contract.maintenance_window.pull_before_start_seconds = 0;

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("scheduled maintenance requires positive pull_before_start_seconds"),
        "scheduled maintenance must fail closed without a pull lead, got: {err}"
    );
}

#[test]
fn rejects_scheduled_maintenance_without_windows() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::Scheduled;
    contract.maintenance_window.pull_before_start_seconds = 60;

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("scheduled maintenance requires at least one window"),
        "scheduled maintenance must declare concrete windows, got: {err}"
    );
}

#[test]
fn rejects_none_configured_maintenance_with_pull_lead() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::NoneConfigured;
    contract.maintenance_window.pull_before_start_seconds = 60;

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string().contains(
            "maintenance_window.pull_before_start_seconds must be 0 when policy is none_configured"
        ),
        "none_configured maintenance must reject a pull lead, got: {err}"
    );
}

#[test]
fn rejects_none_configured_maintenance_with_windows() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::NoneConfigured;
    contract.maintenance_window.pull_before_start_seconds = 0;
    contract
        .maintenance_window
        .windows
        .push(ScheduledMaintenanceWindow {
            weekday: Weekday::Sunday,
            start_time_utc: "04:00".to_string(),
            duration_seconds: 900,
        });

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("maintenance_window.windows must be empty when policy is none_configured"),
        "none_configured maintenance must reject concrete windows, got: {err}"
    );
}

#[test]
fn scheduled_maintenance_window_with_positive_duration_validates() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::Scheduled;
    contract.maintenance_window.pull_before_start_seconds = 60;
    contract
        .maintenance_window
        .windows
        .push(ScheduledMaintenanceWindow {
            weekday: Weekday::Sunday,
            start_time_utc: "04:00".to_string(),
            duration_seconds: 900,
        });

    contract
        .validate()
        .expect("scheduled window with pull lead and duration should validate");
}

#[test]
fn rejects_scheduled_maintenance_zero_duration() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::Scheduled;
    contract.maintenance_window.pull_before_start_seconds = 60;
    contract
        .maintenance_window
        .windows
        .push(ScheduledMaintenanceWindow {
            weekday: Weekday::Sunday,
            start_time_utc: "04:00".to_string(),
            duration_seconds: 0,
        });

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("maintenance_window.windows[0].duration_seconds must be positive"),
        "scheduled maintenance duration must fail closed, got: {err}"
    );
}

#[test]
fn rejects_malformed_maintenance_start_time() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.maintenance_window.policy = MaintenancePolicy::Scheduled;
    contract.maintenance_window.pull_before_start_seconds = 60;
    contract
        .maintenance_window
        .windows
        .push(ScheduledMaintenanceWindow {
            weekday: Weekday::Sunday,
            start_time_utc: "4am".to_string(),
            duration_seconds: 900,
        });

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("maintenance_window.windows[0].start_time_utc must be HH:MM"),
        "maintenance start time must fail closed, got: {err}"
    );
}

#[test]
fn rejects_maintenance_start_time_boundaries() {
    for bad_time in ["24:00", "23:60", "1:02", "12:5", "2a:00", "12:a0"] {
        let mut contract = make_contract(base_polymarket_streams());
        contract.maintenance_window.policy = MaintenancePolicy::Scheduled;
        contract.maintenance_window.pull_before_start_seconds = 60;
        contract
            .maintenance_window
            .windows
            .push(ScheduledMaintenanceWindow {
                weekday: Weekday::Sunday,
                start_time_utc: bad_time.to_string(),
                duration_seconds: 900,
            });

        let err = contract.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("maintenance_window.windows[0].start_time_utc must be HH:MM"),
            "maintenance start time {bad_time} must fail closed, got: {err}"
        );
    }
}

#[test]
fn rejects_depth_source_missing_stream_contract() {
    let mut streams = base_polymarket_streams();
    streams.remove("order_book_deltas");
    let contract = make_contract(streams);

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string().contains(
            "depth_availability.book_depth_source references missing stream order_book_deltas"
        ),
        "depth source must require an explicit stream contract, got: {err}"
    );
}

#[test]
fn rejects_depth_source_not_supported_by_stream_contract() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.depth_availability.book_depth_source = BookDepthSource::OrderBookDepths;

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string().contains(
            "depth_availability.book_depth_source references unsupported stream order_book_depths"
        ),
        "depth source must agree with stream capabilities, got: {err}"
    );
}

#[test]
fn rejects_depth_source_disabled_by_stream_contract() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("order_book_deltas").unwrap().policy = Policy::Disabled;
    let contract = make_contract(streams);

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string().contains(
            "depth_availability.book_depth_source references disabled stream order_book_deltas"
        ),
        "depth source must require an enabled stream, got: {err}"
    );
}

#[test]
fn rejects_derived_from_unsupported_source_stream() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().provenance = Provenance::Derived;
    streams.get_mut("quotes").unwrap().derived_from = Some(vec!["mark_prices".to_string()]);
    let contract = make_contract(streams);

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("stream quotes: derived_from references mark_prices which is not supported"),
        "derived stream must reject unsupported source stream, got: {err}"
    );
}

#[test]
fn rejects_contract_fee_source_without_static_bps() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.fee_schedule.maker_fee_rate_source = FeeRateSource::Contract;
    contract.fee_schedule.maker_fee_bps = None;

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("fee_schedule.maker_fee_bps required when maker_fee_rate_source is contract"),
        "contract-sourced maker fees must provide static bps, got: {err}"
    );
}

#[test]
fn rejects_taker_contract_fee_source_without_static_bps() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.fee_schedule.taker_fee_rate_source = FeeRateSource::Contract;
    contract.fee_schedule.taker_fee_bps = None;

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("fee_schedule.taker_fee_bps required when taker_fee_rate_source is contract"),
        "contract-sourced taker fees must provide static bps, got: {err}"
    );
}

#[test]
fn contract_fee_source_with_bounded_static_bps_validates() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.fee_schedule.maker_fee_rate_source = FeeRateSource::Contract;
    contract.fee_schedule.maker_fee_bps = Some(-(STATIC_FEE_BPS_ABSOLUTE_LIMIT as i32));
    contract.fee_schedule.taker_fee_rate_source = FeeRateSource::Contract;
    contract.fee_schedule.taker_fee_bps = Some(STATIC_FEE_BPS_ABSOLUTE_LIMIT as i32);

    contract
        .validate()
        .expect("contract-sourced static bps inside bounds should validate");
}

#[test]
fn rejects_contract_fee_source_with_out_of_bounds_static_bps() {
    fn assert_out_of_bounds_rejected(update: impl FnOnce(&mut VenueContract), side: &str) {
        let mut contract = make_contract(base_polymarket_streams());
        update(&mut contract);

        let err = contract.validate().unwrap_err();
        assert!(
            err.to_string().contains(&format!(
                "fee_schedule.{side}_fee_bps must be within {STATIC_FEE_BPS_ABSOLUTE_LIMIT} bps of zero"
            )),
            "out-of-bounds {side} fee bps must fail closed, got: {err}"
        );
    }

    assert_out_of_bounds_rejected(
        |contract| {
            contract.fee_schedule.maker_fee_rate_source = FeeRateSource::Contract;
            contract.fee_schedule.maker_fee_bps = Some(STATIC_FEE_BPS_ABSOLUTE_LIMIT as i32 + 1);
        },
        "maker",
    );
    assert_out_of_bounds_rejected(
        |contract| {
            contract.fee_schedule.taker_fee_rate_source = FeeRateSource::Contract;
            contract.fee_schedule.taker_fee_bps =
                Some(-((STATIC_FEE_BPS_ABSOLUTE_LIMIT as i32) + 1));
        },
        "taker",
    );
}

#[test]
fn rejects_instrument_fee_source_with_static_bps() {
    fn assert_static_bps_rejected(update: impl FnOnce(&mut VenueContract), side: &str) {
        let mut contract = make_contract(base_polymarket_streams());
        update(&mut contract);

        let err = contract.validate().unwrap_err();
        assert!(
            err.to_string().contains(&format!(
                "fee_schedule.{side}_fee_bps must be absent when {side}_fee_rate_source is instrument"
            )),
            "instrument-sourced {side} fees must not provide static bps, got: {err}"
        );
    }

    assert_static_bps_rejected(
        |contract| contract.fee_schedule.maker_fee_bps = Some(5),
        "maker",
    );
    assert_static_bps_rejected(
        |contract| contract.fee_schedule.taker_fee_bps = Some(5),
        "taker",
    );
}

#[test]
fn rejects_empty_fee_schedule_currency() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.fee_schedule.settlement_currency.clear();

    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("fee_schedule.settlement_currency must be non-empty"),
        "fee schedule currency must fail closed, got: {err}"
    );
}

#[test]
fn rejects_malformed_toml() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "this is not valid toml [[[").unwrap();
    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("failed to parse contract"),
        "unexpected error: {err}"
    );
}

#[test]
fn contract_happy_path_polymarket() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            let deltas = OrderBookDeltas::new(inst, vec![delta]);
            publish_deltas(switchboard::get_book_deltas_topic(inst), &deltas);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let report = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap();

    let cr = report.completeness;
    assert_eq!(cr.outcome, "pass");
    assert_eq!(cr.classes["quotes"].status, "pass");
    assert_eq!(cr.classes["trades"].status, "pass");
    assert_eq!(cr.classes["order_book_deltas"].status, "pass");
    assert_eq!(cr.classes["order_book_depths"].status, "pass_unsupported");
    assert_eq!(cr.classes["index_prices"].status, "pass_unsupported");
    assert_eq!(cr.classes["mark_prices"].status, "pass_unsupported");
    assert_eq!(cr.classes["instrument_closes"].status, "pass_unsupported");

    let report_path = output_root.join("completeness_report.json");
    assert!(report_path.exists());
    let json_str = std::fs::read_to_string(&report_path).unwrap();
    let from_disk: CompletenessReport = serde_json::from_str(&json_str).unwrap();
    assert_eq!(from_disk.outcome, "pass");
}

#[test]
fn contract_fails_when_required_class_absent() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);
            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_required_absent"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(
        report.classes["order_book_deltas"].status,
        "fail_required_absent"
    );

    let retry_error = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();
    assert!(
        retry_error
            .to_string()
            .contains("output_root must be empty before conversion"),
        "{retry_error:?}"
    );
    let retry_report = assert_failure_report_only(&output_root);
    assert_eq!(
        retry_report.classes["order_book_deltas"].status,
        "fail_required_absent"
    );
}

#[test]
fn contract_failure_uses_preexisting_empty_output_root() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    std::fs::create_dir_all(&output_root).unwrap();
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);
            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(
        report.classes["order_book_deltas"].status,
        "fail_required_absent"
    );
}

#[test]
fn contract_fails_when_disabled_supported_stream_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().policy = Policy::Disabled;
    streams.get_mut("trades").unwrap().policy = Policy::Optional;
    streams.get_mut("order_book_deltas").unwrap().policy = Policy::Optional;
    let contract = make_contract(streams);

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;
            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_contract_violation"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(report.classes["quotes"].status, "fail_contract_violation");
}

#[test]
fn contract_fails_when_disabled_conditional_stream_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().capability = Capability::Conditional;
    streams.get_mut("quotes").unwrap().policy = Policy::Disabled;
    streams.get_mut("trades").unwrap().policy = Policy::Optional;
    streams.get_mut("order_book_deltas").unwrap().policy = Policy::Optional;
    let contract = make_contract(streams);

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;
            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_contract_violation"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(report.classes["quotes"].status, "fail_contract_violation");
}

#[test]
fn contract_allows_optional_class_with_spool_present_but_no_converted_rows() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().policy = Policy::Optional;
    streams.get_mut("trades").unwrap().policy = Policy::Disabled;
    streams.get_mut("order_book_deltas").unwrap().policy = Policy::Disabled;
    let contract = make_contract(streams);

    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let instance_root = catalog_root.join("live").join("instance-optional-empty");
    let quotes_root = instance_root.join("quotes");
    std::fs::create_dir_all(&quotes_root).unwrap();
    write_empty_feather_stream(&quotes_root.join("quotes_0.feather"));

    let report = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        "instance-optional-empty",
        &output_root,
        &contract,
    )
    .unwrap();

    let cr = report.completeness;
    assert_eq!(cr.outcome, "pass");
    assert_eq!(
        cr.classes["quotes"].status,
        "spool_present_conversion_empty"
    );
    assert!(cr.classes["quotes"].spool_present);
}

#[test]
fn contract_fails_when_unsupported_class_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            let mark = MarkPriceUpdate::new(inst, Price::from("0.55"), ts.into(), ts.into());
            publish_mark_price(switchboard::get_mark_price_topic(inst), &mark);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_contract_violation"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(
        report.classes["mark_prices"].status,
        "fail_contract_violation"
    );
}

#[test]
fn contract_fails_when_unknown_class_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        let catalog_root_clone = catalog_root.clone();
        let instance_id_clone = instance_id.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            let fake_dir = catalog_root_clone
                .join("live")
                .join(&instance_id_clone)
                .join("funding_rates");
            std::fs::create_dir_all(&fake_dir).unwrap();
            std::fs::write(fake_dir.join("dummy.feather"), b"fake feather content").unwrap();

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_unknown"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(report.classes["funding_rates"].status, "fail_unknown");
}

#[test]
fn contract_fails_when_unknown_flat_file_has_data() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        let catalog_root_clone = catalog_root.clone();
        let instance_id_clone = instance_id.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            std::fs::write(
                catalog_root_clone
                    .join("live")
                    .join(&instance_id_clone)
                    .join("bars_123.feather"),
                b"fake feather content",
            )
            .unwrap();

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_unknown"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(
        report.classes["flat_file:bars_123.feather"].status,
        "fail_unknown"
    );
}

fn assert_contract_rejects_flat_instruments_file(file_name: &str) {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();
    let file_name = file_name.to_string();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        let catalog_root_clone = catalog_root.clone();
        let instance_id_clone = instance_id.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            std::fs::write(
                catalog_root_clone
                    .join("live")
                    .join(&instance_id_clone)
                    .join(file_name),
                b"fake feather content",
            )
            .unwrap();

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let error = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("contract validation failed"), "{message}");
    assert!(
        message.contains("flat spool files are not supported; use class directories"),
        "{message}"
    );
}

#[test]
fn contract_rejects_flat_instruments_file() {
    assert_contract_rejects_flat_instruments_file("instruments_123.feather");
}

#[test]
fn contract_rejects_bare_flat_instruments_file() {
    assert_contract_rejects_flat_instruments_file("instruments.feather");
}

#[test]
fn contract_ignores_status_directory_infrastructure() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            let status = nautilus_model::data::InstrumentStatus::new(
                inst,
                nautilus_model::enums::MarketStatusAction::Close,
                ts.into(),
                ts.into(),
                None,
                None,
                Some(false),
                None,
                None,
            );
            publish_any(switchboard::get_instrument_status_topic(inst), &status);

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let status_dir = catalog_root.join("live").join(&instance_id).join("status");
    assert!(
        status_dir.is_dir(),
        "expected status infrastructure directory at {}",
        status_dir.display()
    );
    assert!(
        std::fs::read_dir(&status_dir)
            .expect("status directory should be readable")
            .next()
            .is_some(),
        "expected status infrastructure directory to be non-empty at {}",
        status_dir.display()
    );

    let report = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap();

    let cr = report.completeness;
    assert_eq!(cr.outcome, "pass");
    assert!(
        !cr.classes.contains_key("status"),
        "status directory should be ignored silently, completeness={cr:?}"
    );
}

#[test]
fn contract_fails_when_flat_status_file_is_present() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        let catalog_root_clone = catalog_root.clone();
        let instance_id_clone = instance_id.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            std::fs::write(
                catalog_root_clone
                    .join("live")
                    .join(&instance_id_clone)
                    .join("status_123.feather"),
                b"fake feather content",
            )
            .unwrap();

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let err = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("contract validation failed"), "{msg}");
    assert!(msg.contains("fail_unknown"), "{msg}");
    let report = assert_failure_report_only(&output_root);
    assert_eq!(
        report.classes["flat_file:status_123.feather"].status,
        "fail_unknown"
    );
}

#[test]
fn contract_rejects_flat_multiword_classes() {
    let _guard = venue_contract_test_lock().lock().unwrap();
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .unwrap();

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source_dir = tempdir().unwrap();
    let output_dir = tempdir().unwrap();
    let output_root = output_dir.path().join("contract-output");
    let catalog_root = source_dir.path().join("catalog");
    let inst = test_instrument_id();

    let instance_id = runtime.block_on(local.run_until(async {
        let mut node = fast_test_live_node();
        let handle = node.handle();
        let instance_id = node.instance_id().to_string();

        let guards = nt_runtime_capture::wire_nt_runtime_capture(
            &node,
            handle.clone(),
            catalog_root.to_str().unwrap(),
            60_000,
            50,
            None,
        )
        .unwrap();

        let publisher_handle = handle.clone();
        tokio::task::spawn_local(async move {
            while !publisher_handle.is_running() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let ts = 1_000_000_000u64;

            let quote = QuoteTick::new(
                inst,
                Price::from("0.55"),
                Price::from("0.56"),
                Quantity::from("100"),
                Quantity::from("100"),
                ts.into(),
                ts.into(),
            );
            publish_quote(switchboard::get_quotes_topic(inst), &quote);

            let trade = TradeTick {
                instrument_id: inst,
                price: Price::from("0.55"),
                size: Quantity::from("10"),
                aggressor_side: AggressorSide::Buy,
                trade_id: TradeId::new("T1"),
                ts_event: ts.into(),
                ts_init: ts.into(),
            };
            publish_trade(switchboard::get_trades_topic(inst), &trade);

            let delta = OrderBookDelta::new(
                inst,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.54"), Quantity::from("50"), 1),
                0,
                0,
                ts.into(),
                ts.into(),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(inst),
                &OrderBookDeltas::new(inst, vec![delta]),
            );

            publisher_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();
        instance_id
    }));

    let instance_root = catalog_root.join("live").join(&instance_id);
    flatten_spool_to_flat_layout(&instance_root);

    let error = convert_live_spool_to_parquet(
        catalog_root.as_path(),
        &instance_id,
        &output_root,
        &contract,
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("contract validation failed"), "{message}");
    assert!(
        message.contains("flat spool files are not supported; use class directories"),
        "{message}"
    );
}
