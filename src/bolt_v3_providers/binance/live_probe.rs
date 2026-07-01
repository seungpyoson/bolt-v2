use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_providers::binance::{BinanceDataConfig, BinanceProductType},
};

const BINANCE_INVALID_API_KEY_CODE: &str = "-2015";
const BINANCE_INVALID_API_KEY_HEADER: &str = "invalid x-mbx-apikey";
const BINANCE_INVALID_API_KEY_PERMISSION_PHRASE: &str =
    "invalid api-key, ip, or permissions for action";
const SECRET_REDACTION_PLACEHOLDER: &str = "[redacted-secret]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotLiveProbeClientDescription {
    pub venue: String,
    pub product_type: String,
    pub environment: String,
    pub base_url_http: String,
    pub base_url_ws: String,
    pub spot_market_data_mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotLiveProbeFailureKind {
    InvalidApiKeyIpAllowlist,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotLiveProbeFailure {
    pub kind: SpotLiveProbeFailureKind,
    pub reason: String,
}

pub fn configured_spot_live_probe_client(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<SpotLiveProbeClientDescription, String> {
    let client = loaded
        .root
        .clients
        .get(client_key)
        .ok_or_else(|| format!("clients.{client_key} is not configured"))?;
    let data_value = client
        .data
        .as_ref()
        .ok_or_else(|| format!("clients.{client_key}.data is required"))?;
    let data: BinanceDataConfig =
        data_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| {
                format!("clients.{client_key}.data is not a Binance data block: {error}")
            })?;
    if data.product_type != BinanceProductType::Spot {
        return Err(format!(
            "clients.{client_key}.data.product_type is {:?}, but the Binance spot live probe requires spot",
            data.product_type
        ));
    }
    if client.secrets.is_none() {
        return Err(format!(
            "clients.{client_key}.secrets is required so the real SSM-backed Binance spot data client is constructed"
        ));
    }
    Ok(SpotLiveProbeClientDescription {
        venue: client.venue.to_string(),
        product_type: format!("{:?}", data.product_type).to_ascii_lowercase(),
        environment: format!("{:?}", data.environment).to_ascii_lowercase(),
        base_url_http: data.base_url_http,
        base_url_ws: data.base_url_ws,
        spot_market_data_mode: format!("{:?}", data.spot_market_data_mode).to_ascii_lowercase(),
    })
}

pub fn classify_spot_live_probe_failure(raw_reason: &str, source_ip: &str) -> SpotLiveProbeFailure {
    let lower = raw_reason.to_ascii_lowercase();
    if lower.contains(BINANCE_INVALID_API_KEY_CODE)
        || lower.contains(BINANCE_INVALID_API_KEY_HEADER)
        || lower.contains(BINANCE_INVALID_API_KEY_PERMISSION_PHRASE)
    {
        return SpotLiveProbeFailure {
            kind: SpotLiveProbeFailureKind::InvalidApiKeyIpAllowlist,
            reason: format!(
                "Binance {BINANCE_INVALID_API_KEY_CODE} / Invalid X-MBX-APIKEY: the box's Elastic IP {source_ip} must be added to the Binance API key allowlist, or the API key/permissions must be fixed"
            ),
        };
    }
    SpotLiveProbeFailure {
        kind: SpotLiveProbeFailureKind::Other,
        reason: raw_reason.to_owned(),
    }
}

pub fn no_live_price_reason(instrument: &str, timeout_secs: u64) -> String {
    format!("no live {instrument} quote update arrived within {timeout_secs}s")
}

pub fn live_node_exited_before_price_reason(instrument: &str) -> String {
    format!("LiveNode exited before a live {instrument} quote update arrived")
}

pub fn sanitize_spot_live_probe_error<'a>(
    raw: &str,
    redactions: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut sanitized: String = raw
        .chars()
        .map(|char| match char {
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect();
    for redaction in redactions {
        if !redaction.is_empty() {
            sanitized = sanitized.replace(redaction, SECRET_REDACTION_PLACEHOLDER);
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_binance_error_code_as_ip_allowlist_failure() {
        let failure =
            classify_spot_live_probe_failure("websocket rejected request: -2015", "1.2.3.4");

        assert_eq!(
            failure.kind,
            SpotLiveProbeFailureKind::InvalidApiKeyIpAllowlist
        );
        assert!(failure.reason.contains("Invalid X-MBX-APIKEY"));
        assert!(failure.reason.contains("Elastic IP 1.2.3.4"));
        assert!(failure.reason.contains("allowlist"));
    }

    #[test]
    fn classifies_binance_header_phrase_as_ip_allowlist_failure() {
        let failure = classify_spot_live_probe_failure(
            "Invalid X-MBX-APIKEY; invalid API-key, IP, or permissions for action",
            "5.6.7.8",
        );

        assert_eq!(
            failure.kind,
            SpotLiveProbeFailureKind::InvalidApiKeyIpAllowlist
        );
        assert!(failure.reason.contains("Elastic IP 5.6.7.8"));
    }

    #[test]
    fn preserves_generic_timeout_failure_reason() {
        let raw = no_live_price_reason("BTCUSDT.BINANCE", 15);
        let failure = classify_spot_live_probe_failure(&raw, "1.2.3.4");

        assert_eq!(failure.kind, SpotLiveProbeFailureKind::Other);
        assert_eq!(
            failure.reason,
            "no live BTCUSDT.BINANCE quote update arrived within 15s"
        );
    }

    #[test]
    fn sanitizer_redacts_known_secret_material_and_flattens_lines() {
        let sanitized = sanitize_spot_live_probe_error(
            "first line\nsecret-token\rsecond line",
            ["secret-token"],
        );

        assert_eq!(sanitized, "first line [redacted-secret] second line");
    }
}
