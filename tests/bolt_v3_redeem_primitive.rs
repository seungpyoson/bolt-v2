use bolt_v2::bolt_v3_providers::polymarket::redemption::{
    BoundedWireResponse, ExactQuery, ExactQuerySet, ExecutionProof, MarketMode, PostStateRelation,
    RedemptionBuildInput, RedemptionRequestError, RedemptionResolution, RelayerState,
    ResolutionObservation, WireFailureClass, activation_capable, build_request_pair,
    require_exact_retry, resolve_competing_nonce, resolve_credentials, revalidate_pre_send,
    validate_profile,
};

const CONFIG: &str = include_str!("../config/polymarket-redemption.toml");
const MANIFEST: &str = include_str!("../ci/polymarket-redemption-provider-manifest.toml");
const OWNER: &str = "0xf1c7000000000000000000000000000000000001";
const CONDITION: &str = "0x1000000000000000000000000000000000000000000000000000000000000001";
const SIGNATURE: [u8; 65] = [0x5a; 65];

fn profile() -> bolt_v2::bolt_v3_providers::polymarket::redemption::ValidatedRedemptionProfile {
    validate_profile(CONFIG, MANIFEST).expect("disabled fixture profile must validate")
}

fn pair(
    mode: MarketMode,
    metadata: &str,
) -> bolt_v2::bolt_v3_providers::polymarket::redemption::PreparedRequestPair {
    build_request_pair(
        &profile(),
        RedemptionBuildInput {
            mode,
            owner_address: OWNER,
            condition_id: CONDITION,
            safe_nonce: 7,
            pre_balances: [[0; 32], [1; 32]],
            metadata,
        },
    )
    .expect("fixture request pair must build")
}

#[test]
fn standard_and_negative_risk_fixtures() {
    let standard: toml::Value =
        toml::from_str(include_str!("fixtures/bolt_v3/redeem/standard.toml"))
            .expect("standard fixture must parse");
    let negative: toml::Value =
        toml::from_str(include_str!("fixtures/bolt_v3/redeem/negative-risk.toml"))
            .expect("negative-risk fixture must parse");
    assert_eq!(standard["market_mode"].as_str(), Some("standard"));
    assert_eq!(negative["market_mode"].as_str(), Some("negative_risk"));
    for fixture in [&standard, &negative] {
        let edges = fixture["scaled_integer_edges"]
            .as_array()
            .expect("scaled-integer edges must be an exact string array");
        assert_eq!(edges.first().and_then(toml::Value::as_str), Some("0"));
        assert_eq!(edges.get(1).and_then(toml::Value::as_str), Some("1"));
        assert_eq!(
            edges.last().and_then(toml::Value::as_str),
            Some("340282366920938463463374607431768211455")
        );
    }

    let standard_pair = pair(MarketMode::Standard, "fixture");
    let negative_pair = pair(MarketMode::NegativeRisk, "fixture");
    assert_ne!(
        standard_pair.original.identity().target,
        negative_pair.original.identity().target
    );
    assert_eq!(
        standard_pair.original.identity().calldata_hash,
        negative_pair.original.identity().calldata_hash,
        "both modes must call the same reviewed external four-argument ABI"
    );
}

#[test]
fn original_and_fence_body_boundaries() {
    let profile = profile();
    for size in [0, profile.config.relayer.max_metadata_bytes] {
        let metadata = "m".repeat(size);
        let requests = pair(MarketMode::Standard, &metadata);
        let original = requests
            .original
            .finalize(&profile, &SIGNATURE)
            .expect("original at legal metadata boundary must fit");
        let fence = requests
            .fence
            .finalize(&profile, &SIGNATURE)
            .expect("fence at legal metadata boundary must fit");
        assert!(original.len() <= profile.config.relayer.max_request_bytes);
        assert!(fence.len() <= profile.config.relayer.max_request_bytes);
        assert_eq!(original.as_bytes().first(), Some(&b'{'));
        assert_eq!(fence.as_bytes().first(), Some(&b'{'));
        assert_ne!(
            original.descriptor().safe_transaction_hash,
            fence.descriptor().safe_transaction_hash
        );
        assert_eq!(
            requests.original.finalize(&profile, &[0; 64]),
            Err(RedemptionRequestError::SignatureLength)
        );
        assert_eq!(
            requests.fence.finalize(&profile, &[0; 66]),
            Err(RedemptionRequestError::SignatureLength)
        );
    }
    let too_large = "m".repeat(profile.config.relayer.max_metadata_bytes + 1);
    assert_eq!(
        build_request_pair(
            &profile,
            RedemptionBuildInput {
                mode: MarketMode::Standard,
                owner_address: OWNER,
                condition_id: CONDITION,
                safe_nonce: 7,
                pre_balances: [[0; 32], [1; 32]],
                metadata: &too_large,
            },
        ),
        Err(RedemptionRequestError::MetadataTooLarge)
    );
}

#[test]
fn response_loss_requires_exact_queries() {
    let profile = profile();
    let requests = pair(MarketMode::Standard, "response-loss");
    let query = ExactQuerySet::for_response_loss(&profile, &requests, None);
    assert_eq!(query.queries.len(), 5);
    assert_eq!(
        query
            .queries
            .iter()
            .filter(|item| matches!(item, ExactQuery::SafeExecution { .. }))
            .count(),
        2
    );
    assert!(
        query
            .queries
            .iter()
            .any(|item| matches!(item, ExactQuery::RawPostState { .. }))
    );
    assert!(
        query
            .queries
            .iter()
            .any(|item| matches!(item, ExactQuery::SafeBoundary { .. }))
    );
    assert!(matches!(
        ExactQuerySet::finalized_receipt(&profile, [9; 32]),
        ExactQuery::FinalizedReceipt { .. }
    ));
}

fn observation(post_state: PostStateRelation) -> ResolutionObservation {
    let requests = pair(MarketMode::Standard, "resolution");
    ResolutionObservation {
        prepared_nonce: 7,
        on_chain_nonce: 8,
        original_safe_transaction_hash: requests.original.safe_transaction_hash(),
        fence_safe_transaction_hash: requests.fence.safe_transaction_hash(),
        original_execution: None,
        fence_execution: None,
        post_state,
    }
}

#[test]
fn original_wins_only_with_finalized_post_state() {
    let mut value = observation(PostStateRelation::Redeemed);
    value.original_execution = Some(ExecutionProof {
        safe_transaction_hash: value.original_safe_transaction_hash,
        finalized: true,
        safe_execution_succeeded: true,
        compatible_logs: true,
    });
    assert_eq!(
        resolve_competing_nonce(&value),
        RedemptionResolution::RedemptionFinalized
    );
    value.original_execution.as_mut().unwrap().finalized = false;
    assert_eq!(
        resolve_competing_nonce(&value),
        RedemptionResolution::IntegrityFailure
    );
}

#[test]
fn fence_wins_only_with_unchanged_post_state() {
    let mut value = observation(PostStateRelation::Unchanged);
    value.fence_execution = Some(ExecutionProof {
        safe_transaction_hash: value.fence_safe_transaction_hash,
        finalized: true,
        safe_execution_succeeded: true,
        compatible_logs: true,
    });
    assert_eq!(
        resolve_competing_nonce(&value),
        RedemptionResolution::PermanentlyFencedNoEffect
    );
    value.post_state = PostStateRelation::Drifted;
    assert_eq!(
        resolve_competing_nonce(&value),
        RedemptionResolution::IntegrityFailure
    );
}

#[test]
fn unrelated_nonce_fails_closed() {
    let mut value = observation(PostStateRelation::Unchanged);
    value.on_chain_nonce = 9;
    assert_eq!(
        resolve_competing_nonce(&value),
        RedemptionResolution::IntegrityFailure
    );
    value.on_chain_nonce = 8;
    assert_eq!(
        resolve_competing_nonce(&value),
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
    let first = pair(MarketMode::Standard, "retry")
        .original
        .finalize(&profile, &SIGNATURE)
        .unwrap();
    let identical = pair(MarketMode::Standard, "retry")
        .original
        .finalize(&profile, &SIGNATURE)
        .unwrap();
    let changed = pair(MarketMode::Standard, "retry-changed")
        .original
        .finalize(&profile, &SIGNATURE)
        .unwrap();
    assert_eq!(require_exact_retry(&first, &identical), Ok(()));
    assert_eq!(
        require_exact_retry(&first, &changed),
        Err(RedemptionRequestError::RetryMismatch)
    );
}

#[test]
fn pre_send_balance_and_lease_revalidation_fails_closed() {
    let requests = pair(MarketMode::Standard, "revalidate");
    assert_eq!(
        revalidate_pre_send(&requests, requests.pre_balances, true),
        Ok(())
    );
    assert_eq!(
        revalidate_pre_send(&requests, [[2; 32], [1; 32]], true),
        Err(RedemptionRequestError::PreSendDrift)
    );
    assert_eq!(
        revalidate_pre_send(&requests, requests.pre_balances, false),
        Err(RedemptionRequestError::ExclusiveLeaseUnavailable)
    );
}

#[test]
fn sentinels_do_not_reach_redacted_diagnostics() {
    let sentinel = "SENTINEL_PRIVATE_KEY_SENTINEL_SIGNATURE_SENTINEL_RESPONSE";
    let malformed =
        BoundedWireResponse::from_relayer(&profile(), sentinel.as_bytes().to_vec()).unwrap();
    let error = malformed.parse_submit(&profile()).unwrap_err();
    assert_eq!(error.diagnostic.class, WireFailureClass::Malformed);
    assert!(!format!("{error:?}").contains(sentinel));
    assert!(!format!("{malformed:?}").contains(sentinel));

    let oversize = BoundedWireResponse::from_relayer(
        &profile(),
        vec![b'x'; profile().config.relayer.max_response_bytes + 1],
    )
    .unwrap_err();
    assert!(!format!("{oversize:?}").contains(sentinel));

    let successful = format!(
        r#"[{{"transactionID":"exact-id","transactionHash":"","from":"{sentinel}","to":"0x0","proxyAddress":"0x0","data":"{sentinel}","nonce":"7","value":"0","state":"STATE_NEW","type":"SAFE","metadata":"{sentinel}","createdAt":"time","updatedAt":"time"}}]"#
    );
    let parsed = BoundedWireResponse::from_relayer(&profile(), successful.into_bytes())
        .unwrap()
        .parse_exact_transaction(&profile(), "exact-id")
        .unwrap();
    assert!(!format!("{parsed:?}").contains(sentinel));

    let failed =
        format!(r#"{{"transactionID":"exact-id","state":"{sentinel}","transactionHash":""}}"#);
    let failed = BoundedWireResponse::from_relayer(&profile(), failed.into_bytes())
        .unwrap()
        .parse_submit(&profile())
        .unwrap_err();
    assert_eq!(failed.diagnostic.class, WireFailureClass::UnknownState);
    assert!(!format!("{failed:?}").contains(sentinel));
}

#[test]
fn primitive_is_mechanically_disabled() {
    let profile = profile();
    assert!(!activation_capable(&profile));
    let enabled = CONFIG.replacen("enabled = false", "enabled = true", 1);
    assert!(validate_profile(&enabled, MANIFEST).is_err());
    let drifted_target = CONFIG.replacen(
        "0xADa100874d00e3331D00F2007a9c336a65009718",
        "0x0000000000000000000000000000000000000001",
        1,
    );
    assert!(validate_profile(&drifted_target, MANIFEST).is_err());

    let mut paths = Vec::new();
    let credentials = resolve_credentials(&profile, "fixture-region", &mut |_, path| {
        paths.push(path.to_string());
        Ok::<_, String>(format!("sentinel-for-{path}"))
    })
    .expect("fixture SSM resolver must supply every grouped credential");
    assert_eq!(paths.len(), 4);
    let rendered = format!("{credentials:?}");
    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains("sentinel-for-"));
}
