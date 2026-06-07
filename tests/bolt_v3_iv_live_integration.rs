use std::{fs, path::Path};

use bolt_v2::{
    bolt_v3_config::BoltV3RootConfig,
    bolt_v3_iv::{
        health::IvSourceHealthState,
        ingest::{IvBasisValue, IvGreekValues, IvIngestEvent, IvOptionGreeksPayload, IvRawPayload},
        store::{IvRetentionPolicy, IvStore},
        time::UnixNanos,
        types::{IvBasis, IvConvention, IvSourceKind},
    },
};

fn repo_path(relative: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(relative)
        .to_string_lossy()
        .to_string()
}

fn greeks_event(ts: u64) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(ts),
        ts_init_ns: Some(UnixNanos::new(ts.saturating_sub(1))),
        received_ts_ns: UnixNanos::new(ts + 1),
        subscription_generation: 1,
        source_health_state: IvSourceHealthState::Active,
        payload: IvRawPayload::OptionGreeks(IvOptionGreeksPayload {
            instrument_id: "configured-option-instrument".to_string(),
            convention: IvConvention::Named("configured-convention".to_string()),
            basis_values: vec![IvBasisValue {
                basis: IvBasis::Mark,
                iv: 0.40,
            }],
            greeks: IvGreekValues {
                delta: None,
                gamma: None,
                vega: None,
                theta: None,
                rho: None,
            },
            underlying_price: None,
            open_interest: None,
        }),
    }
}

#[test]
fn source_health_and_retention_eviction_keep_current_views_bounded() {
    assert!(IvSourceHealthState::Configured.can_transition_to(IvSourceHealthState::Subscribing));
    assert!(IvSourceHealthState::Subscribing.can_transition_to(IvSourceHealthState::Active));
    assert!(IvSourceHealthState::Active.can_transition_to(IvSourceHealthState::Stale));
    assert!(!IvSourceHealthState::Removed.can_satisfy_current_query());

    let mut store = IvStore::default();
    store.ingest_event(greeks_event(1_000)).unwrap();
    store.ingest_event(greeks_event(2_000)).unwrap();
    store.ingest_event(greeks_event(3_000)).unwrap();

    store.enforce_retention(&IvRetentionPolicy {
        max_raw_events: 2,
        max_indexed_points: 2,
        max_smiles: 2,
        max_surfaces: 2,
        max_source_health_events: 2,
    });

    assert_eq!(store.raw_events().len(), 2);
    assert_eq!(store.iv_points().len(), 2);
    assert_eq!(
        store.raw_events()[0].provenance.ts_event_ns,
        UnixNanos::new(2_000)
    );
}

#[test]
fn root_config_accepts_iv_profile_block() {
    let root = fs::read_to_string(repo_path("tests/fixtures/bolt_v3/root.toml")).unwrap();
    let with_iv = format!(
        "{root}\n{}",
        r#"
[iv]
schema_version = 1

[[iv.profiles]]
profile_id = "configured-profile"
enabled_products = ["iv_point", "source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_source_health_events = 2

[[iv.profiles.sources]]
source_id = "configured-greeks-source"
source_kind = "option_greeks"
client_id = "configured-client"
accepted_conventions = ["configured-convention"]

[iv.profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["configured-instrument"]

[iv.profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[iv.profiles.sources.params]
configured_source_param = "configured-value"
"#
    );

    let parsed: BoltV3RootConfig = toml::from_str(&with_iv).unwrap();

    assert!(parsed.iv.is_some());
    assert_eq!(
        parsed.iv.unwrap().profiles[0].profile_id,
        "configured-profile"
    );
}
