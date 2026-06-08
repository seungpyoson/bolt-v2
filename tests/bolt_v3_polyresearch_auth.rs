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
        "wss://stream.example.test/feed?key=test-prr-api-key"
    );
}

#[test]
fn rejects_polyresearch_endpoint_that_already_contains_key() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed?key=test-prr-api-key".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint must not duplicate the separately configured api key");

    assert!(
        error.contains("websocket_endpoint"),
        "error should identify the bad endpoint field: {error}"
    );
    assert!(
        error.contains("key"),
        "error should identify the duplicated query key: {error}"
    );
}

#[test]
fn rejects_polyresearch_endpoint_that_already_contains_legacy_api_key() {
    let config = PolyResearchAuthConfig {
        websocket_endpoint: "wss://stream.example.test/feed?apiKey=test-prr-api-key".to_string(),
        api_key: "test-prr-api-key".to_string(),
    };

    let error = polyresearch_websocket_url(&config)
        .expect_err("endpoint must not carry legacy credential query parameters");

    assert!(
        error.contains("websocket_endpoint"),
        "error should identify the bad endpoint field: {error}"
    );
    assert!(
        error.contains("apiKey"),
        "error should identify the duplicated legacy query key: {error}"
    );
}
