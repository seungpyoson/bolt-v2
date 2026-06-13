use bolt_v2::bolt_v3_iv::{
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    health::IvSourceHealthState,
    ingest::{
        IvAggregateGreeksPayload, IvBasisValue, IvCustomIvPayload, IvGreekValues, IvIngestEvent,
        IvOptionChainQuotePayload, IvOptionChainSlicePayload, IvOptionChainStrikePayload,
        IvOptionGreeksPayload, IvRawPayload,
    },
    provenance::validate_iv_provenance,
    store::{IvStore, IvStoreError},
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

fn configured_convention() -> IvConvention {
    IvConvention::Named("configured-convention".to_string())
}

fn input_bounds(inclusive_max: f64) -> IvNumericBounds {
    IvNumericBounds {
        finite_required: true,
        positive_required: true,
        inclusive_min: Some(0.0),
        inclusive_max: Some(inclusive_max),
        exclusive_min: None,
        exclusive_max: None,
        unit: IvBoundUnit::Unitless,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: [configured_convention()].into_iter().collect(),
        },
    }
}

fn base_event(source_kind: IvSourceKind, payload: IvRawPayload) -> IvIngestEvent {
    IvIngestEvent {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-source".to_string(),
        source_kind,
        selector_fingerprint: "configured-selector-fingerprint".to_string(),
        nt_revision: "configured-nt-revision".to_string(),
        nt_evidence_path: "configured/nt/evidence/path.rs".to_string(),
        nt_symbol: "ConfiguredNtSymbol".to_string(),
        ts_event_ns: UnixNanos::new(1_000),
        ts_init_ns: Some(UnixNanos::new(900)),
        received_ts_ns: UnixNanos::new(1_100),
        subscription_generation: 12,
        source_health_state: IvSourceHealthState::Active,
        payload,
    }
}

fn greeks_payload() -> IvOptionGreeksPayload {
    IvOptionGreeksPayload {
        instrument_id: "configured-option-instrument".to_string(),
        convention: configured_convention(),
        basis_values: vec![
            IvBasisValue {
                basis: IvBasis::Mark,
                iv: 0.42,
            },
            IvBasisValue {
                basis: IvBasis::Bid,
                iv: 0.41,
            },
            IvBasisValue {
                basis: IvBasis::Ask,
                iv: 0.43,
            },
        ],
        greeks: IvGreekValues {
            delta: Some(0.51),
            gamma: Some(0.02),
            vega: Some(0.13),
            theta: Some(-0.04),
            rho: Some(0.01),
        },
        underlying_price: Some(101.25),
        open_interest: Some(1200.0),
    }
}

fn chain_greeks_payload(instrument_id: &str, mark_iv: f64) -> IvOptionGreeksPayload {
    IvOptionGreeksPayload {
        instrument_id: instrument_id.to_string(),
        convention: configured_convention(),
        basis_values: vec![IvBasisValue {
            basis: IvBasis::Mark,
            iv: mark_iv,
        }],
        greeks: IvGreekValues {
            delta: Some(0.51),
            gamma: Some(0.02),
            vega: Some(0.13),
            theta: Some(-0.04),
            rho: Some(0.01),
        },
        underlying_price: Some(101.25),
        open_interest: Some(1200.0),
    }
}

fn chain_quote_payload(instrument_id: &str) -> IvOptionChainQuotePayload {
    IvOptionChainQuotePayload {
        instrument_id: instrument_id.to_string(),
        bid_price: Some(4.1),
        ask_price: Some(4.3),
        bid_size: Some(12.0),
        ask_size: Some(13.0),
        ts_event_ns: UnixNanos::new(950),
        ts_init_ns: Some(UnixNanos::new(960)),
    }
}

#[test]
fn option_greeks_ingest_rejects_values_outside_configured_input_bounds() {
    let mut payload = greeks_payload();
    payload.basis_values[0].iv = 0.61;
    let mut store = IvStore::with_input_bounds(input_bounds(0.60));

    let result = store.ingest_event(base_event(
        IvSourceKind::OptionGreeks,
        IvRawPayload::OptionGreeks(payload.clone()),
    ));

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert_eq!(
        store.raw_events()[0].payload,
        IvRawPayload::OptionGreeks(payload)
    );
    assert!(store.iv_points().is_empty());
    assert!(store.greeks_points().is_empty());
}

#[test]
fn option_greeks_raw_payload_is_preserved_and_indexed_by_basis() {
    let payload = IvRawPayload::OptionGreeks(greeks_payload());
    let mut store = IvStore::empty();

    let raw = store
        .ingest_event(base_event(IvSourceKind::OptionGreeks, payload.clone()))
        .unwrap();

    assert_eq!(store.raw_events()[0].payload, payload);
    assert_eq!(raw.payload, payload);
    assert_eq!(store.iv_points().len(), 3);
    assert_eq!(store.greeks_points().len(), 3);

    let mark = store
        .iv_points()
        .iter()
        .find(|point| point.basis == IvBasis::Mark)
        .unwrap();
    assert_eq!(mark.iv, 0.42);
    assert_eq!(mark.instrument_id, "configured-option-instrument");
    assert_eq!(
        mark.convention,
        IvConvention::Named("configured-convention".to_string())
    );
    assert_eq!(
        mark.provenance.raw_event_id.as_deref(),
        Some(raw.raw_event_id.as_str())
    );
    validate_iv_provenance(&mark.provenance).unwrap();

    let greeks = store
        .greeks_points()
        .iter()
        .find(|point| point.point.basis == IvBasis::Ask)
        .unwrap();
    assert_eq!(greeks.point.iv, 0.43);
    assert_eq!(greeks.greeks.delta, Some(0.51));
    assert_eq!(greeks.underlying_price, Some(101.25));
    assert_eq!(greeks.open_interest, Some(1200.0));
}

#[test]
fn option_chain_slices_build_smiles_and_surface_views_without_interpolation() {
    let mut store = IvStore::empty();

    for series_id in ["configured-series-a", "configured-series-b"] {
        store
            .ingest_event(base_event(
                IvSourceKind::OptionChain,
                IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
                    series_id: series_id.to_string(),
                    surface_selector: "configured-surface-selector".to_string(),
                    atm_strike: Some(100.0),
                    calls: vec![
                        IvOptionChainStrikePayload {
                            strike: 90.0,
                            quote: chain_quote_payload("configured-call-90"),
                            greeks: Some(chain_greeks_payload("configured-call-90", 0.32)),
                        },
                        IvOptionChainStrikePayload {
                            strike: 100.0,
                            quote: chain_quote_payload("configured-call-100"),
                            greeks: Some(chain_greeks_payload("configured-call-100", 0.35)),
                        },
                    ],
                    puts: Vec::new(),
                }),
            ))
            .unwrap();
    }

    assert_eq!(store.raw_events().len(), 2);
    assert_eq!(store.smiles().len(), 2);
    let IvRawPayload::OptionChainSlice(raw_chain) = &store.raw_events()[0].payload else {
        panic!("expected raw option-chain slice");
    };
    assert_eq!(raw_chain.atm_strike, Some(100.0));
    assert_eq!(raw_chain.calls.len(), 2);
    assert!(raw_chain.puts.is_empty());
    assert_eq!(raw_chain.calls[0].quote.bid_price, Some(4.1));
    assert_eq!(
        raw_chain.calls[0].greeks.as_ref().unwrap().basis_values[0].iv,
        0.32
    );

    let first_smile = store
        .smiles()
        .iter()
        .find(|smile| smile.series_id == "configured-series-a")
        .unwrap();
    assert_eq!(first_smile.points_by_strike.len(), 2);
    assert_eq!(first_smile.side, "call");
    assert_eq!(first_smile.basis, IvBasis::Mark);
    assert_eq!(first_smile.atm_strike, Some(100.0));
    assert_eq!(first_smile.points_by_strike[0].strike, 90.0);
    assert_eq!(first_smile.points_by_strike[0].iv, 0.32);
    validate_iv_provenance(&first_smile.provenance).unwrap();

    let surface = store
        .surface(
            "configured-surface-selector",
            "configured-source",
            IvBasis::Mark,
            UnixNanos::new(1_000),
        )
        .unwrap();
    assert_eq!(surface.smiles.len(), 2);
    assert!(
        surface
            .smiles
            .iter()
            .all(|smile| smile.surface_selector == "configured-surface-selector")
    );
    validate_iv_provenance(&surface.provenance).unwrap();
}

#[test]
fn option_chain_strikes_with_empty_nested_basis_values_are_skipped() {
    let mut store = IvStore::empty();
    let mut empty_basis_greeks = chain_greeks_payload("configured-call-empty", 0.32);
    empty_basis_greeks.basis_values.clear();

    store
        .ingest_event(base_event(
            IvSourceKind::OptionChain,
            IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
                series_id: "configured-series-a".to_string(),
                surface_selector: "configured-surface-selector".to_string(),
                atm_strike: Some(100.0),
                calls: vec![
                    IvOptionChainStrikePayload {
                        strike: 90.0,
                        quote: chain_quote_payload("configured-call-empty"),
                        greeks: Some(empty_basis_greeks),
                    },
                    IvOptionChainStrikePayload {
                        strike: 100.0,
                        quote: chain_quote_payload("configured-call-valid"),
                        greeks: Some(chain_greeks_payload("configured-call-valid", 0.35)),
                    },
                ],
                puts: Vec::new(),
            }),
        ))
        .unwrap();

    assert_eq!(store.raw_events().len(), 1);
    assert_eq!(store.smiles().len(), 1);
    assert_eq!(store.smiles()[0].points_by_strike.len(), 1);
    assert_eq!(store.smiles()[0].points_by_strike[0].strike, 100.0);
    assert_eq!(store.smiles()[0].points_by_strike[0].iv, 0.35);
}

#[test]
fn option_chain_strikes_with_invalid_nested_iv_skip_bad_strike_and_index_usable_points() {
    let mut store = IvStore::empty();

    store
        .ingest_event(base_event(
            IvSourceKind::OptionChain,
            IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
                series_id: "configured-series-a".to_string(),
                surface_selector: "configured-surface-selector".to_string(),
                atm_strike: Some(100.0),
                calls: vec![
                    IvOptionChainStrikePayload {
                        strike: 90.0,
                        quote: chain_quote_payload("configured-call-invalid"),
                        greeks: Some(chain_greeks_payload("configured-call-invalid", f64::NAN)),
                    },
                    IvOptionChainStrikePayload {
                        strike: 100.0,
                        quote: chain_quote_payload("configured-call-valid"),
                        greeks: Some(chain_greeks_payload("configured-call-valid", 0.35)),
                    },
                ],
                puts: Vec::new(),
            }),
        ))
        .unwrap();

    assert_eq!(store.raw_events().len(), 1);
    assert_eq!(store.smiles().len(), 1);
    assert_eq!(store.smiles()[0].points_by_strike.len(), 1);
    assert_eq!(store.smiles()[0].points_by_strike[0].strike, 100.0);
    assert_eq!(store.smiles()[0].points_by_strike[0].iv, 0.35);
}

#[test]
fn option_chain_strikes_with_non_finite_strikes_preserve_raw_event_without_indexing_smiles() {
    let mut store = IvStore::empty();

    let result = store.ingest_event(base_event(
        IvSourceKind::OptionChain,
        IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
            series_id: "configured-series-a".to_string(),
            surface_selector: "configured-surface-selector".to_string(),
            atm_strike: Some(100.0),
            calls: vec![
                IvOptionChainStrikePayload {
                    strike: f64::NAN,
                    quote: chain_quote_payload("configured-call-invalid-strike"),
                    greeks: Some(chain_greeks_payload("configured-call-invalid-strike", 0.32)),
                },
                IvOptionChainStrikePayload {
                    strike: 100.0,
                    quote: chain_quote_payload("configured-call-valid"),
                    greeks: Some(chain_greeks_payload("configured-call-valid", 0.35)),
                },
            ],
            puts: Vec::new(),
        }),
    ));

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.smiles().is_empty());
}

#[test]
fn option_chain_strikes_with_missing_nested_greeks_skip_bad_strike_and_index_usable_points() {
    let mut store = IvStore::empty();

    store
        .ingest_event(base_event(
            IvSourceKind::OptionChain,
            IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
                series_id: "configured-series-a".to_string(),
                surface_selector: "configured-surface-selector".to_string(),
                atm_strike: Some(100.0),
                calls: vec![
                    IvOptionChainStrikePayload {
                        strike: 90.0,
                        quote: chain_quote_payload("configured-call-missing-greeks"),
                        greeks: None,
                    },
                    IvOptionChainStrikePayload {
                        strike: 100.0,
                        quote: chain_quote_payload("configured-call-valid"),
                        greeks: Some(chain_greeks_payload("configured-call-valid", 0.35)),
                    },
                ],
                puts: Vec::new(),
            }),
        ))
        .unwrap();

    assert_eq!(store.raw_events().len(), 1);
    assert_eq!(store.smiles().len(), 1);
    assert_eq!(store.smiles()[0].points_by_strike.len(), 1);
    assert_eq!(store.smiles()[0].points_by_strike[0].strike, 100.0);
    assert_eq!(store.smiles()[0].points_by_strike[0].iv, 0.35);
}

#[test]
fn option_chain_with_non_finite_atm_strike_preserves_raw_event_without_indexing_smiles() {
    let mut store = IvStore::empty();

    let result = store.ingest_event(base_event(
        IvSourceKind::OptionChain,
        IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
            series_id: "configured-series-a".to_string(),
            surface_selector: "configured-surface-selector".to_string(),
            atm_strike: Some(f64::NAN),
            calls: vec![IvOptionChainStrikePayload {
                strike: 100.0,
                quote: chain_quote_payload("configured-call-valid"),
                greeks: Some(chain_greeks_payload("configured-call-valid", 0.35)),
            }],
            puts: Vec::new(),
        }),
    ));

    assert_eq!(result, Err(IvStoreError::InvalidIvValue));
    assert_eq!(store.raw_events().len(), 1);
    assert!(store.smiles().is_empty());
}

#[test]
fn aggregate_greeks_events_are_preserved_and_indexed_as_products() {
    let mut store = IvStore::empty();

    store
        .ingest_event(base_event(
            IvSourceKind::AggregateGreeks,
            IvRawPayload::AggregateGreeks(IvAggregateGreeksPayload {
                aggregate_key: "configured-aggregate-key".to_string(),
                underlying_selectors: vec!["configured-underlying-selector".to_string()],
                greeks: IvGreekValues {
                    delta: Some(1.25),
                    gamma: Some(0.15),
                    vega: Some(2.5),
                    theta: None,
                    rho: None,
                },
                aggregate_iv: None,
                nt_custom_data_json: None,
            }),
        ))
        .unwrap();

    assert_eq!(store.raw_events().len(), 1);
    assert_eq!(store.aggregate_greeks().len(), 1);
    assert_eq!(
        store.aggregate_greeks()[0].aggregate_key,
        "configured-aggregate-key"
    );
    assert_eq!(store.aggregate_greeks()[0].greeks.vega, Some(2.5));
    validate_iv_provenance(&store.aggregate_greeks()[0].provenance).unwrap();
}

#[test]
fn custom_implied_volatility_events_are_preserved_as_custom_iv_evidence() {
    let mut store = IvStore::empty();

    store
        .ingest_event(base_event(
            IvSourceKind::CustomImpliedVolatility,
            IvRawPayload::CustomImpliedVolatility(IvCustomIvPayload {
                iv_evidence_kind: "configured-custom-evidence-kind".to_string(),
                value: 0.37,
                nt_custom_data_json: None,
            }),
        ))
        .unwrap();

    assert_eq!(store.raw_events().len(), 1);
    assert_eq!(store.iv_evidence().len(), 1);
    assert_eq!(
        store.iv_evidence()[0].iv_evidence_kind,
        "configured-custom-evidence-kind"
    );
    assert_eq!(store.iv_evidence()[0].value, 0.37);
    validate_iv_provenance(&store.iv_evidence()[0].provenance).unwrap();
}
