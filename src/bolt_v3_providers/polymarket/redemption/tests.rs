#![cfg(test)]

use std::cmp::Ordering;
use std::io::Cursor;

use alloy_primitives::keccak256;
use serde_json::{Value, json};

use super::capability::hermetic;
use super::config::HermeticCredentialSource;
use super::*;

const CONFIG: &str = include_str!("../../../../config/polymarket-redemption.toml");
const MANIFEST: &str = include_str!("../../../../ci/polymarket-redemption-provider-manifest.toml");
const OWNER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const PRIVATE_KEY: &[u8] = b"0000000000000000000000000000000000000000000000000000000000000001";

fn credential_value(
    path: &str,
    sink: &mut CredentialSink<'_>,
) -> Result<(), RedemptionConfigError> {
    let value: &[u8] = if path.ends_with("private-key") {
        PRIVATE_KEY
    } else if path.ends_with("builder-api-secret") {
        b"c2VjcmV0"
    } else if path.ends_with("redaction-hmac-key") {
        b"hermetic-redaction-key-material"
    } else if path.ends_with("builder-api-key") {
        b"hermetic-api-key"
    } else {
        b"hermetic-passphrase"
    };
    sink.append(value)
}

fn profile() -> ValidatedRedemptionProfile {
    validate_profile(CONFIG, MANIFEST).unwrap()
}

fn credentials(profile: &ValidatedRedemptionProfile) -> ResolvedRedemptionCredentials {
    let mut source = HermeticCredentialSource::new(credential_value);
    resolve_credentials(profile, "hermetic-region", &mut source).unwrap()
}

fn prepared(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    mode: MarketMode,
    condition: u8,
    nonce: SafeNonce,
) -> PreparedRequestPair {
    let snapshot = hermetic::snapshot([condition; 32], [[1; 32], [2; 32]], 7);
    let capacity = hermetic::nonce_capacity(
        profile.safe_address(),
        nonce,
        profile.max_request_bytes(),
        profile.max_request_bytes(),
        11,
    );
    build_request_pair(
        profile,
        credentials,
        snapshot,
        capacity,
        RedemptionBuildInput::new(mode, OWNER, "hermetic-redemption"),
        1_700_000_000,
    )
    .unwrap()
}

fn authorize_original(prepared: PreparedRequestPair) -> OriginalMayHaveStartedRequest {
    let (binding, original_hash, _, snapshot_generation, lane_generation) =
        prepared.hermetic_bindings();
    prepared
        .authorize_original(
            hermetic::fresh(binding, snapshot_generation, lane_generation),
            hermetic::original_durable(binding, original_hash, 13),
        )
        .unwrap()
}

fn authorize_fence(original: OriginalMayHaveStartedRequest) -> FenceMayHaveStartedRequest {
    let (binding, original_hash, fence_hash, snapshot_generation, lane_generation) =
        original.prepared().hermetic_bindings();
    original
        .authorize_fence(
            hermetic::fresh(binding, snapshot_generation, lane_generation),
            hermetic::fence_durable(binding, original_hash, fence_hash, 17),
        )
        .unwrap()
}

fn query_values(queries: &ExactQuerySet) -> Vec<Value> {
    (0..queries.count())
        .map(|index| serde_json::from_slice(queries.query_bytes(index).unwrap()).unwrap())
        .collect()
}

fn chain_response(
    authority: &impl ResponseReadAuthority,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    value: Value,
) -> BoundedWireResponse {
    BoundedWireResponse::read_chain(
        authority,
        profile,
        credentials,
        Cursor::new(serde_json::to_vec(&value).unwrap()),
    )
    .unwrap()
}

fn relayer_response(
    authority: &impl ResponseReadAuthority,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    value: Value,
) -> BoundedWireResponse {
    BoundedWireResponse::read_relayer(
        authority,
        profile,
        credentials,
        Cursor::new(serde_json::to_vec(&value).unwrap()),
    )
    .unwrap()
}

fn raw_responses(
    authority: &impl ResponseReadAuthority,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    queries: &ExactQuerySet,
    winner: Option<RequestKind>,
    reorged: Option<RequestKind>,
    wrong_query_id: Option<RequestKind>,
    observed_nonce: SafeNonce,
    post_balances: [[u8; 32]; 2],
) -> ExactQueryResponses {
    let values = query_values(queries);
    let nonce_query = values
        .iter()
        .find(|value| value["kind"] == "safe_nonce")
        .unwrap();
    let executions: Vec<_> = values
        .iter()
        .filter(|value| value["kind"] == "finalized_receipt_logs")
        .collect();
    let post = values
        .iter()
        .find(|value| value["kind"] == "raw_post_state")
        .unwrap();
    let boundary = values
        .iter()
        .find(|value| value["kind"] == "safe_boundary")
        .unwrap();
    let head_number = "0x81";
    let head_hash = format!("0x{}", "a5".repeat(32));
    let event_topic = format!(
        "0x{}",
        hex::encode(keccak256(b"ExecutionSuccess(bytes32,uint256)"))
    );
    let execution = |index: usize, kind: RequestKind| {
        let present = winner == Some(kind);
        let safe_hash = executions[index]["safe_transaction_hash"].as_str().unwrap();
        let transaction_hash = format!("0x{}", if index == 0 { "31" } else { "32" }.repeat(32));
        let block_hash = format!("0x{}", if index == 0 { "41" } else { "42" }.repeat(32));
        let receipts = if present {
            json!([{
                "transactionHash": transaction_hash.clone(),
                "blockNumber": "0x2",
                "blockHash": block_hash.clone(),
                "transactionIndex": "0x0",
                "status": "0x1",
                "logs": [{
                    "address": executions[index]["safe"],
                    "topics": [event_topic.clone()],
                    "data": format!("{}{}", safe_hash, "00".repeat(32)),
                    "blockNumber": "0x2",
                    "blockHash": block_hash.clone(),
                    "transactionHash": transaction_hash,
                    "transactionIndex": "0x0",
                    "logIndex": "0x0",
                    "removed": false
                }]
            }])
        } else {
            json!([])
        };
        let canonical_block = if present {
            let canonical_hash = if reorged == Some(kind) {
                format!("0x{}", "55".repeat(32))
            } else {
                block_hash
            };
            json!({"blockNumber": "0x2", "blockHash": canonical_hash})
        } else {
            Value::Null
        };
        json!({
            "queryId": if wrong_query_id == Some(kind) {
                "wrong_query"
            } else if index == 0 {
                "original_finalized_receipt_logs"
            } else {
                "fence_finalized_receipt_logs"
            },
            "safeAddress": executions[index]["safe"],
            "safeTransactionHash": safe_hash,
            "observedAtBlockNumber": head_number,
            "observedAtBlockHash": head_hash.clone(),
            "receipts": receipts,
            "canonicalBlock": canonical_block
        })
    };
    ExactQueryResponses::new(
        authority,
        credentials,
        chain_response(
            authority,
            profile,
            credentials,
            json!({
                "queryId": "safe_nonce",
                "safeAddress": nonce_query["safe"],
                "calldata": nonce_query["calldata"],
                "blockNumber": head_number,
                "blockHash": head_hash.clone(),
                "result": format!("0x{}", hex::encode(observed_nonce.as_word()))
            }),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            execution(0, RequestKind::Original),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            execution(1, RequestKind::Fence),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            json!({
                "queryId": "raw_post_state",
                "target": post["target"],
                "conditionId": post["condition_id"],
                "blockNumber": head_number,
                "blockHash": head_hash.clone(),
                "results": post_balances.map(|value| format!("0x{}", hex::encode(value)))
            }),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            json!({
                "queryId": "safe_boundary",
                "safe": boundary["safe"],
                "factory": boundary["factory"],
                "implementation": boundary["implementation"],
                "fallbackHandler": boundary["fallback_handler"],
                "guard": boundary["guard"],
                "modules": [],
                "blockNumber": head_number,
                "blockHash": head_hash.clone()
            }),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            json!({
                "queryId": "finalized_head",
                "chainId": 137,
                "blockNumber": head_number,
                "blockHash": head_hash
            }),
        ),
    )
    .unwrap()
}

#[test]
fn standard_and_negative_risk_fixtures() {
    for (fixture, expected) in [
        (
            include_str!("../../../../tests/fixtures/bolt_v3/redeem/standard.toml"),
            "standard",
        ),
        (
            include_str!("../../../../tests/fixtures/bolt_v3/redeem/negative-risk.toml"),
            "negative_risk",
        ),
    ] {
        let value: toml::Value = toml::from_str(fixture).unwrap();
        assert_eq!(value["market_mode"].as_str(), Some(expected));
        assert_eq!(value["expected_post_balances"].as_array().unwrap().len(), 2);
    }
    let profile = profile();
    let credentials = credentials(&profile);
    assert!(
        prepared(
            &profile,
            &credentials,
            MarketMode::Standard,
            1,
            SafeNonce::ZERO
        )
        .same_nonce()
    );
    assert!(
        prepared(
            &profile,
            &credentials,
            MarketMode::NegativeRisk,
            2,
            SafeNonce::ZERO
        )
        .same_nonce()
    );
}

#[test]
fn original_and_fence_body_boundaries() {
    let profile = profile();
    let credentials = credentials(&profile);
    let pair = prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    );
    let repeated = prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    );
    for kind in [RequestKind::Original, RequestKind::Fence] {
        let body = pair.hermetic_body(kind);
        assert_eq!(body, repeated.hermetic_body(kind));
        assert_eq!(pair.hermetic_headers(kind), repeated.hermetic_headers(kind));
        assert!(body.len() <= profile.max_request_bytes());
        assert!(body.starts_with(b"{\"type\":\"SAFE\""));
        let mut too_small = super::bounded::CappedBytes::with_capacity(body.len() - 1);
        assert!(too_small.extend(body).is_err());
        let mut exact = super::bounded::CappedBytes::with_capacity(body.len());
        assert!(exact.extend(body).is_ok());
        assert_eq!(exact.len(), body.len());
        let mut spare = super::bounded::CappedBytes::with_capacity(body.len() + 1);
        assert!(spare.extend(body).is_ok());
        assert_eq!(spare.len(), body.len());
    }
}

#[test]
fn response_loss_requires_exact_queries() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let queries = ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
    assert_eq!(queries.count() + 1, profile.max_query_items());
    assert_eq!(queries.kind_count(QueryKind::SafeNonce), 1);
    assert_eq!(
        queries.kind_count(QueryKind::OriginalFinalizedReceiptLogs),
        1
    );
    assert_eq!(queries.kind_count(QueryKind::FenceFinalizedReceiptLogs), 1);
    assert_eq!(queries.kind_count(QueryKind::RawPostState), 1);
    assert_eq!(queries.kind_count(QueryKind::SafeBoundary), 1);
    assert_eq!(queries.kind_count(QueryKind::FinalizedHead), 1);
}

#[test]
fn exact_relayer_record_binds_every_source_field() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let submit = BoundedWireResponse::read_relayer(
        &original,
        &profile,
        &credentials,
        Cursor::new(br#"{"transactionID":"exact-id","state":"STATE_NEW","transactionHash":""}"#),
    )
    .unwrap()
    .parse_submit(&original, &profile, &credentials)
    .unwrap();
    let body: Value =
        serde_json::from_slice(original.prepared().hermetic_body(RequestKind::Original)).unwrap();
    let record = json!([{
        "transactionID": "exact-id",
        "transactionHash": "",
        "from": body["from"],
        "to": body["to"],
        "proxyAddress": body["proxyWallet"],
        "data": body["data"],
        "nonce": body["nonce"],
        "value": "0",
        "state": "STATE_NEW",
        "type": "SAFE",
        "metadata": body["metadata"],
        "createdAt": "2026-07-15T00:00:00Z",
        "updatedAt": "2026-07-15T00:00:00Z"
    }]);
    let response = relayer_response(&original, &profile, &credentials, record.clone());
    response
        .parse_exact_transaction(
            &original,
            &profile,
            &credentials,
            &submit,
            RequestKind::Original,
        )
        .unwrap();
    for field in [
        "transactionID",
        "transactionHash",
        "from",
        "to",
        "proxyAddress",
        "data",
        "nonce",
        "value",
        "type",
        "metadata",
    ] {
        let mut tampered = record.clone();
        tampered[0][field] = Value::String("tampered".into());
        let response = relayer_response(&original, &profile, &credentials, tampered);
        assert!(
            response
                .parse_exact_transaction(
                    &original,
                    &profile,
                    &credentials,
                    &submit,
                    RequestKind::Original,
                )
                .is_err()
        );
    }
    let queries =
        ExactQuerySet::after_original_response_loss(&profile, &original, Some(&submit)).unwrap();
    assert_eq!(queries.kind_count(QueryKind::RelayerTransaction), 1);
}

#[test]
fn original_wins_only_with_finalized_post_state() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let queries = ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
    let responses = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        Some(RequestKind::Original),
        None,
        None,
        SafeNonce::from_decimal("1").unwrap(),
        [[0; 32]; 2],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .resolution(),
        RedemptionResolution::RedemptionFinalized
    );
}

#[test]
fn fence_wins_only_with_unchanged_post_state() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let queries = ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
    let responses = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        Some(RequestKind::Fence),
        None,
        None,
        SafeNonce::from_decimal("1").unwrap(),
        [[1; 32], [2; 32]],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .resolution(),
        RedemptionResolution::IntegrityFailure
    );
    let fence = authorize_fence(original);
    assert_eq!(
        responses
            .verify_after_fence(&profile, &credentials, &fence)
            .unwrap()
            .resolution(),
        RedemptionResolution::PermanentlyFencedNoEffect
    );
}

#[test]
fn unrelated_nonce_fails_closed() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let queries = ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
    let responses = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        None,
        None,
        None,
        SafeNonce::from_decimal("9").unwrap(),
        [[1; 32], [2; 32]],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .resolution(),
        RedemptionResolution::IntegrityFailure
    );
}

#[test]
fn relayer_states_never_prove_terminal_effect() {
    for state in [
        RelayerState::New,
        RelayerState::Executed,
        RelayerState::Mined,
        RelayerState::Invalid,
        RelayerState::Confirmed,
        RelayerState::Failed,
    ] {
        assert!(!state.is_terminal_proof());
    }
}

#[test]
fn retry_requires_exact_body_bytes() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let exact = original
        .prepared()
        .hermetic_body(RequestKind::Original)
        .to_vec();
    let headers = original
        .prepared()
        .hermetic_headers(RequestKind::Original)
        .to_vec();
    assert_eq!(
        require_exact_retry(
            &profile,
            &credentials,
            &original.original(),
            Cursor::new(exact.clone()),
            Cursor::new(headers.clone())
        ),
        Ok(())
    );
    let mut changed = exact;
    changed[0] ^= 1;
    assert_eq!(
        require_exact_retry(
            &profile,
            &credentials,
            &original.original(),
            Cursor::new(changed),
            Cursor::new(headers.clone())
        ),
        Err(RedemptionRequestError::RetryMismatch)
    );
    let mut changed_headers = headers;
    changed_headers[0] ^= 1;
    assert_eq!(
        require_exact_retry(
            &profile,
            &credentials,
            &original.original(),
            Cursor::new(exact),
            Cursor::new(changed_headers)
        ),
        Err(RedemptionRequestError::RetryMismatch)
    );
}

#[test]
fn stale_pre_send_token_is_rejected() {
    let profile = profile();
    let credentials = credentials(&profile);
    let prepared = prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    );
    let (binding, original_hash, _, snapshot_generation, lane_generation) =
        prepared.hermetic_bindings();
    assert!(matches!(
        prepared.authorize_original(
            hermetic::fresh(binding, snapshot_generation + 1, lane_generation),
            hermetic::original_durable(binding, original_hash, 1)
        ),
        Err(RedemptionRequestError::CapabilityMismatch)
    ));
}

#[test]
fn pre_send_balance_and_lease_revalidation_fails_closed() {
    stale_pre_send_token_is_rejected();
}

#[test]
fn fence_first_is_unrepresentable_and_mismatched_fence_is_rejected() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let (binding, original_hash, fence_hash, snapshot_generation, lane_generation) =
        original.prepared().hermetic_bindings();
    assert!(matches!(
        original.authorize_fence(
            hermetic::fresh(binding, snapshot_generation, lane_generation),
            hermetic::fence_durable(binding, original_hash, [0; 32], 1)
        ),
        Err(RedemptionRequestError::CapabilityMismatch)
    ));
    assert_ne!(fence_hash, [0; 32]);
}

#[test]
fn concurrent_conditions_cannot_share_one_nonce_permit() {
    let profile = profile();
    let credentials = credentials(&profile);
    let permit = hermetic::nonce_capacity(
        profile.safe_address(),
        SafeNonce::ZERO,
        profile.max_request_bytes(),
        profile.max_request_bytes(),
        1,
    );
    let first = build_request_pair(
        &profile,
        &credentials,
        hermetic::snapshot([1; 32], [[1; 32], [2; 32]], 1),
        permit,
        RedemptionBuildInput::new(MarketMode::Standard, OWNER, "one"),
        1,
    );
    assert!(first.is_ok());
    // `permit` was moved into `first`; a second call using it is rejected by Rust's
    // linear move semantics. The structural verifier compiles this negative shape.
}

#[test]
fn full_width_nonce_domain_and_maximum_are_deterministic() {
    assert_eq!(SafeNonce::from_decimal("0").unwrap(), SafeNonce::ZERO);
    let two64 = SafeNonce::from_decimal("18446744073709551616").unwrap();
    assert_eq!(two64.relation(SafeNonce::ZERO), Ordering::Greater);
    let max_minus_one = SafeNonce::from_decimal(
        "115792089237316195423570985008687907853269984665640564039457584007913129639934",
    )
    .unwrap();
    let maximum = SafeNonce::from_decimal(
        "115792089237316195423570985008687907853269984665640564039457584007913129639935",
    )
    .unwrap();
    assert_eq!(max_minus_one.successor(), Some(maximum));
    assert_eq!(maximum.successor(), None);
    assert_eq!(
        classify_nonce_successor(maximum, maximum),
        NonceRelation::Current
    );
    assert!(SafeNonce::from_decimal("-1").is_err());
    assert!(SafeNonce::from_decimal("not-a-nonce").is_err());
    let profile = profile();
    let credentials = credentials(&profile);
    let snapshot = hermetic::snapshot([1; 32], [[1; 32], [2; 32]], 1);
    let capacity = hermetic::nonce_capacity(
        profile.safe_address(),
        maximum,
        profile.max_request_bytes(),
        profile.max_request_bytes(),
        1,
    );
    assert!(matches!(
        build_request_pair(
            &profile,
            &credentials,
            snapshot,
            capacity,
            RedemptionBuildInput::new(MarketMode::Standard, OWNER, "max"),
            1,
        ),
        Err(RedemptionRequestError::NonceExhausted)
    ));
}

#[test]
fn capped_reader_honors_limit_minus_one_limit_and_limit_plus_one() {
    let key = b"hermetic-key";
    for (input, expected) in [(vec![1; 7], true), (vec![1; 8], true), (vec![1; 9], false)] {
        let result = super::bounded::CappedBytes::read_with_probe(
            Cursor::new(input),
            8,
            1,
            key,
            1,
            ProjectionClass::ChainResponse,
        );
        assert_eq!(result.is_ok(), expected);
    }
    let mut huge_spare = Vec::with_capacity(4096);
    huge_spare.extend_from_slice(&[1; 8]);
    let bounded = super::bounded::CappedBytes::read_with_probe(
        Cursor::new(huge_spare),
        8,
        1,
        key,
        1,
        ProjectionClass::ChainResponse,
    )
    .unwrap();
    assert_eq!(bounded.len(), 8);
}

#[test]
fn oversized_credential_acquisition_is_rejected_before_append() {
    macro_rules! oversized_at {
        ($name:ident, $suffix:literal) => {
            fn $name(
                path: &str,
                sink: &mut CredentialSink<'_>,
            ) -> Result<(), RedemptionConfigError> {
                if path.ends_with($suffix) {
                    sink.append(&[b'x'; 4096])?;
                    sink.append(b"x")
                } else {
                    credential_value(path, sink)
                }
            }
        };
    }
    oversized_at!(oversized_signer, "private-key");
    oversized_at!(oversized_api_key, "builder-api-key");
    oversized_at!(oversized_api_secret, "builder-api-secret");
    oversized_at!(oversized_passphrase, "builder-passphrase");
    oversized_at!(oversized_redaction_key, "redaction-hmac-key");
    let profile = profile();
    for producer in [
        oversized_signer,
        oversized_api_key,
        oversized_api_secret,
        oversized_passphrase,
        oversized_redaction_key,
    ] {
        let mut source = HermeticCredentialSource::new(producer);
        assert_eq!(
            resolve_credentials(&profile, "hermetic-region", &mut source).map(|_| ()),
            Err(RedemptionConfigError::SecretBound)
        );
    }
}

#[test]
fn raw_queries_reject_duplicate_missing_conflicting_and_fabricated_fields() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let duplicate = json!({
        "queryId": "original_finalized_receipt_logs",
        "safeAddress": "0x0000000000000000000000000000000000000000",
        "safeTransactionHash": format!("0x{}", "00".repeat(32)),
        "observedAtBlockNumber": "0x81",
        "observedAtBlockHash": format!("0x{}", "00".repeat(32)),
        "receipts": [{}, {}],
        "winner": "original"
    });
    let duplicate_text = duplicate.to_string();
    assert!(serde_json::from_str::<super::wire::ExecutionQueryWire<'_>>(&duplicate_text).is_err());
    for value in [
        json!({"queryId":"original_finalized_receipt_logs"}),
        json!({
            "queryId": "original_finalized_receipt_logs",
            "safeAddress": "0x0000000000000000000000000000000000000000",
            "safeTransactionHash": format!("0x{}", "00".repeat(32)),
            "observedAtBlockNumber": "0x81",
            "observedAtBlockHash": format!("0x{}", "00".repeat(32)),
            "receipts": [],
            "canonicalBlock": null,
            "finalized": true
        }),
    ] {
        let text = value.to_string();
        assert!(serde_json::from_str::<super::wire::ExecutionQueryWire<'_>>(&text).is_err());
    }
    let queries = ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
    let responses = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        Some(RequestKind::Original),
        None,
        None,
        SafeNonce::from_decimal("1").unwrap(),
        [[9; 32], [8; 32]],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .resolution(),
        RedemptionResolution::IntegrityFailure
    );
    let reorged = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        Some(RequestKind::Original),
        Some(RequestKind::Original),
        None,
        SafeNonce::from_decimal("1").unwrap(),
        [[0; 32]; 2],
    );
    assert_eq!(
        reorged
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .resolution(),
        RedemptionResolution::IntegrityFailure
    );
    let wrong_id = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        Some(RequestKind::Original),
        None,
        Some(RequestKind::Original),
        SafeNonce::from_decimal("1").unwrap(),
        [[0; 32]; 2],
    );
    assert_eq!(
        wrong_id
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .resolution(),
        RedemptionResolution::IntegrityFailure
    );
}

#[test]
fn sentinel_values_never_appear_in_redacted_projections() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        0x5a,
        SafeNonce::from_decimal("18446744073709551616").unwrap(),
    ));
    let projection = original.original().projection(&credentials);
    let rendered = format!("{projection:?}");
    assert!(!rendered.contains(OWNER));
    assert!(!rendered.contains("5a5a5a5a"));
    assert!(!rendered.contains("18446744073709551616"));
    assert!(!rendered.contains("hermetic-api-key"));
}

#[test]
fn sentinels_do_not_reach_redacted_diagnostics() {
    sentinel_values_never_appear_in_redacted_projections();
}

#[test]
fn primitive_is_mechanically_disabled() {
    assert!(!MECHANICALLY_ENABLED);
    assert!(
        validate_profile(
            &CONFIG.replacen(
                "competing_same_nonce_conformance = false",
                "competing_same_nonce_conformance = true",
                1
            ),
            MANIFEST
        )
        .is_err()
    );
}
