use anyhow::Result;

use crate::{
    bolt_v3_config::{PRICE_GATE_VALUE_KIND, RESOLUTION_GATE_ROLE},
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_operator_artifacts::{EntryReadinessGateSession, GateSatisfaction},
};

pub const PRICE_TO_BEAT_VALUE_FIELD: &str = "price_to_beat_value";

pub fn price_to_beat_from_readiness_session(session: &EntryReadinessGateSession) -> Result<f64> {
    let satisfaction = session
        .satisfied_roles
        .get(RESOLUTION_GATE_ROLE)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "entry decision evidence source readiness_session is missing resolution evidence"
            )
        })?;
    let GateSatisfaction::Evidence { evidence } = satisfaction else {
        anyhow::bail!(
            "entry decision evidence source readiness_session resolution evidence is required"
        );
    };
    anyhow::ensure!(
        evidence.value_kind == PRICE_GATE_VALUE_KIND,
        "entry decision evidence source readiness_session resolution value_kind is invalid"
    );
    let value = evidence
        .normalized_value
        .get(PRICE_TO_BEAT_VALUE_FIELD)
        .and_then(json_value_as_f64)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "entry decision evidence source readiness_session price_to_beat_value is invalid"
            )
        })?;
    anyhow::ensure!(
        is_positive_finite(value),
        "entry decision evidence source readiness_session price_to_beat_value is invalid"
    );
    Ok(value)
}

fn json_value_as_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
        .or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<f64>().ok())
        })
}
