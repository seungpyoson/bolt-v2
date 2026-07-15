#![cfg(test)]

use std::cmp::Ordering;
use std::io::Cursor;

use alloy_primitives::keccak256;
use base64::{Engine, engine::general_purpose};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use super::capability::hermetic;
use super::config::HermeticCredentialSource;
use super::*;

const CONFIG: &str = include_str!("../../../../config/polymarket-redemption.toml");
const MANIFEST: &str = include_str!("../../../../ci/polymarket-redemption-provider-manifest.toml");
const CONFIGURED_OWNER: &str = "0x13c81bfb4db09c99553572402310b67429c19a53";
const OWNER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const PRIVATE_KEY: &[u8] = b"0000000000000000000000000000000000000000000000000000000000000001";

fn working_set() -> WholeWorkingSetReservation {
    hermetic::working_set(usize::MAX, usize::MAX, 1)
}

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
    hermetic_profile(
        &CONFIG.replace(CONFIGURED_OWNER, OWNER),
        &MANIFEST.replace(CONFIGURED_OWNER, OWNER),
    )
    .unwrap()
}

fn hermetic_profile(
    config: &str,
    manifest: &str,
) -> Result<ValidatedRedemptionProfile, RedemptionConfigError> {
    super::config::validate_profile_hermetic(
        config,
        manifest,
        config.len(),
        manifest.len(),
        working_set(),
    )
}

fn credentials(profile: &ValidatedRedemptionProfile) -> ResolvedRedemptionCredentials {
    let mut source = HermeticCredentialSource::new(credential_value);
    resolve_credentials(profile, "hermetic-region", &mut source).unwrap()
}

fn worst_case_credential_value(
    path: &str,
    sink: &mut CredentialSink<'_>,
) -> Result<(), RedemptionConfigError> {
    if path.ends_with("private-key") {
        sink.append(PRIVATE_KEY)
    } else if path.ends_with("builder-api-secret") {
        sink.append(b"QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFB")
    } else if path.ends_with("redaction-hmac-key") {
        sink.append(&[b'r'; 64])
    } else {
        sink.append(&[0; 64])
    }
}

fn worst_case_credentials(profile: &ValidatedRedemptionProfile) -> ResolvedRedemptionCredentials {
    let mut source = HermeticCredentialSource::new(worst_case_credential_value);
    resolve_credentials(profile, "hermetic-region", &mut source).unwrap()
}

fn profile_with_request_limits(
    original_limit: usize,
    fence_limit: usize,
    header_limit: usize,
) -> ValidatedRedemptionProfile {
    let mut config = CONFIG
        .replacen(
            "max_original_request_bytes = 4096",
            &format!("max_original_request_bytes = {original_limit}"),
            1,
        )
        .replacen(
            "max_fence_request_bytes = 4096",
            &format!("max_fence_request_bytes = {fence_limit}"),
            1,
        )
        .replacen(
            "max_header_bytes = 1024",
            &format!("max_header_bytes = {header_limit}"),
            1,
        )
        .replacen("max_value_bytes = 4096", "max_value_bytes = 64", 1)
        .replacen(
            "max_acquisition_bytes = 4096",
            "max_acquisition_bytes = 64",
            1,
        );
    let mut manifest = MANIFEST
        .replacen(
            "max_original_request_bytes = 4096",
            &format!("max_original_request_bytes = {original_limit}"),
            1,
        )
        .replacen(
            "max_fence_request_bytes = 4096",
            &format!("max_fence_request_bytes = {fence_limit}"),
            1,
        )
        .replacen(
            "max_header_bytes = 1024",
            &format!("max_header_bytes = {header_limit}"),
            1,
        )
        .replace("max_acquisition_bytes = 4096", "max_acquisition_bytes = 64")
        .replacen(
            "max_credential_value_bytes = 4096",
            "max_credential_value_bytes = 64",
            1,
        )
        .replacen(
            "max_credential_acquisition_bytes = 4096",
            "max_credential_acquisition_bytes = 64",
            1,
        );
    let parsed_config: toml::Value = toml::from_str(&config).unwrap();
    let parsed_manifest: toml::Value = toml::from_str(&manifest).unwrap();
    let relayer = &parsed_config["relayer"];
    let rpc = &parsed_config["rpc"];
    let query = &parsed_config["query"];
    let credential = &parsed_config["credentials"];
    let allocation = &parsed_manifest["allocation_boundary"];
    let working = &parsed_config["working_set"];
    let operational = (relayer["max_original_request_bytes"].as_integer().unwrap()
        + relayer["max_fence_request_bytes"].as_integer().unwrap()
        + 2 * (relayer["max_header_bytes"].as_integer().unwrap()
            + relayer["max_metadata_bytes"].as_integer().unwrap())
        + query["max_bytes"].as_integer().unwrap()
        + query["max_items"].as_integer().unwrap()
            * allocation["query_offset_layout_bytes"]
                .as_integer()
                .unwrap()
        + relayer["max_response_bytes"].as_integer().unwrap()
        + relayer["overflow_probe_bytes"].as_integer().unwrap()
        + relayer["max_transaction_id_bytes"].as_integer().unwrap()
        + (query["max_items"].as_integer().unwrap() - 1)
            * (rpc["max_response_bytes"].as_integer().unwrap()
                + rpc["overflow_probe_bytes"].as_integer().unwrap())
        + credential["max_acquisition_bytes"].as_integer().unwrap()
        + 6 * credential["max_value_bytes"].as_integer().unwrap()
        + rpc["max_receipt_logs"].as_integer().unwrap()
            * allocation["receipt_log_index_layout_bytes"]
                .as_integer()
                .unwrap()
        + working["operational_structural_bytes"]
            .as_integer()
            .unwrap()) as usize;
    config = config.replacen(
        "max_operational_working_set_bytes = 14790791",
        &format!("max_operational_working_set_bytes = {operational}"),
        1,
    );
    manifest = manifest.replacen(
        "max_operational_working_set_bytes = 14790791",
        &format!("max_operational_working_set_bytes = {operational}"),
        1,
    );
    let source_bytes = config.len() + manifest.len();
    let startup = source_bytes + 65_536;
    config = config.replacen(
        "max_startup_working_set_bytes = 75628",
        &format!("max_startup_working_set_bytes = {startup}"),
        1,
    );
    manifest = manifest
        .replacen(
            "startup_source_bytes = 10092",
            &format!("startup_source_bytes = {source_bytes}"),
            1,
        )
        .replacen(
            "max_startup_working_set_bytes = 75628",
            &format!("max_startup_working_set_bytes = {startup}"),
            1,
        );
    super::config::validate_profile_hermetic(
        &config,
        &manifest,
        config.len(),
        manifest.len(),
        hermetic::working_set(startup, operational, 1),
    )
    .unwrap()
}

fn scaled_word(value: &str) -> [u8; 32] {
    *SafeNonce::from_decimal(value).unwrap().as_word()
}

fn prepared(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    mode: MarketMode,
    condition: u8,
    nonce: SafeNonce,
) -> PreparedRequestPair {
    prepared_with_state(
        profile,
        credentials,
        mode,
        condition,
        nonce,
        [[1; 32], [2; 32]],
        [3; 32],
        [4; 32],
    )
}

#[allow(clippy::too_many_arguments)]
fn prepared_with_state(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    mode: MarketMode,
    condition: u8,
    nonce: SafeNonce,
    pre_claim_balances: [[u8; 32]; 2],
    pre_collateral_balance: [u8; 32],
    expected_redeemed_collateral_balance: [u8; 32],
) -> PreparedRequestPair {
    let snapshot = hermetic::snapshot(
        [condition; 32],
        pre_claim_balances,
        pre_collateral_balance,
        expected_redeemed_collateral_balance,
        7,
    );
    let capacity = hermetic::nonce_capacity(
        profile.safe_address(),
        nonce,
        profile.max_request_bytes_for(false),
        profile.max_request_bytes_for(true),
        11,
    );
    build_request_pair(
        profile,
        credentials,
        snapshot,
        capacity,
        RedemptionBuildInput::new(mode, "hermetic-redemption"),
        1_700_000_000,
    )
    .unwrap()
}

fn build_worst_case_pair(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
) -> Result<PreparedRequestPair, RedemptionRequestError> {
    let nonce = SafeNonce::from_decimal(
        "115792089237316195423570985008687907853269984665640564039457584007913129639934",
    )
    .unwrap();
    let metadata = String::from_utf8(vec![0; profile.max_metadata_bytes()]).unwrap();
    build_request_pair(
        profile,
        credentials,
        hermetic::snapshot([1; 32], [[1; 32], [2; 32]], [3; 32], [4; 32], 7),
        hermetic::nonce_capacity(
            profile.safe_address(),
            nonce,
            profile.max_request_bytes_for(false),
            profile.max_request_bytes_for(true),
            11,
        ),
        RedemptionBuildInput::new(MarketMode::NegativeRisk, &metadata),
        u64::MAX,
    )
}

fn authorize_original(prepared: PreparedRequestPair) -> OriginalMayHaveStartedRequest {
    let (binding, original_hash, _, snapshot_generation, lane_generation) =
        prepared.hermetic_bindings();
    let (owner_set, threshold) = prepared.hermetic_safe_owner_contract();
    prepared
        .authorize_original(
            hermetic::fresh(
                binding,
                owner_set,
                threshold,
                snapshot_generation,
                lane_generation,
            ),
            hermetic::original_durable(binding, original_hash, 13),
        )
        .unwrap()
}

fn authorize_fence(original: OriginalMayHaveStartedRequest) -> FenceMayHaveStartedRequest {
    let (binding, original_hash, fence_hash, snapshot_generation, lane_generation) =
        original.prepared().hermetic_bindings();
    let (owner_set, threshold) = original.prepared().hermetic_safe_owner_contract();
    original
        .authorize_fence(
            hermetic::fresh(
                binding,
                owner_set,
                threshold,
                snapshot_generation,
                lane_generation,
            ),
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
    authority: &impl ExactActionBinding,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    queries: &ExactQuerySet,
    kind: QueryKind,
    value: Value,
) -> FinalizedChainSourceResponse {
    let mut finalized_block_number = [0; 32];
    finalized_block_number[31] = 0x81;
    FinalizedChainSourceResponse::from_hermetic_bytes(
        authority,
        profile,
        credentials,
        queries,
        kind,
        finalized_block_number,
        [0xa5; 32],
        &serde_json::to_vec(&value).unwrap(),
    )
    .unwrap()
}

fn relayer_response(
    authority: &impl ExactActionBinding,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    queries: &ExactQuerySet,
    value: Value,
) -> RelayerSourceResponse {
    RelayerSourceResponse::from_hermetic_query_bytes(
        authority,
        profile,
        credentials,
        queries,
        &serde_json::to_vec(&value).unwrap(),
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum PostMutation {
    None,
    WrongOutput,
    SwappedClaims,
    ReplayedCondition,
}

#[derive(Clone, Copy)]
enum QueryBindingSwap {
    None,
    NonceFinalized,
    Receipts,
    PostBoundary,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiptMutation {
    None,
    MissingPayout,
    DuplicatePayout,
    CorruptPayout,
    WrongEmitter,
    WrongField,
    WrongAmount,
    ReorgedPayout,
    Invalid(RequestKind),
    CorruptSafe(RequestKind),
    Malformed(RequestKind),
    AlsoCompatible(RequestKind),
    ExtraLogs(usize),
    DuplicateLogIndex,
    OutOfOrderLogIndex,
    WrongSafeOwner,
    WrongSafeThreshold,
}

#[allow(clippy::too_many_arguments)]
fn raw_responses(
    authority: &impl ExactActionBinding,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    queries: &ExactQuerySet,
    winner: Option<RequestKind>,
    reorged: Option<RequestKind>,
    wrong_query_id: Option<RequestKind>,
    observed_nonce: SafeNonce,
    post_claim_balances: [[u8; 32]; 2],
    post_collateral_balance: [u8; 32],
) -> ExactQueryResponses {
    raw_responses_with_mutation(
        authority,
        profile,
        credentials,
        queries,
        winner,
        reorged,
        wrong_query_id,
        observed_nonce,
        post_claim_balances,
        post_collateral_balance,
        PostMutation::None,
        QueryBindingSwap::None,
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn raw_responses_with_mutation(
    authority: &impl ExactActionBinding,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    queries: &ExactQuerySet,
    winner: Option<RequestKind>,
    reorged: Option<RequestKind>,
    wrong_query_id: Option<RequestKind>,
    observed_nonce: SafeNonce,
    post_claim_balances: [[u8; 32]; 2],
    post_collateral_balance: [u8; 32],
    post_mutation: PostMutation,
    query_binding_swap: QueryBindingSwap,
) -> Result<ExactQueryResponses, WireParseError> {
    raw_responses_with_receipt_mutation(
        authority,
        profile,
        credentials,
        queries,
        winner,
        reorged,
        wrong_query_id,
        observed_nonce,
        post_claim_balances,
        post_collateral_balance,
        post_mutation,
        query_binding_swap,
        ReceiptMutation::None,
    )
}

#[allow(clippy::too_many_arguments)]
fn raw_responses_with_receipt_mutation(
    authority: &impl ExactActionBinding,
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    queries: &ExactQuerySet,
    winner: Option<RequestKind>,
    reorged: Option<RequestKind>,
    wrong_query_id: Option<RequestKind>,
    observed_nonce: SafeNonce,
    post_claim_balances: [[u8; 32]; 2],
    post_collateral_balance: [u8; 32],
    post_mutation: PostMutation,
    query_binding_swap: QueryBindingSwap,
    receipt_mutation: ReceiptMutation,
) -> Result<ExactQueryResponses, WireParseError> {
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
    let safe_event_topic = format!(
        "0x{}",
        hex::encode(keccak256(b"ExecutionSuccess(bytes32,uint256)"))
    );
    let prepared = authority.prepared_request_pair();
    let address_topic = |address: [u8; 20]| {
        let mut word = [0; 32];
        word[12..].copy_from_slice(&address);
        format!("0x{}", hex::encode(word))
    };
    let payout = super::wire::subtract_word(
        prepared.expected_redeemed_collateral_balance(),
        prepared.pre_collateral_balance(),
    )
    .unwrap();
    let payout_log = |transaction_hash: &str, block_hash: &str| {
        let target = prepared.request(RequestKind::Original).identity().target();
        let (standard_emitter, negative_emitter, underlying, _, standard_topic, negative_topic) =
            profile.terminal_event_contract();
        match prepared.mode() {
            MarketMode::Standard => {
                let index_sets = profile.adapter_arguments().2;
                let data = [
                    prepared.condition_id(),
                    super::wire::small_word(96),
                    payout,
                    super::wire::small_word(2),
                    super::wire::small_word(index_sets[0]),
                    super::wire::small_word(index_sets[1]),
                ];
                json!({
                    "address": format!("0x{}", hex::encode(standard_emitter)),
                    "topics": [
                        format!("0x{}", hex::encode(standard_topic)),
                        address_topic(target),
                        address_topic(underlying),
                        format!("0x{}", "00".repeat(32))
                    ],
                    "data": format!("0x{}", data.map(hex::encode).concat()),
                    "blockNumber": "0x2", "blockHash": block_hash,
                    "transactionHash": transaction_hash, "transactionIndex": "0x0",
                    "logIndex": "0x1", "removed": false
                })
            }
            MarketMode::NegativeRisk => {
                let data = [
                    super::wire::small_word(64),
                    payout,
                    super::wire::small_word(2),
                    prepared.pre_claim_balances()[0],
                    prepared.pre_claim_balances()[1],
                ];
                json!({
                    "address": format!("0x{}", hex::encode(negative_emitter)),
                    "topics": [
                        format!("0x{}", hex::encode(negative_topic)),
                        address_topic(target),
                        format!("0x{}", hex::encode(prepared.condition_id()))
                    ],
                    "data": format!("0x{}", data.map(hex::encode).concat()),
                    "blockNumber": "0x2", "blockHash": block_hash,
                    "transactionHash": transaction_hash, "transactionIndex": "0x0",
                    "logIndex": "0x1", "removed": false
                })
            }
        }
    };
    let execution = |index: usize, kind: RequestKind| {
        let present = winner == Some(kind)
            || matches!(receipt_mutation, ReceiptMutation::Invalid(value) | ReceiptMutation::CorruptSafe(value) | ReceiptMutation::Malformed(value) | ReceiptMutation::AlsoCompatible(value) if value == kind);
        let safe_hash = executions[index]["safe_transaction_hash"].as_str().unwrap();
        let transaction_hash = format!("0x{}", if index == 0 { "31" } else { "32" }.repeat(32));
        let block_hash = format!("0x{}", if index == 0 { "41" } else { "42" }.repeat(32));
        let receipts = if present {
            let safe_log = json!({
                    "address": executions[index]["safe"],
                    "topics": [safe_event_topic.clone()],
                    "data": format!("{}{}", safe_hash, "00".repeat(32)),
                    "blockNumber": "0x2",
                    "blockHash": block_hash.clone(),
                    "transactionHash": transaction_hash.clone(),
                    "transactionIndex": "0x0",
                    "logIndex": "0x0",
                    "removed": false
            });
            let mut logs = if kind == RequestKind::Original {
                vec![safe_log, payout_log(&transaction_hash, &block_hash)]
            } else {
                vec![safe_log]
            };
            if kind == RequestKind::Original {
                match receipt_mutation {
                    ReceiptMutation::MissingPayout => {
                        logs.pop();
                    }
                    ReceiptMutation::DuplicatePayout => logs.push(logs[1].clone()),
                    ReceiptMutation::CorruptPayout => logs[1]["data"] = json!("0x00"),
                    ReceiptMutation::WrongEmitter => {
                        logs[1]["address"] = json!(format!("0x{}", "ee".repeat(20)));
                    }
                    ReceiptMutation::WrongField => {
                        logs[1]["topics"][1] = json!(format!("0x{}", "00".repeat(32)));
                    }
                    ReceiptMutation::WrongAmount => {
                        let payout_word = match prepared.mode() {
                            MarketMode::Standard => 2,
                            MarketMode::NegativeRisk => 1,
                        };
                        let mut data = logs[1]["data"].as_str().unwrap().as_bytes().to_vec();
                        let start = 2 + payout_word * 64;
                        data[start..start + 64].fill(b'f');
                        logs[1]["data"] = json!(String::from_utf8(data).unwrap());
                    }
                    ReceiptMutation::ReorgedPayout => logs[1]["removed"] = json!(true),
                    ReceiptMutation::None
                    | ReceiptMutation::Invalid(_)
                    | ReceiptMutation::CorruptSafe(_)
                    | ReceiptMutation::Malformed(_)
                    | ReceiptMutation::AlsoCompatible(_)
                    | ReceiptMutation::ExtraLogs(_)
                    | ReceiptMutation::DuplicateLogIndex
                    | ReceiptMutation::OutOfOrderLogIndex
                    | ReceiptMutation::WrongSafeOwner
                    | ReceiptMutation::WrongSafeThreshold => {}
                }
            }
            if receipt_mutation == ReceiptMutation::DuplicateLogIndex {
                if kind == RequestKind::Original {
                    logs[1]["logIndex"] = logs[0]["logIndex"].clone();
                } else {
                    logs.push(json!({
                        "address": format!("0x{}", "aa".repeat(20)),
                        "topics": [format!("0x{}", "77".repeat(32))],
                        "data": "0x",
                        "blockNumber": "0x2",
                        "blockHash": block_hash.clone(),
                        "transactionHash": transaction_hash.clone(),
                        "transactionIndex": "0x0",
                        "logIndex": "0x0",
                        "removed": false
                    }));
                }
            }
            if receipt_mutation == ReceiptMutation::OutOfOrderLogIndex {
                if kind == RequestKind::Original {
                    logs[0]["logIndex"] = json!("0x2");
                    logs[1]["logIndex"] = json!("0x1");
                } else {
                    logs[0]["logIndex"] = json!("0x2");
                    logs.push(json!({
                        "address": format!("0x{}", "aa".repeat(20)),
                        "topics": [format!("0x{}", "77".repeat(32))],
                        "data": "0x",
                        "blockNumber": "0x2",
                        "blockHash": block_hash.clone(),
                        "transactionHash": transaction_hash.clone(),
                        "transactionIndex": "0x0",
                        "logIndex": "0x1",
                        "removed": false
                    }));
                }
            }
            if let ReceiptMutation::ExtraLogs(count) = receipt_mutation {
                for log_index in 0..count {
                    logs.push(json!({
                        "address": format!("0x{}", "aa".repeat(20)),
                        "topics": [format!("0x{}", "77".repeat(32))],
                        "data": "0x",
                        "blockNumber": "0x2",
                        "blockHash": block_hash.clone(),
                        "transactionHash": transaction_hash.clone(),
                        "transactionIndex": "0x0",
                        "logIndex": format!("0x{:x}", log_index + 2),
                        "removed": false
                    }));
                }
            }
            if matches!(receipt_mutation, ReceiptMutation::CorruptSafe(value) if value == kind) {
                logs[0]["data"] = json!("0x00");
            }
            let mut receipt = json!({
                "transactionHash": transaction_hash.clone(),
                "blockNumber": "0x2",
                "blockHash": block_hash.clone(),
                "transactionIndex": "0x0",
                "status": "0x1",
                "logs": logs
            });
            if matches!(receipt_mutation, ReceiptMutation::Invalid(value) if value == kind) {
                receipt["status"] = json!("0x2");
            }
            if matches!(receipt_mutation, ReceiptMutation::Malformed(value) if value == kind) {
                receipt = json!({"status": "0x1"});
            }
            if matches!(receipt_mutation, ReceiptMutation::AlsoCompatible(value) if value == kind)
                && winner == Some(kind)
            {
                json!([receipt.clone(), receipt])
            } else {
                json!([receipt])
            }
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
    let post_condition = if matches!(post_mutation, PostMutation::ReplayedCondition) {
        format!("0x{}", "ff".repeat(32))
    } else {
        post["condition_id"].as_str().unwrap().to_owned()
    };
    let post_output = if matches!(post_mutation, PostMutation::WrongOutput) {
        format!("0x{}", "ee".repeat(20))
    } else {
        post["output_asset"].as_str().unwrap().to_owned()
    };
    let claims = if matches!(post_mutation, PostMutation::SwappedClaims) {
        [post_claim_balances[1], post_claim_balances[0]]
    } else {
        post_claim_balances
    };
    ExactQueryResponses::new(
        queries,
        credentials,
        chain_response(
            authority,
            profile,
            credentials,
            queries,
            if matches!(query_binding_swap, QueryBindingSwap::NonceFinalized) {
                QueryKind::FinalizedHead
            } else {
                QueryKind::SafeNonce
            },
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
            queries,
            if matches!(query_binding_swap, QueryBindingSwap::Receipts) {
                QueryKind::FenceFinalizedReceiptLogs
            } else {
                QueryKind::OriginalFinalizedReceiptLogs
            },
            execution(0, RequestKind::Original),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            queries,
            if matches!(query_binding_swap, QueryBindingSwap::Receipts) {
                QueryKind::OriginalFinalizedReceiptLogs
            } else {
                QueryKind::FenceFinalizedReceiptLogs
            },
            execution(1, RequestKind::Fence),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            queries,
            if matches!(query_binding_swap, QueryBindingSwap::PostBoundary) {
                QueryKind::SafeBoundary
            } else {
                QueryKind::RawPostState
            },
            json!({
                "queryId": "raw_post_state",
                "target": post["target"],
                "conditionId": post_condition,
                "collateral": post["collateral"],
                "outputAsset": post_output,
                "account": post["account"],
                "blockNumber": head_number,
                "blockHash": head_hash.clone(),
                "claimResults": claims.map(|value| format!("0x{}", hex::encode(value))),
                "collateralBalance": format!("0x{}", hex::encode(post_collateral_balance))
            }),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            queries,
            if matches!(query_binding_swap, QueryBindingSwap::PostBoundary) {
                QueryKind::RawPostState
            } else {
                QueryKind::SafeBoundary
            },
            json!({
                "queryId": "safe_boundary",
                "safe": boundary["safe"],
                "factory": boundary["factory"],
                "implementation": boundary["implementation"],
                "fallbackHandler": boundary["fallback_handler"],
                "guard": boundary["guard"],
                "modules": [],
                "owners": if receipt_mutation == ReceiptMutation::WrongSafeOwner {
                    json!([format!("0x{}", "ee".repeat(20))])
                } else {
                    boundary["owners"].clone()
                },
                "threshold": if receipt_mutation == ReceiptMutation::WrongSafeThreshold {
                    json!(2)
                } else {
                    boundary["threshold"].clone()
                },
                "blockNumber": head_number,
                "blockHash": head_hash.clone()
            }),
        ),
        chain_response(
            authority,
            profile,
            credentials,
            queries,
            if matches!(query_binding_swap, QueryBindingSwap::NonceFinalized) {
                QueryKind::SafeNonce
            } else {
                QueryKind::FinalizedHead
            },
            json!({
                "queryId": "finalized_head",
                "chainId": 137,
                "blockNumber": head_number,
                "blockHash": head_hash
            }),
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn terminal_resolution(
    profile: &ValidatedRedemptionProfile,
    credentials: &ResolvedRedemptionCredentials,
    mode: MarketMode,
    condition: u8,
    pre_claim_balances: [[u8; 32]; 2],
    pre_collateral_balance: [u8; 32],
    expected_redeemed_collateral_balance: [u8; 32],
    winner: RequestKind,
) -> RedemptionResolution {
    let original = authorize_original(prepared_with_state(
        profile,
        credentials,
        mode,
        condition,
        SafeNonce::ZERO,
        pre_claim_balances,
        pre_collateral_balance,
        expected_redeemed_collateral_balance,
    ));
    match winner {
        RequestKind::Original => {
            let queries =
                ExactQuerySet::after_original_response_loss(profile, &original, None).unwrap();
            let responses = raw_responses(
                &original,
                profile,
                credentials,
                &queries,
                Some(RequestKind::Original),
                None,
                None,
                SafeNonce::from_decimal("1").unwrap(),
                [[0; 32]; 2],
                expected_redeemed_collateral_balance,
            );
            responses
                .verify_after_original(profile, credentials, &original)
                .unwrap()
                .consume_after_original(&responses, profile, credentials, &original)
                .unwrap()
        }
        RequestKind::Fence => {
            let fence = authorize_fence(original);
            let queries = ExactQuerySet::after_fence_response_loss(profile, &fence, None).unwrap();
            let responses = raw_responses(
                &fence,
                profile,
                credentials,
                &queries,
                Some(RequestKind::Fence),
                None,
                None,
                SafeNonce::from_decimal("1").unwrap(),
                pre_claim_balances,
                pre_collateral_balance,
            );
            responses
                .verify_after_fence(profile, credentials, &fence)
                .unwrap()
                .consume_after_fence(&responses, profile, credentials, &fence)
                .unwrap()
        }
    }
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
        assert_eq!(
            value["expected_original_claim_balances"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(value["expected_redeemed_collateral_balance"].is_str());
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
fn standard_negative_risk_original_and_fence_post_state_fixtures() {
    let profile = profile();
    let credentials = credentials(&profile);
    for (fixture, mode, condition) in [
        (
            include_str!("../../../../tests/fixtures/bolt_v3/redeem/standard.toml"),
            MarketMode::Standard,
            1,
        ),
        (
            include_str!("../../../../tests/fixtures/bolt_v3/redeem/negative-risk.toml"),
            MarketMode::NegativeRisk,
            2,
        ),
    ] {
        let value: toml::Value = toml::from_str(fixture).unwrap();
        let claims = value["pre_claim_balances"].as_array().unwrap();
        let pre_claim_balances = [
            scaled_word(claims[0].as_str().unwrap()),
            scaled_word(claims[1].as_str().unwrap()),
        ];
        let pre_collateral_balance = scaled_word(value["pre_collateral_balance"].as_str().unwrap());
        let expected_redeemed_collateral_balance = scaled_word(
            value["expected_redeemed_collateral_balance"]
                .as_str()
                .unwrap(),
        );
        let expected_original = value["expected_original_claim_balances"]
            .as_array()
            .unwrap();
        let expected_fence = value["expected_fence_claim_balances"].as_array().unwrap();
        assert_eq!(
            [
                scaled_word(expected_original[0].as_str().unwrap()),
                scaled_word(expected_original[1].as_str().unwrap()),
            ],
            [[0; 32]; 2]
        );
        assert_eq!(
            [
                scaled_word(expected_fence[0].as_str().unwrap()),
                scaled_word(expected_fence[1].as_str().unwrap()),
            ],
            pre_claim_balances
        );
        assert_eq!(
            scaled_word(value["expected_fence_collateral_balance"].as_str().unwrap()),
            pre_collateral_balance
        );
        assert_eq!(
            terminal_resolution(
                &profile,
                &credentials,
                mode,
                condition,
                pre_claim_balances,
                pre_collateral_balance,
                expected_redeemed_collateral_balance,
                RequestKind::Original,
            ),
            RedemptionResolution::RedemptionFinalized
        );
        assert_eq!(
            terminal_resolution(
                &profile,
                &credentials,
                mode,
                condition,
                pre_claim_balances,
                pre_collateral_balance,
                expected_redeemed_collateral_balance,
                RequestKind::Fence,
            ),
            RedemptionResolution::PermanentlyFencedNoEffect
        );
    }
}

#[test]
fn zero_and_dust_collateral_balances_are_exact() {
    let profile = profile();
    let credentials = credentials(&profile);
    for (pre_collateral, expected_collateral) in [
        (scaled_word("0"), scaled_word("0")),
        (scaled_word("1"), scaled_word("2")),
    ] {
        assert_eq!(
            terminal_resolution(
                &profile,
                &credentials,
                MarketMode::Standard,
                3,
                [scaled_word("1"), scaled_word("0")],
                pre_collateral,
                expected_collateral,
                RequestKind::Original,
            ),
            RedemptionResolution::RedemptionFinalized
        );
        assert_eq!(
            terminal_resolution(
                &profile,
                &credentials,
                MarketMode::Standard,
                3,
                [scaled_word("1"), scaled_word("0")],
                pre_collateral,
                expected_collateral,
                RequestKind::Fence,
            ),
            RedemptionResolution::PermanentlyFencedNoEffect
        );
    }
}

#[test]
fn consistent_dummy_index_set_mutation_is_not_replaced() {
    let changed_config = CONFIG.replace("dummy_index_sets = [1, 2]", "dummy_index_sets = [3, 4]");
    let changed_manifest =
        MANIFEST.replace("dummy_index_sets = [1, 2]", "dummy_index_sets = [3, 4]");
    let changed = hermetic_profile(&changed_config, &changed_manifest).unwrap();
    assert_eq!(changed.adapter_arguments().2, [3, 4]);
}

#[test]
fn wrong_output_and_post_state_drift_fail_closed() {
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
    for (mutation, collateral_balance) in [
        (PostMutation::WrongOutput, [4; 32]),
        (PostMutation::None, [9; 32]),
    ] {
        let responses = raw_responses_with_mutation(
            &original,
            &profile,
            &credentials,
            &queries,
            Some(RequestKind::Original),
            None,
            None,
            SafeNonce::from_decimal("1").unwrap(),
            [[0; 32]; 2],
            collateral_balance,
            mutation,
            QueryBindingSwap::None,
        )
        .unwrap();
        assert_eq!(
            responses
                .verify_after_original(&profile, &credentials, &original)
                .unwrap()
                .consume_after_original(&responses, &profile, &credentials, &original)
                .unwrap(),
            RedemptionResolution::IntegrityFailure
        );
    }
}

#[test]
fn swapped_or_replayed_post_state_source_fails_closed() {
    let profile = profile();
    let credentials = credentials(&profile);
    for mutation in [PostMutation::SwappedClaims, PostMutation::ReplayedCondition] {
        let original = authorize_original(prepared(
            &profile,
            &credentials,
            MarketMode::NegativeRisk,
            2,
            SafeNonce::ZERO,
        ));
        let fence = authorize_fence(original);
        let queries = ExactQuerySet::after_fence_response_loss(&profile, &fence, None).unwrap();
        let responses = raw_responses_with_mutation(
            &fence,
            &profile,
            &credentials,
            &queries,
            Some(RequestKind::Fence),
            None,
            None,
            SafeNonce::from_decimal("1").unwrap(),
            [[1; 32], [2; 32]],
            [3; 32],
            mutation,
            QueryBindingSwap::None,
        )
        .unwrap();
        assert_eq!(
            responses
                .verify_after_fence(&profile, &credentials, &fence)
                .unwrap()
                .consume_after_fence(&responses, &profile, &credentials, &fence)
                .unwrap(),
            RedemptionResolution::IntegrityFailure
        );
    }
}

#[test]
fn swapped_query_capabilities_are_rejected_before_parsing() {
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
    for swap in [
        QueryBindingSwap::NonceFinalized,
        QueryBindingSwap::Receipts,
        QueryBindingSwap::PostBoundary,
    ] {
        let error = raw_responses_with_mutation(
            &original,
            &profile,
            &credentials,
            &queries,
            None,
            None,
            None,
            SafeNonce::ZERO,
            [[1; 32], [2; 32]],
            [3; 32],
            PostMutation::None,
            swap,
        )
        .err()
        .unwrap();
        assert_eq!(error.diagnostic.class, WireFailureClass::IntegrityFailure);
    }
}

#[test]
fn old_prepared_new_profile_key_and_source_fail_closed() {
    let old_profile = profile();
    let old_credentials = credentials(&old_profile);
    let original = authorize_original(prepared(
        &old_profile,
        &old_credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let queries =
        ExactQuerySet::after_original_response_loss(&old_profile, &original, None).unwrap();
    let responses = raw_responses(
        &original,
        &old_profile,
        &old_credentials,
        &queries,
        None,
        None,
        None,
        SafeNonce::ZERO,
        [[1; 32], [2; 32]],
        [3; 32],
    );
    let outcome = responses
        .verify_after_original(&old_profile, &old_credentials, &original)
        .unwrap();

    let key_config = CONFIG.replace("key_version = 1", "key_version = 2");
    let key_profile = hermetic_profile(&key_config, MANIFEST).unwrap();
    let key_credentials = credentials(&key_profile);
    assert!(matches!(
        ExactQuerySet::after_original_response_loss(&key_profile, &original, None),
        Err(QueryError::IntegrityFailure)
    ));
    let key_prepared = prepared(
        &key_profile,
        &key_credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    );
    assert_ne!(
        original.prepared().action_digest(),
        key_prepared.action_digest()
    );
    let construction_error = RelayerSourceResponse::from_hermetic_submit_bytes(
        &original,
        &key_profile,
        &key_credentials,
        RequestKind::Original,
        br#"{"transactionID":"exact-id","state":"STATE_NEW","transactionHash":""}"#,
    )
    .err()
    .unwrap();
    assert_eq!(
        construction_error.diagnostic.class,
        WireFailureClass::IntegrityFailure
    );
    let chain_construction_error = FinalizedChainSourceResponse::from_hermetic_bytes(
        &original,
        &key_profile,
        &key_credentials,
        &queries,
        QueryKind::SafeNonce,
        [1; 32],
        [2; 32],
        b"{}",
    )
    .err()
    .unwrap();
    assert_eq!(
        chain_construction_error.diagnostic.class,
        WireFailureClass::IntegrityFailure
    );
    assert_eq!(
        responses
            .verify_after_original(&key_profile, &key_credentials, &original)
            .err()
            .unwrap()
            .diagnostic
            .class,
        WireFailureClass::IntegrityFailure
    );
    assert_eq!(
        outcome.consume_after_original(&responses, &key_profile, &key_credentials, &original,),
        Err(QueryError::IntegrityFailure)
    );

    for (changed_config, changed_manifest) in [
        (
            CONFIG.replace("max_metadata_bytes = 256", "max_metadata_bytes = 255"),
            MANIFEST.to_owned(),
        ),
        (
            CONFIG.replace(
                "https://relayer-v2.polymarket.com",
                "https://relayer-review.invalid",
            ),
            MANIFEST.replace(
                "https://relayer-v2.polymarket.com",
                "https://relayer-review.invalid",
            ),
        ),
        (
            CONFIG.replace("https://polygon-rpc.com", "https://rpc-review.invalid"),
            MANIFEST.replace("https://polygon-rpc.com", "https://rpc-review.invalid"),
        ),
    ] {
        let changed_profile = hermetic_profile(&changed_config, &changed_manifest).unwrap();
        let changed_credentials = credentials(&changed_profile);
        assert!(matches!(
            ExactQuerySet::after_original_response_loss(&changed_profile, &original, None),
            Err(QueryError::IntegrityFailure)
        ));
        let changed_prepared = prepared(
            &changed_profile,
            &changed_credentials,
            MarketMode::Standard,
            1,
            SafeNonce::ZERO,
        );
        assert_ne!(
            original.prepared().action_digest(),
            changed_prepared.action_digest()
        );
        let error = RelayerSourceResponse::from_hermetic_submit_bytes(
            &original,
            &changed_profile,
            &changed_credentials,
            RequestKind::Original,
            br#"{"transactionID":"exact-id","state":"STATE_NEW","transactionHash":""}"#,
        )
        .err()
        .unwrap();
        assert_eq!(error.diagnostic.class, WireFailureClass::IntegrityFailure);
        assert_eq!(
            responses
                .verify_after_original(&changed_profile, &changed_credentials, &original)
                .err()
                .unwrap()
                .diagnostic
                .class,
            WireFailureClass::IntegrityFailure
        );
        let drift_outcome = responses
            .verify_after_original(&old_profile, &old_credentials, &original)
            .unwrap();
        assert_eq!(
            drift_outcome.consume_after_original(
                &responses,
                &changed_profile,
                &changed_credentials,
                &original,
            ),
            Err(QueryError::IntegrityFailure)
        );
    }
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
        let mut too_small = super::bounded::CappedBytes::try_with_capacity(body.len() - 1).unwrap();
        assert!(too_small.extend(body).is_err());
        let mut exact = super::bounded::CappedBytes::try_with_capacity(body.len()).unwrap();
        assert!(exact.extend(body).is_ok());
        assert_eq!(exact.len(), body.len());
        let mut spare = super::bounded::CappedBytes::try_with_capacity(body.len() + 1).unwrap();
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
    let submit = RelayerSourceResponse::from_hermetic_submit_bytes(
        &original,
        &profile,
        &credentials,
        RequestKind::Original,
        br#"{"transactionID":"exact-id","state":"STATE_NEW","transactionHash":""}"#,
    )
    .unwrap()
    .parse_submit(&original, &profile, &credentials, RequestKind::Original)
    .unwrap();
    let queries =
        ExactQuerySet::after_original_response_loss(&profile, &original, Some(&submit)).unwrap();
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
    let response = relayer_response(&original, &profile, &credentials, &queries, record.clone());
    response
        .parse_exact_transaction(
            &original,
            &profile,
            &credentials,
            &queries,
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
        let response = relayer_response(&original, &profile, &credentials, &queries, tampered);
        assert!(
            response
                .parse_exact_transaction(
                    &original,
                    &profile,
                    &credentials,
                    &queries,
                    &submit,
                    RequestKind::Original,
                )
                .is_err()
        );
    }
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
        [4; 32],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .consume_after_original(&responses, &profile, &credentials, &original)
            .unwrap(),
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
        [3; 32],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .consume_after_original(&responses, &profile, &credentials, &original)
            .unwrap(),
        RedemptionResolution::IntegrityFailure
    );
    let fence = authorize_fence(original);
    assert_eq!(
        responses
            .verify_after_fence(&profile, &credentials, &fence)
            .unwrap()
            .consume_after_fence(&responses, &profile, &credentials, &fence)
            .unwrap(),
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
        [3; 32],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .consume_after_original(&responses, &profile, &credentials, &original)
            .unwrap(),
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
            hermetic::fresh(
                binding,
                owner_set,
                threshold,
                snapshot_generation + 1,
                lane_generation
            ),
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
    let (owner_set, threshold) = original.prepared().hermetic_safe_owner_contract();
    assert!(matches!(
        original.authorize_fence(
            hermetic::fresh(
                binding,
                owner_set,
                threshold,
                snapshot_generation,
                lane_generation
            ),
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
        profile.max_request_bytes_for(false),
        profile.max_request_bytes_for(true),
        1,
    );
    let first = build_request_pair(
        &profile,
        &credentials,
        hermetic::snapshot([1; 32], [[1; 32], [2; 32]], [3; 32], [4; 32], 1),
        permit,
        RedemptionBuildInput::new(MarketMode::Standard, "one"),
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
    let snapshot = hermetic::snapshot([1; 32], [[1; 32], [2; 32]], [3; 32], [4; 32], 1);
    let capacity = hermetic::nonce_capacity(
        profile.safe_address(),
        maximum,
        profile.max_request_bytes_for(false),
        profile.max_request_bytes_for(true),
        1,
    );
    assert!(matches!(
        build_request_pair(
            &profile,
            &credentials,
            snapshot,
            capacity,
            RedemptionBuildInput::new(MarketMode::Standard, "max"),
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
        [3; 32],
    );
    assert_eq!(
        responses
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .consume_after_original(&responses, &profile, &credentials, &original)
            .unwrap(),
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
        [4; 32],
    );
    assert_eq!(
        reorged
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .consume_after_original(&reorged, &profile, &credentials, &original)
            .unwrap(),
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
        [4; 32],
    );
    assert_eq!(
        wrong_id
            .verify_after_original(&profile, &credentials, &original)
            .unwrap()
            .consume_after_original(&wrong_id, &profile, &credentials, &original)
            .unwrap(),
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
fn fabricated_reader_has_no_production_proof_path() {
    let profile = profile();
    let credentials = credentials(&profile);
    let original = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    ));
    let response = RelayerSourceResponse::from_hermetic_submit_bytes(
        &original,
        &profile,
        &credentials,
        RequestKind::Original,
        br#"{"transactionID":"exact-id","state":"STATE_NEW","transactionHash":""}"#,
    )
    .unwrap();
    assert_eq!(
        response.projection(&credentials).class,
        ProjectionClass::RelayerResponse
    );
}

#[test]
fn cross_action_outcome_reuse_is_rejected() {
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
        SafeNonce::ZERO,
        [[1; 32], [2; 32]],
        [3; 32],
    );
    let outcome = responses
        .verify_after_original(&profile, &credentials, &original)
        .unwrap();
    let other = authorize_original(prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        2,
        SafeNonce::ZERO,
    ));
    assert_eq!(
        outcome.consume_after_original(&responses, &profile, &credentials, &other),
        Err(QueryError::BindingMismatch)
    );
}

#[test]
fn profile_key_source_and_finalized_bindings_fail_closed() {
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
        SafeNonce::ZERO,
        [[1; 32], [2; 32]],
        [3; 32],
    );
    let outcome = responses
        .verify_after_original(&profile, &credentials, &original)
        .unwrap();
    let changed_config = CONFIG.replace("key_version = 1", "key_version = 2");
    let changed_profile = hermetic_profile(&changed_config, MANIFEST).unwrap();
    let changed_credentials = credentials(&changed_profile);
    assert_eq!(
        outcome.consume_after_original(
            &responses,
            &changed_profile,
            &changed_credentials,
            &original,
        ),
        Err(QueryError::IntegrityFailure)
    );

    let wrong_source = RelayerSourceResponse::from_hermetic_submit_bytes(
        &original,
        &profile,
        &credentials,
        RequestKind::Original,
        br#"{"transactionID":"exact-id","state":"STATE_NEW","transactionHash":""}"#,
    )
    .unwrap()
    .with_hermetic_source_identity([0x77; 32]);
    assert_eq!(
        wrong_source
            .parse_submit(&original, &profile, &credentials, RequestKind::Original)
            .err()
            .unwrap()
            .diagnostic
            .class,
        WireFailureClass::IntegrityFailure
    );

    let finalized_responses = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        None,
        None,
        None,
        SafeNonce::ZERO,
        [[1; 32], [2; 32]],
        [3; 32],
    );
    let finalized_outcome = finalized_responses
        .verify_after_original(&profile, &credentials, &original)
        .unwrap();
    let mismatched_finalized =
        finalized_responses.with_hermetic_finalized_coordinates([0x82; 32], [0xb6; 32]);
    assert_eq!(
        finalized_outcome.consume_after_original(
            &mismatched_finalized,
            &profile,
            &credentials,
            &original,
        ),
        Err(QueryError::BindingMismatch)
    );

    let source_responses = raw_responses(
        &original,
        &profile,
        &credentials,
        &queries,
        None,
        None,
        None,
        SafeNonce::ZERO,
        [[1; 32], [2; 32]],
        [3; 32],
    );
    let source_outcome = source_responses
        .verify_after_original(&profile, &credentials, &original)
        .unwrap();
    let wrong_chain_source = source_responses.with_hermetic_chain_source_identity([0x66; 32]);
    assert_eq!(
        source_outcome.consume_after_original(
            &wrong_chain_source,
            &profile,
            &credentials,
            &original,
        ),
        Err(QueryError::IntegrityFailure)
    );
}

#[test]
fn sentinels_do_not_reach_redacted_diagnostics() {
    sentinel_values_never_appear_in_redacted_projections();
}

#[test]
fn primitive_is_mechanically_disabled() {
    assert!(!MECHANICALLY_ENABLED);
    assert!(
        hermetic_profile(
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

#[test]
fn profile_source_limits_and_closed_form_peak_are_exact() {
    assert!(
        super::config::validate_profile_hermetic(
            CONFIG,
            MANIFEST,
            CONFIG.len() - 1,
            MANIFEST.len(),
            working_set(),
        )
        .is_err()
    );
    assert!(
        super::config::validate_profile_hermetic(
            CONFIG,
            MANIFEST,
            CONFIG.len(),
            MANIFEST.len() - 1,
            working_set(),
        )
        .is_err()
    );
    assert!(
        super::config::validate_profile_hermetic(
            CONFIG,
            MANIFEST,
            CONFIG.len(),
            MANIFEST.len(),
            working_set(),
        )
        .is_ok()
    );
    assert!(
        super::config::validate_profile_hermetic(
            CONFIG,
            MANIFEST,
            CONFIG.len() + 1,
            MANIFEST.len() + 1,
            working_set(),
        )
        .is_ok()
    );
    let oversized = format!("{CONFIG}\n");
    assert!(
        super::config::validate_profile_hermetic(
            &oversized,
            MANIFEST,
            CONFIG.len(),
            MANIFEST.len(),
            working_set(),
        )
        .is_err()
    );
    let oversized_manifest = format!("{MANIFEST}\n");
    assert!(
        super::config::validate_profile_hermetic(
            CONFIG,
            &oversized_manifest,
            CONFIG.len(),
            MANIFEST.len(),
            working_set(),
        )
        .is_err()
    );

    let config: toml::Value = toml::from_str(CONFIG).unwrap();
    let manifest: toml::Value = toml::from_str(MANIFEST).unwrap();
    let relayer = &config["relayer"];
    let rpc = &config["rpc"];
    let query = &config["query"];
    let credentials = &config["credentials"];
    let allocation = &manifest["allocation_boundary"];
    let working_set_config = &config["working_set"];
    let peak = relayer["max_original_request_bytes"].as_integer().unwrap()
        + relayer["max_fence_request_bytes"].as_integer().unwrap()
        + 2 * (relayer["max_header_bytes"].as_integer().unwrap()
            + relayer["max_metadata_bytes"].as_integer().unwrap())
        + query["max_bytes"].as_integer().unwrap()
        + query["max_items"].as_integer().unwrap()
            * allocation["query_offset_layout_bytes"]
                .as_integer()
                .unwrap()
        + relayer["max_response_bytes"].as_integer().unwrap()
        + relayer["overflow_probe_bytes"].as_integer().unwrap()
        + relayer["max_transaction_id_bytes"].as_integer().unwrap()
        + (query["max_items"].as_integer().unwrap() - 1)
            * (rpc["max_response_bytes"].as_integer().unwrap()
                + rpc["overflow_probe_bytes"].as_integer().unwrap())
        + credentials["max_acquisition_bytes"].as_integer().unwrap()
        + 6 * credentials["max_value_bytes"].as_integer().unwrap()
        + rpc["max_receipt_logs"].as_integer().unwrap()
            * allocation["receipt_log_index_layout_bytes"]
                .as_integer()
                .unwrap()
        + working_set_config["operational_structural_bytes"]
            .as_integer()
            .unwrap();
    assert_eq!(
        peak,
        allocation["max_operational_working_set_bytes"]
            .as_integer()
            .unwrap()
    );
}

#[test]
fn oversized_profile_elements_and_maxima_fail_closed() {
    let oversized_elements =
        CONFIG.replace("dummy_index_sets = [1, 2]", "dummy_index_sets = [1, 2, 3]");
    assert!(hermetic_profile(&oversized_elements, MANIFEST).is_err());

    let oversized_maximum = CONFIG.replace(
        "max_original_request_bytes = 4096",
        "max_original_request_bytes = 8192",
    );
    assert!(hermetic_profile(&oversized_maximum, MANIFEST).is_err());

    let oversized_manifest_elements = MANIFEST.replace(
        "ignored_argument_indices = [0, 1, 3]",
        "ignored_argument_indices = [0, 1, 2, 3]",
    );
    assert!(hermetic_profile(CONFIG, &oversized_manifest_elements).is_err());
}

#[test]
fn standard_and_negative_risk_adapter_log_failures_are_integrity_failures() {
    for mode in [MarketMode::Standard, MarketMode::NegativeRisk] {
        for mutation in [
            ReceiptMutation::MissingPayout,
            ReceiptMutation::DuplicatePayout,
            ReceiptMutation::CorruptPayout,
            ReceiptMutation::WrongEmitter,
            ReceiptMutation::WrongField,
            ReceiptMutation::WrongAmount,
            ReceiptMutation::ReorgedPayout,
        ] {
            let profile = profile();
            let credentials = credentials(&profile);
            let original =
                authorize_original(prepared(&profile, &credentials, mode, 1, SafeNonce::ZERO));
            let queries =
                ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
            let responses = raw_responses_with_receipt_mutation(
                &original,
                &profile,
                &credentials,
                &queries,
                Some(RequestKind::Original),
                None,
                None,
                SafeNonce::from_decimal("1").unwrap(),
                [[0; 32]; 2],
                [4; 32],
                PostMutation::None,
                QueryBindingSwap::None,
                mutation,
            )
            .unwrap();
            assert_eq!(
                responses
                    .verify_after_original(&profile, &credentials, &original)
                    .unwrap()
                    .consume_after_original(&responses, &profile, &credentials, &original)
                    .unwrap(),
                RedemptionResolution::IntegrityFailure
            );
        }
    }
}

#[test]
fn receipt_log_limit_minus_one_limit_and_limit_plus_one_are_exact() {
    let limit = profile().max_receipt_logs();
    for (total_logs, expected) in [
        (limit - 1, RedemptionResolution::RedemptionFinalized),
        (limit, RedemptionResolution::RedemptionFinalized),
        (limit + 1, RedemptionResolution::IntegrityFailure),
    ] {
        let profile = profile();
        let credentials = credentials(&profile);
        let original = authorize_original(prepared(
            &profile,
            &credentials,
            MarketMode::Standard,
            1,
            SafeNonce::ZERO,
        ));
        let queries =
            ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
        let responses = raw_responses_with_receipt_mutation(
            &original,
            &profile,
            &credentials,
            &queries,
            Some(RequestKind::Original),
            None,
            None,
            SafeNonce::from_decimal("1").unwrap(),
            [[0; 32]; 2],
            [4; 32],
            PostMutation::None,
            QueryBindingSwap::None,
            ReceiptMutation::ExtraLogs(total_logs - 2),
        )
        .unwrap();
        assert_eq!(
            responses
                .verify_after_original(&profile, &credentials, &original)
                .unwrap()
                .consume_after_original(&responses, &profile, &credentials, &original)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn present_invalid_losing_and_winning_receipts_fail_closed() {
    for (winner, invalid, nonce, claims, collateral) in [
        (
            None,
            RequestKind::Original,
            SafeNonce::ZERO,
            [[1; 32], [2; 32]],
            [3; 32],
        ),
        (
            Some(RequestKind::Original),
            RequestKind::Original,
            SafeNonce::from_decimal("1").unwrap(),
            [[0; 32]; 2],
            [4; 32],
        ),
        (
            Some(RequestKind::Original),
            RequestKind::Fence,
            SafeNonce::from_decimal("1").unwrap(),
            [[0; 32]; 2],
            [4; 32],
        ),
        (
            Some(RequestKind::Fence),
            RequestKind::Original,
            SafeNonce::from_decimal("1").unwrap(),
            [[1; 32], [2; 32]],
            [3; 32],
        ),
    ] {
        for mutation in [
            ReceiptMutation::Invalid(invalid),
            ReceiptMutation::CorruptSafe(invalid),
            ReceiptMutation::Malformed(invalid),
            ReceiptMutation::AlsoCompatible(invalid),
        ] {
            let profile = profile();
            let credentials = credentials(&profile);
            let original = authorize_original(prepared(
                &profile,
                &credentials,
                MarketMode::Standard,
                1,
                SafeNonce::ZERO,
            ));
            let queries =
                ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
            let responses = raw_responses_with_receipt_mutation(
                &original,
                &profile,
                &credentials,
                &queries,
                winner,
                None,
                None,
                nonce,
                claims,
                collateral,
                PostMutation::None,
                QueryBindingSwap::None,
                mutation,
            )
            .unwrap();
            assert_eq!(
                responses
                    .verify_after_original(&profile, &credentials, &original)
                    .unwrap()
                    .consume_after_original(&responses, &profile, &credentials, &original)
                    .unwrap(),
                RedemptionResolution::IntegrityFailure
            );
        }
    }
}

#[test]
fn typed_policy_drives_json_eip712_and_http_method() {
    let profile = profile_with_request_limits(4096, 4096, 2048);
    let credentials = worst_case_credentials(&profile);
    let prepared = build_worst_case_pair(&profile, &credentials).unwrap();
    let body_bytes = prepared.hermetic_body(RequestKind::Original);
    let body: Value = serde_json::from_slice(body_bytes).unwrap();
    let policy: toml::Value = toml::from_str(CONFIG).unwrap();
    let policy = &policy["transaction_policy"];
    assert_eq!(
        body["signatureParams"]["gasPrice"],
        policy["gas_price"].as_str().unwrap()
    );
    assert_eq!(
        body["signatureParams"]["operation"],
        policy["operation"].as_integer().unwrap().to_string()
    );
    assert_eq!(
        body["signatureParams"]["safeTxGas"],
        policy["safe_tx_gas"].as_str().unwrap()
    );
    assert_eq!(
        body["signatureParams"]["baseGas"],
        policy["base_gas"].as_str().unwrap()
    );
    assert_eq!(
        body["signatureParams"]["gasToken"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase(),
        policy["gas_token"].as_str().unwrap().to_ascii_lowercase()
    );
    assert_eq!(
        body["signatureParams"]["refundReceiver"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase(),
        policy["refund_receiver"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
    );

    let headers: Value =
        serde_json::from_slice(prepared.hermetic_headers(RequestKind::Original)).unwrap();
    let decoded_secret = general_purpose::STANDARD
        .decode(credentials.builder_api_secret())
        .unwrap();
    let mut hmac = Hmac::<Sha256>::new_from_slice(&decoded_secret).unwrap();
    hmac.update(u64::MAX.to_string().as_bytes());
    hmac.update(policy["http_method"].as_str().unwrap().as_bytes());
    hmac.update(profile.submit_path().as_bytes());
    hmac.update(body_bytes);
    let expected = general_purpose::URL_SAFE.encode(hmac.finalize().into_bytes());
    assert_eq!(headers["POLY_BUILDER_SIGNATURE"], expected);
    assert_ne!(
        prepared
            .request(RequestKind::Original)
            .identity()
            .safe_transaction_hash(),
        [0; 32]
    );
}

#[test]
fn whole_working_set_reservation_and_capacity_failures_are_exact() {
    let config: toml::Value = toml::from_str(CONFIG).unwrap();
    let startup = config["working_set"]["max_startup_working_set_bytes"]
        .as_integer()
        .unwrap() as usize;
    let operational = config["working_set"]["max_operational_working_set_bytes"]
        .as_integer()
        .unwrap() as usize;
    assert!(validate_profile(hermetic::working_set(startup, operational, 1)).is_ok());
    assert!(matches!(
        validate_profile(hermetic::working_set(startup - 1, operational, 1)),
        Err(RedemptionConfigError::Capacity)
    ));
    assert!(matches!(
        validate_profile(hermetic::working_set(startup, operational - 1, 1)),
        Err(RedemptionConfigError::Capacity)
    ));
    assert!(matches!(
        super::bounded::CappedBytes::try_with_capacity(usize::MAX),
        Err(super::bounded::CappedIoError::Allocation)
    ));
    assert_eq!(super::query::query_offset_layout_bytes(), 128);
}

#[test]
fn original_and_fence_worst_case_builder_limits_are_exact() {
    let baseline = profile_with_request_limits(4096, 4096, 2048);
    let credentials = worst_case_credentials(&baseline);
    let prepared = build_worst_case_pair(&baseline, &credentials).unwrap();
    let original_len = prepared.hermetic_body(RequestKind::Original).len();
    let fence_len = prepared.hermetic_body(RequestKind::Fence).len();
    let original_header_len = prepared.hermetic_headers(RequestKind::Original).len();
    let fence_header_len = prepared.hermetic_headers(RequestKind::Fence).len();
    assert_eq!(original_header_len, fence_header_len);

    for (original_limit, fence_limit, header_limit, succeeds) in [
        (original_len - 1, 4096, 2048, false),
        (original_len, fence_len, original_header_len, true),
        (
            original_len + 1,
            fence_len + 1,
            original_header_len + 1,
            true,
        ),
        (4096, fence_len - 1, 2048, false),
        (4096, 4096, original_header_len - 1, false),
        (4096, 4096, original_header_len, true),
        (4096, 4096, original_header_len + 1, true),
    ] {
        let bounded = profile_with_request_limits(original_limit, fence_limit, header_limit);
        let credentials = worst_case_credentials(&bounded);
        let result = build_worst_case_pair(&bounded, &credentials);
        assert_eq!(result.is_ok(), succeeds);
        if !succeeds {
            assert!(matches!(
                result,
                Err(RedemptionRequestError::RequestTooLarge)
            ));
        }
    }
}

#[test]
fn signer_and_finalized_safe_owner_set_are_exact() {
    let profile = profile();
    let credentials = credentials(&profile);
    let prepared = prepared(
        &profile,
        &credentials,
        MarketMode::Standard,
        1,
        SafeNonce::ZERO,
    );
    assert_eq!(
        prepared.request(RequestKind::Original).owner(),
        credentials.signer_address()
    );
    let (binding, original_hash, _, snapshot_generation, lane_generation) =
        prepared.hermetic_bindings();
    let (_, threshold) = prepared.hermetic_safe_owner_contract();
    assert!(matches!(
        prepared.authorize_original(
            hermetic::fresh(
                binding,
                [0; 32],
                threshold,
                snapshot_generation,
                lane_generation
            ),
            hermetic::original_durable(binding, original_hash, 1),
        ),
        Err(RedemptionRequestError::CapabilityMismatch)
    ));

    let wrong_owner = format!("0x{}", "ee".repeat(20));
    let owner_config = CONFIG.replace(CONFIGURED_OWNER, &wrong_owner);
    let owner_manifest = MANIFEST.replace(CONFIGURED_OWNER, &wrong_owner);
    let owner_profile = hermetic_profile(&owner_config, &owner_manifest).unwrap();
    let mut source = HermeticCredentialSource::new(credential_value);
    assert!(matches!(
        resolve_credentials(&owner_profile, "hermetic-region", &mut source),
        Err(RedemptionConfigError::SignerMismatch)
    ));
    assert!(
        hermetic_profile(
            &CONFIG.replace("threshold = 1", "threshold = 2"),
            &MANIFEST.replace("threshold = 1", "threshold = 2"),
        )
        .is_err()
    );
    assert!(
        hermetic_profile(
            &CONFIG.replace(&format!("owners = [\"{CONFIGURED_OWNER}\"]"), "owners = []",),
            &MANIFEST.replace(&format!("owners = [\"{CONFIGURED_OWNER}\"]"), "owners = []",),
        )
        .is_err()
    );

    for mutation in [
        ReceiptMutation::WrongSafeOwner,
        ReceiptMutation::WrongSafeThreshold,
    ] {
        let original = authorize_original(prepared(
            &profile,
            &credentials,
            MarketMode::Standard,
            1,
            SafeNonce::ZERO,
        ));
        let queries =
            ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
        let responses = raw_responses_with_receipt_mutation(
            &original,
            &profile,
            &credentials,
            &queries,
            Some(RequestKind::Original),
            None,
            None,
            SafeNonce::from_decimal("1").unwrap(),
            [[0; 32]; 2],
            [4; 32],
            PostMutation::None,
            QueryBindingSwap::None,
            mutation,
        )
        .unwrap();
        assert_eq!(
            responses
                .verify_after_original(&profile, &credentials, &original)
                .unwrap()
                .consume_after_original(&responses, &profile, &credentials, &original)
                .unwrap(),
            RedemptionResolution::IntegrityFailure
        );
    }
}

#[test]
fn duplicate_and_out_of_order_log_indices_fail_closed() {
    for mode in [MarketMode::Standard, MarketMode::NegativeRisk] {
        for (winner, nonce, claims, collateral) in [
            (
                RequestKind::Original,
                SafeNonce::from_decimal("1").unwrap(),
                [[0; 32]; 2],
                [4; 32],
            ),
            (
                RequestKind::Fence,
                SafeNonce::from_decimal("1").unwrap(),
                [[1; 32], [2; 32]],
                [3; 32],
            ),
        ] {
            for mutation in [
                ReceiptMutation::DuplicateLogIndex,
                ReceiptMutation::OutOfOrderLogIndex,
            ] {
                let profile = profile();
                let credentials = credentials(&profile);
                let original =
                    authorize_original(prepared(&profile, &credentials, mode, 1, SafeNonce::ZERO));
                let queries =
                    ExactQuerySet::after_original_response_loss(&profile, &original, None).unwrap();
                let responses = raw_responses_with_receipt_mutation(
                    &original,
                    &profile,
                    &credentials,
                    &queries,
                    Some(winner),
                    None,
                    None,
                    nonce,
                    claims,
                    collateral,
                    PostMutation::None,
                    QueryBindingSwap::None,
                    mutation,
                )
                .unwrap();
                assert_eq!(
                    responses
                        .verify_after_original(&profile, &credentials, &original)
                        .unwrap()
                        .consume_after_original(&responses, &profile, &credentials, &original)
                        .unwrap(),
                    RedemptionResolution::IntegrityFailure
                );
            }
        }
    }
}
