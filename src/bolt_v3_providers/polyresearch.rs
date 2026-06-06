//! PolyResearch reference-price WebSocket authentication helpers.
//!
//! PRR auth is a query parameter named `apiKey`. Bolt keeps the endpoint and
//! credential as separate SSM values, then constructs the credentialed URL once
//! at the provider edge.

use url::Url;

const POLYRESEARCH_API_KEY_QUERY_FIELD: &str = "apiKey";

pub struct PolyResearchAuthConfig {
    pub websocket_endpoint: String,
    pub api_key: String,
}

pub fn polyresearch_websocket_url(config: &PolyResearchAuthConfig) -> Result<Url, String> {
    validate_secret_field("api_key", &config.api_key)?;
    let mut url = validate_websocket_endpoint(&config.websocket_endpoint)?;
    if url
        .query_pairs()
        .any(|(key, _)| key.eq_ignore_ascii_case(POLYRESEARCH_API_KEY_QUERY_FIELD))
    {
        return Err(format!(
            "polyresearch websocket_endpoint must not contain `{POLYRESEARCH_API_KEY_QUERY_FIELD}`; configure api_key separately"
        ));
    }
    url.query_pairs_mut()
        .append_pair(POLYRESEARCH_API_KEY_QUERY_FIELD, &config.api_key);
    Ok(url)
}

fn validate_secret_field(field: &'static str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(format!("polyresearch {field} is invalid"));
    }
    Ok(())
}

fn validate_websocket_endpoint(value: &str) -> Result<Url, String> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err("polyresearch websocket_endpoint must be a non-empty wss URL".to_string());
    }
    let url = Url::parse(value)
        .map_err(|_| "polyresearch websocket_endpoint must be a valid wss URL".to_string())?;
    if url.scheme() != "wss" || !url.has_host() || !value[url.scheme().len()..].starts_with("://") {
        return Err("polyresearch websocket_endpoint must be a valid wss URL".to_string());
    }
    Ok(url)
}
