use std::collections::HashMap;

use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_network::http::{HttpClient, Method, USER_AGENT};
use nautilus_polymarket::{
    common::{consts::DUST_POSITION_THRESHOLD, credential::Secrets as PolymarketSecrets},
    http::{clob::PolymarketClobHttpClient, query::GetOrdersParams},
};
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

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
const POLYMARKET_DATA_API_PAGE_SIZE: u32 = 100;
const POLYMARKET_DATA_API_CONTENT_TYPE_HEADER: &str = "Content-Type";
const POLYMARKET_DATA_API_CONTENT_TYPE_JSON: &str = "application/json";
/// Data-API positions query values: smallest position size to return,
/// whether already-redeemed positions are included, and the sort order the
/// venue applies before paging. All three pin the read window the readiness
/// probe depends on, so they are named rather than inlined.
const POLYMARKET_DATA_API_POSITIONS_SIZE_THRESHOLD: &str = "0";
const POLYMARKET_DATA_API_POSITIONS_REDEEMABLE: &str = "false";
const POLYMARKET_DATA_API_POSITIONS_SORT_BY: &str = "TOKENS";
const POLYMARKET_DATA_API_POSITIONS_SORT_DIRECTION: &str = "DESC";
/// Defensive fallback for a Data-API position record that omits `redeemable`:
/// an absent flag is treated as a non-redeemable (still-open) position so the
/// readiness proof never silently drops an active position.
const POSITION_REDEEMABLE_WHEN_ABSENT: bool = false;

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
        Some(secrets.api_key.as_str().to_owned()),
        Some(secrets.api_secret.as_str().to_owned()),
        Some(secrets.passphrase.as_str().to_owned()),
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
    let mut positions = fetch_external_snapshot_with_retries(confirmation_policy, || {
        fetch_non_redeemable_positions(
            &cfg.base_url_data_api,
            cfg.http_timeout_secs,
            &account_address,
        )
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
        || {
            fetch_non_redeemable_positions(
                &cfg.base_url_data_api,
                cfg.http_timeout_secs,
                &account_address,
            )
        },
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
    crate::bolt_v3_source_integrity::sha256_hex_lower(value.as_bytes())
}

async fn fetch_non_redeemable_positions(
    base_url: &str,
    timeout_secs: u64,
    user_address: &str,
) -> Result<Vec<ReadinessDataApiPosition>, BoltV3OperatorArtifactError> {
    let client = HttpClient::new(
        HashMap::from([
            (USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string()),
            (
                POLYMARKET_DATA_API_CONTENT_TYPE_HEADER.to_string(),
                POLYMARKET_DATA_API_CONTENT_TYPE_JSON.to_string(),
            ),
        ]),
        vec![],
        vec![],
        None,
        Some(timeout_secs),
        None,
    )
    .map_err(
        |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
            field: "execution.base_url_data_api",
        },
    )?;
    let base_url = base_url.trim_end_matches('/').to_string();
    let mut positions = Vec::new();
    let mut offset = 0_u32;

    loop {
        let params = positions_request_params(user_address, POLYMARKET_DATA_API_PAGE_SIZE, offset);
        let response = client
            .request_with_params(
                Method::GET,
                format!("{base_url}{POLYMARKET_DATA_API_POSITIONS_PATH}"),
                Some(&params),
                None,
                None,
                None,
                None,
            )
            .await
            .map_err(
                |_| BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                    field: "positions_response",
                },
            )?;
        if !response.status.is_success() {
            return Err(
                BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                    field: "positions_response",
                },
            );
        }
        let page: Vec<ReadinessDataApiPosition> =
            serde_json::from_slice(&response.body).map_err(|_| {
                BoltV3OperatorArtifactError::PreRunVenueAccountStateSourceInvalid {
                    field: "positions_response",
                }
            })?;
        let count = page.len() as u32;
        positions.extend(page);
        if count < POLYMARKET_DATA_API_PAGE_SIZE {
            break;
        }
        offset += count;
    }

    Ok(positions)
}

fn positions_request_params(user_address: &str, limit: u32, offset: u32) -> Vec<(String, String)> {
    vec![
        ("user".to_string(), user_address.to_string()),
        ("limit".to_string(), limit.to_string()),
        ("offset".to_string(), offset.to_string()),
        (
            "sizeThreshold".to_string(),
            POLYMARKET_DATA_API_POSITIONS_SIZE_THRESHOLD.to_string(),
        ),
        (
            "redeemable".to_string(),
            POLYMARKET_DATA_API_POSITIONS_REDEEMABLE.to_string(),
        ),
        (
            "sortBy".to_string(),
            POLYMARKET_DATA_API_POSITIONS_SORT_BY.to_string(),
        ),
        (
            "sortDirection".to_string(),
            POLYMARKET_DATA_API_POSITIONS_SORT_DIRECTION.to_string(),
        ),
    ]
}

fn active_position_count(positions: &[ReadinessDataApiPosition]) -> usize {
    let dust_position_threshold = DUST_POSITION_THRESHOLD
        .to_f64()
        .expect("Polymarket dust position threshold must fit in f64");
    positions
        .iter()
        .filter(|position| position.size >= dust_position_threshold)
        .count()
}

fn has_active_positions(positions: &[ReadinessDataApiPosition]) -> bool {
    active_position_count(positions) > usize::MIN
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessDataApiPosition {
    asset: String,
    #[serde(alias = "condition_id", alias = "conditionId")]
    condition_id: String,
    size: f64,
    #[serde(alias = "avgPrice", alias = "avg_price")]
    avg_price: Option<f64>,
    #[serde(alias = "current_value", alias = "currentValue")]
    current_value: Option<f64>,
    redeemable: Option<bool>,
}

#[derive(Serialize)]
struct DataApiPositionSummary {
    asset: String,
    condition_id: String,
    size: String,
    avg_price: Option<String>,
    current_value: Option<String>,
    redeemable: bool,
}

impl From<&ReadinessDataApiPosition> for DataApiPositionSummary {
    fn from(position: &ReadinessDataApiPosition) -> Self {
        Self {
            asset: position.asset.clone(),
            condition_id: position.condition_id.clone(),
            size: position.size.to_string(),
            avg_price: position.avg_price.map(|avg_price| avg_price.to_string()),
            current_value: position
                .current_value
                .map(|current_value| current_value.to_string()),
            redeemable: position
                .redeemable
                .unwrap_or(POSITION_REDEEMABLE_WHEN_ABSENT),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn readiness_position(size: f64, redeemable: bool) -> ReadinessDataApiPosition {
        ReadinessDataApiPosition {
            asset: "asset".to_string(),
            condition_id: "condition".to_string(),
            size,
            avg_price: None,
            current_value: Some(0.0),
            redeemable: Some(redeemable),
        }
    }

    #[test]
    fn positions_request_params_exclude_redeemable_positions() {
        let params = positions_request_params("0xabc", 100, 0);

        assert!(params.contains(&("redeemable".to_string(), "false".to_string())));
        assert!(params.contains(&("sizeThreshold".to_string(), "0".to_string())));
    }

    #[test]
    fn active_position_count_counts_only_returned_non_redeemable_positions() {
        let positions = vec![readiness_position(
            DUST_POSITION_THRESHOLD
                .to_f64()
                .expect("Polymarket dust position threshold must fit in f64"),
            false,
        )];

        assert_eq!(active_position_count(&positions), 1);
    }
}
