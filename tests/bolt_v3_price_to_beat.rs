use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{PRICE_GATE_VALUE_KIND, RESOLUTION_GATE_ROLE},
    bolt_v3_market_families::SelectedMarketRequirement,
    bolt_v3_operator_artifacts::{EntryReadinessGateSession, GateEvidence, GateSatisfaction},
    bolt_v3_price_to_beat::price_to_beat_from_readiness_session,
};

fn readiness_session_with_resolution_value(value: serde_json::Value) -> EntryReadinessGateSession {
    let mut satisfied_roles = BTreeMap::new();
    satisfied_roles.insert(
        RESOLUTION_GATE_ROLE.to_string(),
        GateSatisfaction::Evidence {
            evidence: Box::new(GateEvidence {
                schema_version: 1,
                record_kind: "gate_evidence".to_string(),
                role: RESOLUTION_GATE_ROLE.to_string(),
                provider_id: "resolution_provider".to_string(),
                provider_kind: "venue_native".to_string(),
                selected_market_key: "selected-market-key".to_string(),
                collector_observed_at_ms: 1_000,
                source_observed_at_ms: 1_000,
                fresh_until_ms: 2_000,
                value_kind: PRICE_GATE_VALUE_KIND.to_string(),
                normalized_value: value,
                normalized_value_sha256: "0".repeat(64),
                provider_provenance: serde_json::json!({}),
                provider_provenance_sha256: "0".repeat(64),
                artifact_refs: Vec::new(),
            }),
        },
    );

    EntryReadinessGateSession {
        schema_version: 1,
        record_kind: "entry_readiness_gate_session".to_string(),
        strategy_instance_id: "strategy".to_string(),
        configured_target_id: "target".to_string(),
        selected_market: SelectedMarketRequirement {
            configured_target_id: "target".to_string(),
            venue: "POLYMARKET".to_string(),
            family_key: "updown".to_string(),
            market_id: "market".to_string(),
            instrument_ids: vec![
                "condition-DOWN.POLYMARKET".to_string(),
                "condition-UP.POLYMARKET".to_string(),
            ],
            market_class: "binary_option".to_string(),
            resolution_kind: "venue_native".to_string(),
            resolution_identity: "price-threshold".to_string(),
            value_kind: PRICE_GATE_VALUE_KIND.to_string(),
            metadata_provenance_sha256: "0".repeat(64),
            selected_market_key: "selected-market-key".to_string(),
            selected_at_ms: 1_000,
        },
        created_at_ms: 1_000,
        satisfied_roles,
        session_hash: "0".repeat(64),
        artifact_refs: Vec::new(),
    }
}

#[test]
fn readiness_session_helper_extracts_positive_price_to_beat() {
    let session = readiness_session_with_resolution_value(serde_json::json!({
        "price_to_beat_value": "3100.25"
    }));

    let price_to_beat = price_to_beat_from_readiness_session(&session)
        .expect("positive source-bound price_to_beat should extract");

    assert_eq!(price_to_beat, 3100.25);
}
