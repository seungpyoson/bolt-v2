use bolt_v2::bolt_v3_providers::polyresearch::{
    PolyResearchAuthConfig, polyresearch_websocket_url,
};

#[test]
fn builds_polyresearch_websocket_url_from_clean_endpoint_and_api_key() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let url = polyresearch_websocket_url(&config)
        .expect("clean endpoint plus separate api key should build websocket URL");

    assert_eq!(
        url.as_str(),
        "wss://stream.example.test/feed?apiKey=test-prr-api-key"
    );
}

#[test]
fn rejects_polyresearch_endpoint_that_already_contains_api_key() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed?apiKey=test-prr-api-key".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint must not duplicate the separately configured api key");

    assert!(
        error.contains("websocket_endpoint"),
        "error should identify the bad endpoint field: {error}"
    );
    assert!(
        error.contains("apiKey"),
        "error should identify the duplicated query key: {error}"
    );
}

#[test]
fn rejects_polyresearch_endpoint_with_old_key_query_as_non_credential_free() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed?key=test-prr-api-key".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint query params must not bypass SSM-owned api key config");

    assert!(
        error.contains("credential-free"),
        "old key query should be rejected as a forbidden endpoint query: {error}"
    );
}

#[test]
fn rejects_polyresearch_endpoint_with_non_credential_query() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed?route=reference".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint query params must not bypass TOML-owned endpoint shape");

    assert!(
        error.contains("credential-free"),
        "error should identify the credential-free endpoint requirement: {error}"
    );
}

#[test]
fn rejects_polyresearch_endpoint_with_url_userinfo() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://user:pass@stream.example.test/feed".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint userinfo must not bypass SSM-owned api key config");

    assert!(
        error.contains("credential-free"),
        "error should identify the credential-free endpoint requirement: {error}"
    );
}

#[test]
fn rejects_polyresearch_endpoint_with_fragment() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed#test-prr-api-key".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint fragment must not carry credential material");

    assert!(
        error.contains("credential-free"),
        "error should identify the credential-free endpoint requirement: {error}"
    );
}
