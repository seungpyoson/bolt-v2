use alloy_primitives::Signature;
use nautilus_polymarket::{
    common::{
        credential::EvmPrivateKey,
        enums::{PolymarketOrderSide, SignatureType},
    },
    http::models::PolymarketOrder,
    signing::eip712::{OrderSigner, order_hash},
};
use rust_decimal::Decimal;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{
    bolt_v3_operator_artifacts::{BoltV3OperatorArtifactError, json_artifact_sha256},
    bolt_v3_providers::{
        ClobV2AdapterSigningSourceMaterialization, ClobV2AdapterSigningSourceMaterializationRequest,
    },
};

const CLOB_V2_ADAPTER_SIGNING_EMPTY_BYTES32: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000000";
const CLOB_V2_ADAPTER_SIGNING_NO_EXPIRATION: &str = "0";
const CLOB_V2_ADAPTER_SIGNING_KEYGEN_MAX_ATTEMPTS: usize = 16;
const CLOB_V2_ADAPTER_SIGNING_SEED_BYTES: usize = 32;
const CLOB_V2_ADAPTER_SIGNING_U64_BYTES: usize = std::mem::size_of::<u64>();
const CLOB_V2_ADAPTER_SIGNING_U128_BYTES: usize = std::mem::size_of::<u128>();
const CLOB_V2_ADAPTER_SIGNING_SALT_OFFSET: usize = 0;
const CLOB_V2_ADAPTER_SIGNING_MAKER_AMOUNT_OFFSET: usize = CLOB_V2_ADAPTER_SIGNING_U64_BYTES;
const CLOB_V2_ADAPTER_SIGNING_TAKER_AMOUNT_OFFSET: usize = CLOB_V2_ADAPTER_SIGNING_U64_BYTES * 2;
const CLOB_V2_ADAPTER_SIGNING_TIMESTAMP_OFFSET: usize = CLOB_V2_ADAPTER_SIGNING_U64_BYTES * 3;
const CLOB_V2_ADAPTER_SIGNING_AMOUNT_MODULUS: u64 = 1_000_000;
const CLOB_V2_ADAPTER_SIGNING_MIN_POSITIVE: u64 = 1;

pub fn materialize_clob_v2_adapter_signing_source_from_nt_signing_source(
    request: ClobV2AdapterSigningSourceMaterializationRequest<'_>,
) -> Result<ClobV2AdapterSigningSourceMaterialization, BoltV3OperatorArtifactError> {
    let domain_requirements_sha256 = clob_v2_adapter_signing_domain_requirements_sha256(request)?;
    let signer = generate_ephemeral_clob_v2_adapter_signer()?;
    let signer_address = format!("{:#x}", signer.address());
    let seed = clob_v2_adapter_signing_probe_seed(request);
    let mut order = clob_v2_adapter_signing_probe_order(&seed, &signer_address);
    let signature = signer.sign_order(&order, false).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "signature" }
    })?;
    let signing_hash = order_hash(&order, false).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "order_hash",
        }
    })?;
    let parsed_signature = signature.parse::<Signature>().map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "signature" }
    })?;
    let recovered_address = parsed_signature
        .recover_address_from_prehash(&signing_hash)
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(signer_recovered_matches_expected),
        })?;
    let signer_recovered_matches_expected = recovered_address == signer.address();
    if !signer_recovered_matches_expected {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: stringify!(signer_recovered_matches_expected),
        });
    }

    order.signature = signature.clone();
    let signed_order_fixture = ClobV2AdapterSigningSignedOrderFixture {
        schema_version: request.schema_version,
        record_kind: request.signed_order_fixture_record_kind,
        clob_signing_version: request.clob_signing_version,
        neg_risk: false,
        order: &order,
    };
    let signed_order_fixture_sha256 = json_artifact_sha256(&signed_order_fixture)?;
    let order_hash_hex = format!("{signing_hash:#x}");
    let signer_address_sha256 = hex::encode(Sha256::digest(signer_address.as_bytes()));
    let recovered_address_sha256 =
        hex::encode(Sha256::digest(format!("{recovered_address:#x}").as_bytes()));
    let signature_sha256 = hex::encode(Sha256::digest(signature.as_bytes()));
    let signature_verification = ClobV2AdapterSigningSignatureVerification {
        schema_version: request.schema_version,
        record_kind: request.signature_verification_record_kind,
        clob_signing_version: request.clob_signing_version,
        order_hash: &order_hash_hex,
        signer_address_sha256: &signer_address_sha256,
        recovered_address_sha256: &recovered_address_sha256,
        signature_sha256: &signature_sha256,
        signer_recovered_matches_expected,
    };
    let signature_verification_sha256 = json_artifact_sha256(&signature_verification)?;

    Ok(ClobV2AdapterSigningSourceMaterialization {
        domain_requirements_sha256,
        signed_order_fixture_sha256,
        signature_verification_sha256,
        signer_recovered_matches_expected,
    })
}

fn clob_v2_adapter_signing_domain_requirements_sha256(
    request: ClobV2AdapterSigningSourceMaterializationRequest<'_>,
) -> Result<String, BoltV3OperatorArtifactError> {
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "DOMAIN_VERSION",
        "domain_version",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "DOMAIN_NAME",
        "domain_name",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "POLYGON_CHAIN_ID",
        "polygon_chain_id",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "CTF_EXCHANGE",
        "ctf_exchange",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "NEG_RISK_CTF_EXCHANGE",
        "neg_risk_ctf_exchange",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "OrderSigner",
        "order_signer",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "sign_order",
        "sign_order",
    )?;
    require_clob_v2_adapter_signing_source_marker(
        request.clob_signing_source,
        "order_hash",
        "order_hash",
    )?;

    let proof = ClobV2AdapterSigningDomainRequirementsProof {
        schema_version: request.schema_version,
        record_kind: request.domain_requirements_record_kind,
        clob_signing_version: request.clob_signing_version,
        clob_signing_source_sha256: request.clob_signing_source_sha256,
        domain_version_declared: true,
        domain_name_declared: true,
        polygon_chain_id_declared: true,
        ctf_exchange_declared: true,
        neg_risk_ctf_exchange_declared: true,
        order_signer_declared: true,
        sign_order_declared: true,
        order_hash_declared: true,
    };
    json_artifact_sha256(&proof)
}

fn require_clob_v2_adapter_signing_source_marker(
    source: &str,
    marker: &'static str,
    field: &'static str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.contains(marker) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field })
    }
}

fn generate_ephemeral_clob_v2_adapter_signer() -> Result<OrderSigner, BoltV3OperatorArtifactError> {
    let mut key_bytes = [u8::MIN; CLOB_V2_ADAPTER_SIGNING_SEED_BYTES];
    for _ in std::iter::repeat_n((), CLOB_V2_ADAPTER_SIGNING_KEYGEN_MAX_ATTEMPTS) {
        getrandom::fill(&mut key_bytes).map_err(|_| {
            BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "ephemeral_private_key",
            }
        })?;
        if key_bytes.iter().all(|byte| *byte == 0) {
            continue;
        }
        let mut key_hex = hex::encode(key_bytes);
        let private_key = EvmPrivateKey::new(&key_hex);
        key_hex.zeroize();
        let Ok(private_key) = private_key else {
            continue;
        };
        match OrderSigner::new(&private_key) {
            Ok(signer) => {
                key_bytes.zeroize();
                return Ok(signer);
            }
            Err(_) => continue,
        }
    }
    key_bytes.zeroize();
    Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "ephemeral_private_key",
    })
}

fn clob_v2_adapter_signing_probe_seed(
    request: ClobV2AdapterSigningSourceMaterializationRequest<'_>,
) -> [u8; CLOB_V2_ADAPTER_SIGNING_SEED_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(request.clob_signing_version.as_bytes());
    hasher.update(request.clob_signing_source_sha256.as_bytes());
    hasher.finalize().into()
}

fn clob_v2_adapter_signing_probe_order(
    seed: &[u8; CLOB_V2_ADAPTER_SIGNING_SEED_BYTES],
    signer_address: &str,
) -> PolymarketOrder {
    let salt = clob_v2_adapter_signing_seed_u64(seed, CLOB_V2_ADAPTER_SIGNING_SALT_OFFSET)
        .max(CLOB_V2_ADAPTER_SIGNING_MIN_POSITIVE);
    let maker_amount = Decimal::from(
        (clob_v2_adapter_signing_seed_u64(seed, CLOB_V2_ADAPTER_SIGNING_MAKER_AMOUNT_OFFSET)
            % CLOB_V2_ADAPTER_SIGNING_AMOUNT_MODULUS)
            + CLOB_V2_ADAPTER_SIGNING_MIN_POSITIVE,
    );
    let taker_amount = Decimal::from(
        (clob_v2_adapter_signing_seed_u64(seed, CLOB_V2_ADAPTER_SIGNING_TAKER_AMOUNT_OFFSET)
            % CLOB_V2_ADAPTER_SIGNING_AMOUNT_MODULUS)
            + CLOB_V2_ADAPTER_SIGNING_MIN_POSITIVE,
    );
    let timestamp =
        clob_v2_adapter_signing_seed_u64(seed, CLOB_V2_ADAPTER_SIGNING_TIMESTAMP_OFFSET)
            .max(CLOB_V2_ADAPTER_SIGNING_MIN_POSITIVE);
    PolymarketOrder {
        salt,
        maker: signer_address.to_string(),
        signer: signer_address.to_string(),
        token_id: ustr::Ustr::from(clob_v2_adapter_signing_token_id(seed).as_str()),
        maker_amount,
        taker_amount,
        side: PolymarketOrderSide::Buy,
        signature_type: SignatureType::Eoa,
        expiration: CLOB_V2_ADAPTER_SIGNING_NO_EXPIRATION.to_string(),
        timestamp: timestamp.to_string(),
        metadata: CLOB_V2_ADAPTER_SIGNING_EMPTY_BYTES32.to_string(),
        builder: CLOB_V2_ADAPTER_SIGNING_EMPTY_BYTES32.to_string(),
        signature: String::new(),
    }
}

fn clob_v2_adapter_signing_token_id(seed: &[u8; CLOB_V2_ADAPTER_SIGNING_SEED_BYTES]) -> String {
    let mut bytes = [u8::MIN; CLOB_V2_ADAPTER_SIGNING_U128_BYTES];
    bytes.copy_from_slice(
        &seed[CLOB_V2_ADAPTER_SIGNING_SALT_OFFSET..CLOB_V2_ADAPTER_SIGNING_U128_BYTES],
    );
    u128::from_be_bytes(bytes)
        .max(u128::from(CLOB_V2_ADAPTER_SIGNING_MIN_POSITIVE))
        .to_string()
}

fn clob_v2_adapter_signing_seed_u64(
    seed: &[u8; CLOB_V2_ADAPTER_SIGNING_SEED_BYTES],
    start: usize,
) -> u64 {
    let mut bytes = [u8::MIN; CLOB_V2_ADAPTER_SIGNING_U64_BYTES];
    let end = start + CLOB_V2_ADAPTER_SIGNING_U64_BYTES;
    bytes.copy_from_slice(&seed[start..end]);
    u64::from_be_bytes(bytes)
}

#[derive(Serialize)]
struct ClobV2AdapterSigningDomainRequirementsProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    clob_signing_version: &'a str,
    clob_signing_source_sha256: &'a str,
    domain_version_declared: bool,
    domain_name_declared: bool,
    polygon_chain_id_declared: bool,
    ctf_exchange_declared: bool,
    neg_risk_ctf_exchange_declared: bool,
    order_signer_declared: bool,
    sign_order_declared: bool,
    order_hash_declared: bool,
}

#[derive(Serialize)]
struct ClobV2AdapterSigningSignedOrderFixture<'a> {
    schema_version: u32,
    record_kind: &'static str,
    clob_signing_version: &'a str,
    neg_risk: bool,
    order: &'a PolymarketOrder,
}

#[derive(Serialize)]
struct ClobV2AdapterSigningSignatureVerification<'a> {
    schema_version: u32,
    record_kind: &'static str,
    clob_signing_version: &'a str,
    order_hash: &'a str,
    signer_address_sha256: &'a str,
    recovered_address_sha256: &'a str,
    signature_sha256: &'a str,
    signer_recovered_matches_expected: bool,
}
