use std::collections::BTreeSet;

use anyhow::Result;
use serde::Serialize;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{
        BoltV3LiveNodeRuntime, build_bolt_v3_strategy_free_data_client_probe_live_node,
    },
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
    probes: Vec<ReferenceCurrentPriceHealthProbe>,
}

struct ReferenceCurrentPriceHealthProbe {
    client_key: String,
    runtime: BoltV3LiveNodeRuntime,
    scoped_loaded: LoadedBoltV3Config,
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
    let mut probes = Vec::new();

    for client_key in &plan.client_keys {
        let (runtime, scoped_loaded) =
            build_bolt_v3_strategy_free_data_client_probe_live_node(loaded, client_key)?;
        probes.push(ReferenceCurrentPriceHealthProbe {
            client_key: client_key.clone(),
            runtime,
            scoped_loaded,
        });
    }

    Ok(ReferenceCurrentPriceHealthRun { plan, probes })
}

pub async fn run_prepared_reference_current_price_health(
    health_run: &mut ReferenceCurrentPriceHealthRun,
) -> Result<ReferenceCurrentPriceHealthReport> {
    let mut clients = Vec::new();

    for probe in &mut health_run.probes {
        let client_key = probe.client_key.as_str();
        let registered_data_client_ids = sorted_strings(probe.runtime.registered_data_client_ids());
        let registered_exec_client_ids = sorted_strings(probe.runtime.registered_exec_client_ids());
        let registered_strategy_ids = sorted_strings(probe.runtime.registered_strategy_ids());
        if !registered_exec_client_ids.is_empty() {
            return Err(anyhow::anyhow!(
                "reference_current_price health registered execution clients for `{client_key}`: {}",
                registered_exec_client_ids.join(", ")
            ));
        }
        if !registered_strategy_ids.is_empty() {
            return Err(anyhow::anyhow!(
                "reference_current_price health registered strategies for `{client_key}`: {}",
                registered_strategy_ids.join(", ")
            ));
        }

        let connect = probe
            .runtime
            .connect_registered_clients(&probe.scoped_loaded)
            .await
            .map_err(|error| anyhow::anyhow!("client `{client_key}` connect failed: {error}"));
        let disconnect = probe
            .runtime
            .disconnect_registered_clients(&probe.scoped_loaded)
            .await
            .map_err(|error| anyhow::anyhow!("client `{client_key}` disconnect failed: {error}"));

        match (connect, disconnect) {
            (Ok(()), Ok(())) => clients.push(ReferenceCurrentPriceHealthClientReport {
                client_key: probe.client_key.clone(),
                registered_data_client_ids,
                registered_exec_client_ids,
                registered_strategy_ids,
                connect_status: "ok".to_string(),
                disconnect_status: "ok".to_string(),
            }),
            (Err(connect_error), Ok(())) => return Err(connect_error),
            (Ok(()), Err(disconnect_error)) => return Err(disconnect_error),
            (Err(connect_error), Err(disconnect_error)) => {
                return Err(anyhow::anyhow!("{connect_error}; {disconnect_error}"));
            }
        }
    }

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
    use crate::bolt_v3_config::load_bolt_v3_config;

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
}
