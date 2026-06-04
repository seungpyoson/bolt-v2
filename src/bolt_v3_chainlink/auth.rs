//! Chainlink Data Streams REST authentication core.
//!
//! Pure protocol logic moved out of `crate::bolt_v3_operator_artifacts`:
//! credential field validation, signed request-URL construction, the
//! HMAC-SHA256 signing-string headers, and the zeroizing credential
//! holder. No config resolution and no filesystem collectors live here.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::bolt_v3_operator_artifacts::{
    BoltV3OperatorArtifactError, entry_decision_source_invalid,
    price_to_beat_report_provenance_config_invalid,
};

const CHAINLINK_DATA_STREAMS_REPORT_FEED_ID_QUERY_FIELD: &str = "feedID";
const CHAINLINK_DATA_STREAMS_REPORT_TIMESTAMP_QUERY_FIELD: &str = "timestamp";
const CHAINLINK_DATA_STREAMS_AUTHORIZATION_HEADER: &str = "Authorization";
const CHAINLINK_DATA_STREAMS_AUTHORIZATION_TIMESTAMP_HEADER: &str = "X-Authorization-Timestamp";
const CHAINLINK_DATA_STREAMS_AUTHORIZATION_SIGNATURE_HEADER: &str =
    "X-Authorization-Signature-SHA256";
const CHAINLINK_DATA_STREAMS_GET_METHOD: &str = "GET";
const CHAINLINK_DATA_STREAMS_HMAC_BLOCK_BYTES: usize = 64;
const CHAINLINK_DATA_STREAMS_HMAC_IPAD: u8 = 0x36;
const CHAINLINK_DATA_STREAMS_HMAC_OPAD: u8 = 0x5c;

pub(crate) struct ChainlinkDataStreamsCredentials {
    pub(crate) api_key: String,
    pub(crate) api_secret: String,
}

impl Drop for ChainlinkDataStreamsCredentials {
    fn drop(&mut self) {
        self.api_key.zeroize();
        self.api_secret.zeroize();
    }
}

pub(crate) fn chainlink_data_streams_credentials(
    api_key: &str,
    api_secret: &str,
) -> Result<ChainlinkDataStreamsCredentials, BoltV3OperatorArtifactError> {
    Ok(ChainlinkDataStreamsCredentials {
        api_key: chainlink_credential_field(api_key.to_string(), "api_key")?,
        api_secret: chainlink_credential_field(api_secret.to_string(), "api_secret")?,
    })
}

fn chainlink_credential_field(
    value: String,
    field: &'static str,
) -> Result<String, BoltV3OperatorArtifactError> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(entry_decision_source_invalid(format!(
            "Chainlink Data Streams credential field `{field}` is invalid"
        )));
    }
    Ok(value)
}

pub(crate) fn chainlink_data_streams_report_request_url(
    rest_base_url: &str,
    report_endpoint_path: &str,
    feed_id: &str,
    report_timestamp_unix_seconds: u64,
) -> Result<(String, String), BoltV3OperatorArtifactError> {
    let base_url = url::Url::parse(rest_base_url)
        .map_err(|_| price_to_beat_report_provenance_config_invalid())?;
    let mut url = base_url
        .join(report_endpoint_path)
        .map_err(|_| price_to_beat_report_provenance_config_invalid())?;
    url.query_pairs_mut()
        .append_pair(CHAINLINK_DATA_STREAMS_REPORT_FEED_ID_QUERY_FIELD, feed_id)
        .append_pair(
            CHAINLINK_DATA_STREAMS_REPORT_TIMESTAMP_QUERY_FIELD,
            &report_timestamp_unix_seconds.to_string(),
        );
    let mut path_with_query = url.path().to_string();
    if let Some(query) = url.query() {
        path_with_query.push('?');
        path_with_query.push_str(query);
    }
    Ok((url.to_string(), path_with_query))
}

pub(crate) fn chainlink_data_streams_auth_headers(
    credentials: &ChainlinkDataStreamsCredentials,
    path_with_query: &str,
    authorization_timestamp_ms: u64,
) -> HashMap<String, String> {
    let body_hash = hex::encode(Sha256::digest(b""));
    let mut signing_string = format!(
        "{} {} {} {} {}",
        CHAINLINK_DATA_STREAMS_GET_METHOD,
        path_with_query,
        body_hash,
        credentials.api_key,
        authorization_timestamp_ms
    );
    let signature =
        chainlink_hmac_sha256_hex(credentials.api_secret.as_bytes(), signing_string.as_bytes());
    signing_string.zeroize();
    HashMap::from([
        (
            CHAINLINK_DATA_STREAMS_AUTHORIZATION_HEADER.to_string(),
            credentials.api_key.clone(),
        ),
        (
            CHAINLINK_DATA_STREAMS_AUTHORIZATION_TIMESTAMP_HEADER.to_string(),
            authorization_timestamp_ms.to_string(),
        ),
        (
            CHAINLINK_DATA_STREAMS_AUTHORIZATION_SIGNATURE_HEADER.to_string(),
            signature,
        ),
    ])
}

fn chainlink_hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    let mut key_block = if key.len() > CHAINLINK_DATA_STREAMS_HMAC_BLOCK_BYTES {
        Sha256::digest(key).to_vec()
    } else {
        key.to_vec()
    };
    key_block.resize(CHAINLINK_DATA_STREAMS_HMAC_BLOCK_BYTES, 0);
    let mut inner_key = key_block.clone();
    let mut outer_key = key_block.clone();
    for byte in &mut inner_key {
        *byte ^= CHAINLINK_DATA_STREAMS_HMAC_IPAD;
    }
    for byte in &mut outer_key {
        *byte ^= CHAINLINK_DATA_STREAMS_HMAC_OPAD;
    }
    let mut inner = Sha256::new();
    inner.update(&inner_key);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&outer_key);
    outer.update(inner_hash);
    let signature = hex::encode(outer.finalize());
    key_block.zeroize();
    inner_key.zeroize();
    outer_key.zeroize();
    signature
}

#[cfg(test)]
mod tests {
    //! REST authentication core unit tests: signed request-URL construction and
    //! the HMAC-SHA256 signing-string headers. The expected signature is a
    //! golden value computed independently with a standard HMAC-SHA256 over the
    //! exact `"{GET} {path_with_query} {sha256("")} {api_key} {ts_ms}"` signing
    //! string, so the hand-rolled HMAC is checked against a reference, not
    //! against itself.

    use super::*;

    const TEST_BASE_URL: &str = "https://api.example.com/";
    const TEST_ENDPOINT_PATH: &str = "api/v1/reports";
    const TEST_FEED_ID: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const TEST_REPORT_TIMESTAMP_SECONDS: u64 = 600;
    const TEST_API_KEY: &str = "test-api-key";
    const TEST_API_SECRET: &str = "test-api-secret";
    const TEST_AUTHORIZATION_TIMESTAMP_MS: u64 = 1_700_000_000_000;
    // HMAC-SHA256(secret, "GET <path?query> <sha256("") hex> <api_key> <ts_ms>")
    // computed with a standard library HMAC over the pinned inputs below.
    const TEST_EXPECTED_SIGNATURE: &str =
        "64df22e6b33c58ea5ea862fe4bbf7675b74cdea80a1c50262072b585176f11e6";
    const TEST_EXPECTED_PATH_WITH_QUERY: &str = "/api/v1/reports?feedID=0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9&timestamp=600";

    #[test]
    fn report_request_url_appends_feed_id_and_timestamp_query() {
        let (url, path_with_query) = chainlink_data_streams_report_request_url(
            TEST_BASE_URL,
            TEST_ENDPOINT_PATH,
            TEST_FEED_ID,
            TEST_REPORT_TIMESTAMP_SECONDS,
        )
        .expect("a valid base URL and endpoint path should build a request URL");
        assert_eq!(
            url,
            format!("https://api.example.com{TEST_EXPECTED_PATH_WITH_QUERY}")
        );
        assert_eq!(path_with_query, TEST_EXPECTED_PATH_WITH_QUERY);
    }

    #[test]
    fn report_request_url_rejects_invalid_base_url() {
        chainlink_data_streams_report_request_url(
            "not a url",
            TEST_ENDPOINT_PATH,
            TEST_FEED_ID,
            TEST_REPORT_TIMESTAMP_SECONDS,
        )
        .expect_err("a non-URL base must fail closed");
    }

    #[test]
    fn auth_headers_match_golden_hmac_signature() {
        let credentials = chainlink_data_streams_credentials(TEST_API_KEY, TEST_API_SECRET)
            .expect("whitespace-free credentials should validate");
        let headers = chainlink_data_streams_auth_headers(
            &credentials,
            TEST_EXPECTED_PATH_WITH_QUERY,
            TEST_AUTHORIZATION_TIMESTAMP_MS,
        );
        assert_eq!(
            headers.get(CHAINLINK_DATA_STREAMS_AUTHORIZATION_HEADER),
            Some(&TEST_API_KEY.to_string())
        );
        assert_eq!(
            headers.get(CHAINLINK_DATA_STREAMS_AUTHORIZATION_TIMESTAMP_HEADER),
            Some(&TEST_AUTHORIZATION_TIMESTAMP_MS.to_string())
        );
        assert_eq!(
            headers.get(CHAINLINK_DATA_STREAMS_AUTHORIZATION_SIGNATURE_HEADER),
            Some(&TEST_EXPECTED_SIGNATURE.to_string()),
            "hand-rolled HMAC-SHA256 must match the independent golden signature"
        );
    }

    #[test]
    fn credentials_reject_whitespace_bearing_fields() {
        // `ChainlinkDataStreamsCredentials` is credential-bearing and
        // deliberately has no `Debug`, so assert on `is_err()` rather than
        // `expect_err` (which would require formatting the `Ok` value).
        assert!(
            chainlink_data_streams_credentials(" leading", TEST_API_SECRET).is_err(),
            "a credential with whitespace must be rejected"
        );
        assert!(
            chainlink_data_streams_credentials(TEST_API_KEY, "").is_err(),
            "an empty credential must be rejected"
        );
    }
}
