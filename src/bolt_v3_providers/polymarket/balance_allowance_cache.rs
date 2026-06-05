use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_network::http::{HttpClient, Method, USER_AGENT};
use nautilus_polymarket::{
    common::{
        credential::{Credential, Secrets as PolymarketSecrets},
        enums::SignatureType,
    },
    http::query::{AssetType, GetBalanceAllowanceParams},
};

use crate::{
    bolt_v3_operator_artifacts::BoltV3OperatorArtifactError,
    bolt_v3_providers::{ClobV2BalanceAllowanceCacheSync, ClobV2BalanceAllowanceCacheSyncRequest},
};

const CLOB_V2_BALANCE_ALLOWANCE_UPDATE_PATH: &str = "/balance-allowance/update";

pub async fn sync_clob_v2_balance_allowance_cache_from_configured_account(
    request: ClobV2BalanceAllowanceCacheSyncRequest<'_>,
) -> Result<ClobV2BalanceAllowanceCacheSync, BoltV3OperatorArtifactError> {
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
        Some(secrets.api_key.as_str().to_owned()),
        Some(secrets.api_secret.as_str().to_owned()),
        Some(secrets.passphrase.as_str().to_owned()),
        cfg.funder.clone(),
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "polymarket_credentials",
    })?;
    let signature_type = super::nt_signature_type(cfg.signature_type);
    let client = HttpClient::new(
        HashMap::from([(USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string())]),
        Vec::new(),
        Vec::new(),
        None,
        Some(cfg.http_timeout_secs),
        None,
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "execution.base_url_http",
    })?;
    let params = balance_allowance_update_params(signature_type);
    let method = Method::GET;
    let headers = clob_l2_auth_headers(
        &polymarket_secrets.credential,
        &polymarket_secrets.address,
        method.as_str(),
        CLOB_V2_BALANCE_ALLOWANCE_UPDATE_PATH,
    )?;
    let response = client
        .request_with_params(
            method,
            balance_allowance_update_url(&cfg.base_url_http),
            Some(&params),
            Some(headers),
            None,
            Some(cfg.http_timeout_secs),
            None,
        )
        .await
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "balance_allowance_cache_update_response",
        })?;
    if !response.status.is_success() {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "balance_allowance_cache_update_response",
        });
    }

    Ok(ClobV2BalanceAllowanceCacheSync {
        execution_client_id: execution_client_id.to_string(),
        request_path: CLOB_V2_BALANCE_ALLOWANCE_UPDATE_PATH,
        base_url_http_sha256: sha256_text(&cfg.base_url_http),
    })
}

fn balance_allowance_update_url(base_url_http: &str) -> String {
    let mut url = base_url_http.trim_end_matches('/').to_string();
    url.push_str(CLOB_V2_BALANCE_ALLOWANCE_UPDATE_PATH);
    url
}

fn balance_allowance_update_params(signature_type: SignatureType) -> GetBalanceAllowanceParams {
    GetBalanceAllowanceParams {
        asset_type: Some(AssetType::Collateral),
        token_id: None,
        signature_type: Some(signature_type),
    }
}

fn clob_l2_auth_headers(
    credential: &Credential,
    address: &str,
    method: &str,
    request_path: &str,
) -> Result<HashMap<String, String>, BoltV3OperatorArtifactError> {
    let timestamp = current_unix_timestamp_seconds()?;
    let signature = credential.sign(&timestamp, method, request_path, "");
    Ok(HashMap::from([
        ("POLY_ADDRESS".to_string(), address.to_string()),
        ("POLY_SIGNATURE".to_string(), signature),
        ("POLY_TIMESTAMP".to_string(), timestamp),
        ("POLY_API_KEY".to_string(), credential.api_key().to_string()),
        (
            "POLY_PASSPHRASE".to_string(),
            credential.passphrase().to_string(),
        ),
    ]))
}

fn current_unix_timestamp_seconds() -> Result<String, BoltV3OperatorArtifactError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "system_time",
        })?
        .as_secs();
    Ok(timestamp.to_string())
}

fn sha256_text(value: &str) -> String {
    crate::bolt_v3_source_integrity::sha256_hex_lower(value.as_bytes())
}
