use nautilus_polymarket::{
    common::{credential::Secrets as PolymarketSecrets, enums::SignatureType},
    http::{
        clob::PolymarketClobHttpClient,
        query::{AssetType, GetBalanceAllowanceParams},
    },
};
use rust_decimal::Decimal;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_operator_artifacts::{BoltV3OperatorArtifactError, json_artifact_sha256},
    bolt_v3_providers::{
        ClobV2CollateralAccountingSourceMaterialization,
        ClobV2CollateralAccountingSourceMaterializationRequest, ExternalSnapshotConfirmationPolicy,
        fetch_external_snapshot_with_retries,
    },
};

const CLOB_V2_COLLATERAL_BALANCE_ALLOWANCE_PATH: &str = "/balance-allowance";
const CLOB_V2_COLLATERAL_ASSET_TYPE: &str = "COLLATERAL";
const CLOB_V2_COLLATERAL_BALANCE_UNIT: &str = "micro_pusd";
const CLOB_V2_COLLATERAL_PUSD_MICRO_SCALE: u32 = 1_000_000;

pub async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_with_policy(
        request, true,
    )
    .await
}

pub(crate) async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_once(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_with_policy(
        request, false,
    )
    .await
}

async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_with_policy(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
    retry_initial_fetch: bool,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    let strategy = request
        .loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == request.strategy_instance_id)
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "strategy_instance_id",
        })?;
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let client = request.loaded.root.clients.get(execution_client_id).ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "execution_client_id",
        },
    )?;
    if client.venue.as_str() != super::KEY {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "execution_client_provider",
        });
    }
    let execution = client
        .execution
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "execution" })?;
    let cfg: super::PolymarketExecutionConfig = execution.clone().try_into().map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "execution" }
    })?;
    let secrets = super::secrets_for(execution_client_id, request.resolved).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "resolved_secrets",
        }
    })?;
    let polymarket_secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.clone()),
        Some(secrets.api_secret.clone()),
        Some(secrets.passphrase.clone()),
        cfg.funder.clone(),
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "polymarket_credentials",
    })?;
    let signature_type = super::nt_signature_type(cfg.signature_type);
    let http_client = PolymarketClobHttpClient::new(
        polymarket_secrets.credential,
        polymarket_secrets.address,
        Some(cfg.base_url_http.clone()),
        cfg.http_timeout_secs,
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "execution.base_url_http",
    })?;
    let confirmation_policy = ExternalSnapshotConfirmationPolicy::from_retry_fields(
        cfg.max_retries,
        cfg.retry_delay_initial_ms,
        cfg.retry_delay_max_ms,
    );
    let balance_allowance = if retry_initial_fetch {
        fetch_external_snapshot_with_retries(confirmation_policy, || {
            http_client.get_balance_allowance(balance_allowance_params(signature_type))
        })
        .await
    } else {
        http_client
            .get_balance_allowance(balance_allowance_params(signature_type))
            .await
    }
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "balance_allowance_response",
    })?;
    let micro_scale = Decimal::from(CLOB_V2_COLLATERAL_PUSD_MICRO_SCALE);
    let p_usd_balance = balance_allowance.balance / micro_scale;
    let p_usd_allowance = balance_allowance.allowance.ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "p_usd_allowance",
        },
    )? / micro_scale;
    let p_usd_balance = p_usd_balance.to_string();
    let p_usd_allowance = p_usd_allowance.to_string();
    let base_url_http_sha256 = sha256_text(&cfg.base_url_http);
    let proof = ClobV2CollateralAccountingBalanceAllowanceProof {
        schema_version: request.schema_version,
        record_kind: request.balance_allowance_record_kind,
        execution_client_id,
        base_url_http_sha256: &base_url_http_sha256,
        request_path: CLOB_V2_COLLATERAL_BALANCE_ALLOWANCE_PATH,
        asset_type: CLOB_V2_COLLATERAL_ASSET_TYPE,
        signature_type: signature_type_label(cfg.signature_type),
        balance_unit: CLOB_V2_COLLATERAL_BALANCE_UNIT,
        p_usd_balance: &p_usd_balance,
        p_usd_allowance: &p_usd_allowance,
    };
    let collateral_accounting_source_sha256 = json_artifact_sha256(&proof)?;

    Ok(ClobV2CollateralAccountingSourceMaterialization {
        p_usd_balance,
        p_usd_allowance,
        collateral_accounting_source_sha256,
        confirmation_policy,
    })
}

fn balance_allowance_params(signature_type: SignatureType) -> GetBalanceAllowanceParams {
    GetBalanceAllowanceParams {
        asset_type: Some(AssetType::Collateral),
        token_id: None,
        signature_type: Some(signature_type),
    }
}

fn signature_type_label(value: super::PolymarketSignatureType) -> &'static str {
    match value {
        super::PolymarketSignatureType::Eoa => "eoa",
        super::PolymarketSignatureType::PolyProxy => "poly_proxy",
        super::PolymarketSignatureType::PolyGnosisSafe => "poly_gnosis_safe",
    }
}

fn sha256_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[derive(Serialize)]
struct ClobV2CollateralAccountingBalanceAllowanceProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    execution_client_id: &'a str,
    base_url_http_sha256: &'a str,
    request_path: &'static str,
    asset_type: &'static str,
    signature_type: &'static str,
    balance_unit: &'static str,
    p_usd_balance: &'a str,
    p_usd_allowance: &'a str,
}
