use std::collections::BTreeSet;

use anyhow::Result;
use serde::Serialize;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{BoltV3LiveNodeRuntime, build_bolt_v3_strategy_free_live_node},
};

const SOURCE_UPDATE_OBSERVATION_STATUS: &str = "not_collected";
const SOURCE_UPDATE_OBSERVATION_REASON: &str =
    "bounded command verifies registration, connect, and disconnect only";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceHealthTarget {
    pub strategy_instance_id: String,
    pub source_id: String,
    pub asset: String,
    pub provider: String,
    pub client_key: String,
    pub provider_instrument: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceHealthPlan {
    pub targets: Vec<ReferenceCurrentPriceHealthTarget>,
    pub client_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceHealthClientReport {
    pub client_key: String,
    pub registered_data_client_ids: Vec<String>,
    pub registered_exec_client_ids: Vec<String>,
    pub registered_strategy_ids: Vec<String>,
    pub connect_status: String,
    pub disconnect_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceSourceUpdateObservation {
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceHealthReport {
    pub targets: Vec<ReferenceCurrentPriceHealthTarget>,
    pub clients: Vec<ReferenceCurrentPriceHealthClientReport>,
    pub source_update_observation: ReferenceCurrentPriceSourceUpdateObservation,
}

pub struct ReferenceCurrentPriceHealthRun {
    plan: ReferenceCurrentPriceHealthPlan,
    runtime: BoltV3LiveNodeRuntime,
    loaded: LoadedBoltV3Config,
}

pub fn reference_current_price_health_plan(
    loaded: &LoadedBoltV3Config,
) -> Result<ReferenceCurrentPriceHealthPlan> {
    let mut targets = Vec::new();
    let mut client_keys = BTreeSet::new();

    for strategy in &loaded.strategies {
        let Some(reference_current_price) = strategy.config.reference_current_price.as_ref() else {
            continue;
        };
        for source_id in &reference_current_price.source_order {
            let source = reference_current_price.sources.get(source_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "strategy `{}` lists reference_current_price source `{source_id}` without a matching source block",
                    strategy.config.strategy_instance_id
                )
            })?;
            if !source.enabled {
                continue;
            }
            let client_key = source.client_id.to_string();
            client_keys.insert(client_key.clone());
            targets.push(ReferenceCurrentPriceHealthTarget {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                source_id: source_id.clone(),
                asset: reference_current_price.asset.clone(),
                provider: source.provider.as_str().to_string(),
                client_key,
                provider_instrument: provider_instrument(
                    reference_current_price.asset.as_str(),
                    source,
                ),
                required: source.required,
            });
        }
    }

    if targets.is_empty() {
        return Err(anyhow::anyhow!(
            "no enabled reference_current_price sources are configured"
        ));
    }

    Ok(ReferenceCurrentPriceHealthPlan {
        targets,
        client_keys: client_keys.into_iter().collect(),
    })
}

pub fn prepare_reference_current_price_health_run(
    loaded: &LoadedBoltV3Config,
) -> Result<ReferenceCurrentPriceHealthRun> {
    let plan = reference_current_price_health_plan(loaded)?;
    let runtime = build_bolt_v3_strategy_free_live_node(loaded)?;

    Ok(ReferenceCurrentPriceHealthRun {
        plan,
        runtime,
        loaded: loaded.clone(),
    })
}

pub async fn run_prepared_reference_current_price_health(
    health_run: &mut ReferenceCurrentPriceHealthRun,
) -> Result<ReferenceCurrentPriceHealthReport> {
    let registered_data_client_ids =
        sorted_strings(health_run.runtime.registered_data_client_ids());
    let registered_exec_client_ids =
        sorted_strings(health_run.runtime.registered_exec_client_ids());
    let registered_strategy_ids = sorted_strings(health_run.runtime.registered_strategy_ids());
    if !registered_strategy_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "reference_current_price health registered strategies: {}",
            registered_strategy_ids.join(", ")
        ));
    }

    let registered_data_client_set = registered_data_client_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let missing_client_keys = health_run
        .plan
        .client_keys
        .iter()
        .filter(|client_key| !registered_data_client_set.contains(*client_key))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_client_keys.is_empty() {
        return Err(anyhow::anyhow!(
            "reference_current_price health strategy-free transport did not register source data client(s): {}",
            missing_client_keys.join(", ")
        ));
    }

    let connect = health_run
        .runtime
        .connect_registered_clients(&health_run.loaded)
        .await
        .map_err(|error| anyhow::anyhow!("strategy-free transport connect failed: {error}"));
    let disconnect = health_run
        .runtime
        .disconnect_registered_clients(&health_run.loaded)
        .await
        .map_err(|error| anyhow::anyhow!("strategy-free transport disconnect failed: {error}"));

    let clients = match (connect, disconnect) {
        (Ok(()), Ok(())) => health_run
            .plan
            .client_keys
            .iter()
            .map(|client_key| ReferenceCurrentPriceHealthClientReport {
                client_key: client_key.clone(),
                registered_data_client_ids: registered_data_client_ids.clone(),
                registered_exec_client_ids: registered_exec_client_ids.clone(),
                registered_strategy_ids: registered_strategy_ids.clone(),
                connect_status: "ok".to_string(),
                disconnect_status: "ok".to_string(),
            })
            .collect(),
        (Err(connect_error), Ok(())) => return Err(connect_error),
        (Ok(()), Err(disconnect_error)) => return Err(disconnect_error),
        (Err(connect_error), Err(disconnect_error)) => {
            return Err(anyhow::anyhow!("{connect_error}; {disconnect_error}"));
        }
    };

    Ok(ReferenceCurrentPriceHealthReport {
        targets: health_run.plan.targets.clone(),
        clients,
        source_update_observation: ReferenceCurrentPriceSourceUpdateObservation {
            status: SOURCE_UPDATE_OBSERVATION_STATUS.to_string(),
            reason: SOURCE_UPDATE_OBSERVATION_REASON.to_string(),
        },
    })
}

fn provider_instrument(
    asset: &str,
    source: &crate::bolt_v3_config::ReferencePriceSourceBlock,
) -> String {
    source
        .instrument_id
        .as_ref()
        .or(source.symbol.as_ref())
        .cloned()
        .unwrap_or_else(|| asset.to_string())
}

fn sorted_strings<T>(values: Vec<T>) -> Vec<String>
where
    T: ToString,
{
    let mut values = values
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    values.sort();
    values
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{
        bolt_v3_config::load_bolt_v3_config,
        bolt_v3_live_node::build_bolt_v3_strategy_free_live_node_with_summary,
    };

    fn fake_bolt_v3_health_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
        match path {
            "/bolt/polymarket/private-key" => Ok(format!("0x{}", "1".repeat(64))),
            "/bolt/polymarket/api-key" => Ok("polymarket-api-key".to_string()),
            "/bolt/polymarket/api-secret" => Ok("YWJj".to_string()),
            "/bolt/polymarket/api-passphrase" => Ok("polymarket-passphrase".to_string()),
            "/bolt/testnet/chainlink/api-key" => Ok("chainlink-api-key".to_string()),
            "/bolt/testnet/chainlink/api-secret" => Ok("chainlink-api-secret".to_string()),
            _ => {
                Err("unexpected SSM path requested by reference-current-price health fake resolver")
            }
        }
    }

    #[test]
    fn reference_current_price_health_plan_uses_enabled_strategy_sources() {
        let loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");

        let plan = reference_current_price_health_plan(&loaded)
            .expect("reference_current_price health plan should build");

        assert_eq!(plan.client_keys, vec!["chainlink_reference"]);
        assert_eq!(plan.targets.len(), 1);
        let target = &plan.targets[0];
        assert_eq!(target.strategy_instance_id, "configured_updown_main");
        assert_eq!(target.source_id, "chainlink_primary");
        assert_eq!(target.asset, "BTC");
        assert_eq!(target.provider, "chainlink_ws");
        assert_eq!(target.client_key, "chainlink_reference");
        assert_eq!(target.provider_instrument, "BTC-USD.CHAINLINK");
        assert!(target.required);
    }

    #[test]
    fn reference_current_price_health_prepares_strategy_free_transport_runtime() {
        let loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");
        let plan = reference_current_price_health_plan(&loaded)
            .expect("reference_current_price health plan should build");
        let (runtime, _summary) = build_bolt_v3_strategy_free_live_node_with_summary(
            &loaded,
            |_| false,
            fake_bolt_v3_health_resolver,
        )
        .expect("strategy-free transport runtime should build with fake secrets");
        let health_run = ReferenceCurrentPriceHealthRun {
            plan,
            runtime,
            loaded,
        };

        assert_eq!(health_run.plan.client_keys, vec!["chainlink_reference"]);
        assert_eq!(
            sorted_strings(health_run.runtime.registered_data_client_ids()),
            vec!["chainlink_reference", "okx_data", "polymarket_main"],
            "health must prepare all strategy-bound transport data clients"
        );
        assert_eq!(
            sorted_strings(health_run.runtime.registered_exec_client_ids()),
            vec!["polymarket_main"],
            "health may prepare the strategy-bound execution transport client but no order path"
        );
        assert!(
            health_run.runtime.registered_strategy_ids().is_empty(),
            "health must clear strategies from the prepared transport runtime"
        );
    }
}
