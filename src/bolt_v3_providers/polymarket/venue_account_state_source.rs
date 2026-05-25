use nautilus_polymarket::{
    common::{consts::DUST_POSITION_THRESHOLD, credential::Secrets as PolymarketSecrets},
    http::{
        clob::PolymarketClobHttpClient, data_api::PolymarketDataApiHttpClient,
        query::GetOrdersParams,
    },
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_operator_artifacts::{BoltV3OperatorArtifactError, json_artifact_sha256},
    bolt_v3_providers::{
        ExternalSnapshotConfirmationPolicy, VenueAccountStateSourceMaterialization,
        VenueAccountStateSourceMaterializationRequest, confirm_external_snapshot_before_hard_stop,
        fetch_external_snapshot_with_retries,
    },
};

const CLOB_V2_OPEN_ORDERS_PATH: &str = "/data/orders";
const POLYMARKET_DATA_API_POSITIONS_PATH: &str = "/positions";

pub async fn materialize_venue_account_state_source_from_configured_account_queries(
    request: VenueAccountStateSourceMaterializationRequest<'_>,
) -> Result<VenueAccountStateSourceMaterialization, BoltV3OperatorArtifactError> {
    let strategy = request
        .loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == request.strategy_instance_id)
        .ok_or(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "strategy_instance_id",
            },
        )?;
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let configured_target_id = request.configured_target_id;
    let client = request.loaded.root.clients.get(execution_client_id).ok_or(
        BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "execution_client_id",
        },
    )?;
    if client.venue.as_str() != super::KEY {
        return Err(
            BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                field: "execution_client_provider",
            },
        );
    }
    let execution = client.execution.as_ref().ok_or(
        BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid { field: "execution" },
    )?;
    let cfg: super::PolymarketExecutionConfig = execution.clone().try_into().map_err(|_| {
        BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid { field: "execution" }
    })?;
    let secrets = super::secrets_for(execution_client_id, request.resolved).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
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
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "polymarket_credentials",
        },
    )?;
    let credential_address = polymarket_secrets.address.clone();
    let account_address = cfg
        .funder
        .clone()
        .unwrap_or_else(|| credential_address.clone());
    let clob_client = PolymarketClobHttpClient::new(
        polymarket_secrets.credential,
        credential_address,
        Some(cfg.base_url_http.clone()),
        cfg.http_timeout_secs,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "execution.base_url_http",
        },
    )?;
    let confirmation_policy = ExternalSnapshotConfirmationPolicy::from_retry_fields(
        cfg.max_retries,
        cfg.retry_delay_initial_ms,
        cfg.retry_delay_max_ms,
    );
    let mut open_orders = fetch_external_snapshot_with_retries(confirmation_policy, || {
        clob_client.get_orders(open_orders_params())
    })
    .await
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "open_orders_response",
        },
    )?;
    open_orders = confirm_external_snapshot_before_hard_stop(
        open_orders,
        confirmation_policy,
        || clob_client.get_orders(open_orders_params()),
        |orders| !orders.is_empty(),
    )
    .await;
    let data_api_client = PolymarketDataApiHttpClient::new(
        Some(cfg.base_url_data_api.clone()),
        cfg.http_timeout_secs,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "execution.base_url_data_api",
        },
    )?;
    let mut positions = fetch_external_snapshot_with_retries(confirmation_policy, || {
        data_api_client.get_positions(&account_address)
    })
    .await
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "positions_response",
        },
    )?;
    positions = confirm_external_snapshot_before_hard_stop(
        positions,
        confirmation_policy,
        || data_api_client.get_positions(&account_address),
        |positions| has_active_positions(positions),
    )
    .await;
    let open_order_count = usize_to_u64("open_order_count", open_orders.len())?;
    let open_position_count =
        usize_to_u64("open_position_count", active_position_count(&positions))?;
    let position_summaries: Vec<DataApiPositionSummary> =
        positions.iter().map(DataApiPositionSummary::from).collect();
    let open_orders_sha256 = json_artifact_sha256(&open_orders)?;
    let open_positions_sha256 = json_artifact_sha256(&position_summaries)?;
    let base_url_http_sha256 = sha256_text(&cfg.base_url_http);
    let base_url_data_api_sha256 = sha256_text(&cfg.base_url_data_api);
    let user_address_sha256 = sha256_text(&account_address);
    let snapshot = VenueAccountStateSnapshotProof {
        schema_version: request.schema_version,
        record_kind: request.account_state_snapshot_record_kind,
        execution_client_id,
        configured_target_id,
        base_url_http_sha256: &base_url_http_sha256,
        base_url_data_api_sha256: &base_url_data_api_sha256,
        open_orders_request_path: CLOB_V2_OPEN_ORDERS_PATH,
        positions_request_path: POLYMARKET_DATA_API_POSITIONS_PATH,
        user_address_sha256: &user_address_sha256,
        open_order_count,
        open_position_count,
        open_orders_sha256: &open_orders_sha256,
        open_positions_sha256: &open_positions_sha256,
    };
    let account_state_snapshot_sha256 = json_artifact_sha256(&snapshot)?;

    Ok(VenueAccountStateSourceMaterialization {
        open_order_count,
        open_position_count,
        account_state_snapshot_sha256,
    })
}

fn open_orders_params() -> GetOrdersParams {
    GetOrdersParams {
        id: None,
        market: None,
        asset_id: None,
        next_cursor: None,
    }
}

fn usize_to_u64(field: &'static str, value: usize) -> Result<u64, BoltV3OperatorArtifactError> {
    u64::try_from(value)
        .map_err(|_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid { field })
}

fn sha256_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn active_position_count(
    positions: &[nautilus_polymarket::http::models::DataApiPosition],
) -> usize {
    positions
        .iter()
        .filter(|position| position.size >= DUST_POSITION_THRESHOLD)
        .count()
}

fn has_active_positions(positions: &[nautilus_polymarket::http::models::DataApiPosition]) -> bool {
    active_position_count(positions) > usize::MIN
}

#[derive(Serialize)]
struct DataApiPositionSummary {
    asset: String,
    condition_id: String,
    size: String,
    avg_price: Option<String>,
}

impl From<&nautilus_polymarket::http::models::DataApiPosition> for DataApiPositionSummary {
    fn from(position: &nautilus_polymarket::http::models::DataApiPosition) -> Self {
        Self {
            asset: position.asset.clone(),
            condition_id: position.condition_id.clone(),
            size: position.size.to_string(),
            avg_price: position.avg_price.map(|avg_price| avg_price.to_string()),
        }
    }
}

#[derive(Serialize)]
struct VenueAccountStateSnapshotProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    execution_client_id: &'a str,
    configured_target_id: &'a str,
    base_url_http_sha256: &'a str,
    base_url_data_api_sha256: &'a str,
    open_orders_request_path: &'static str,
    positions_request_path: &'static str,
    user_address_sha256: &'a str,
    open_order_count: u64,
    open_position_count: u64,
    open_orders_sha256: &'a str,
    open_positions_sha256: &'a str,
}
