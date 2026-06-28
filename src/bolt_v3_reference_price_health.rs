use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    time::Duration,
};

use anyhow::Result;
use nautilus_common::msgbus::{
    self, MStr, Pattern, ShareableMessageHandler, switchboard::get_custom_topic,
};
use nautilus_core::Params;
use nautilus_live::node::LiveNodeHandle;
use nautilus_model::{
    data::{CustomData, DataType},
    identifiers::ClientId,
};
use serde::Serialize;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_config::ReferencePriceSourceBlock,
    bolt_v3_live_node::{
        BoltV3LiveNodeRuntime, build_bolt_v3_strategy_free_live_node,
        build_bolt_v3_strategy_free_live_node_with_resolved,
    },
    bolt_v3_providers::{ReferencePriceIdentifierKind, reference_price_provider_metadata},
    bolt_v3_reference_price::{
        REFERENCE_PRICE_ASSET_PARAM, REFERENCE_PRICE_INSTRUMENT_ID_PARAM,
        REFERENCE_PRICE_PROVIDER_PARAM, REFERENCE_PRICE_SOURCE_KEY_PARAM,
        REFERENCE_PRICE_SYMBOL_PARAM, ReferencePriceUpdate,
        reference_price_source_is_runtime_available,
    },
    bolt_v3_secrets::ResolvedBoltV3Secrets,
};

const SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED: &str = "observed";
const SOURCE_UPDATE_OBSERVATION_STATUS_TIMED_OUT: &str = "timed_out";
const SOURCE_UPDATE_OBSERVATION_STATUS_NOT_OBSERVED: &str = "not_observed";
const SOURCE_UPDATE_OBSERVATION_REASON_OBSERVED: &str =
    "received reference_current_price custom data before the configured health bound";
const SOURCE_UPDATE_OBSERVATION_REASON_TIMED_OUT: &str =
    "no reference_current_price custom data was observed before the configured health bound";
const SOURCE_UPDATE_OBSERVATION_REASON_NOT_OBSERVED: &str =
    "strategy-free live node exited before reference_current_price custom data was observed";

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
    pub observation_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceHealthClientReport {
    pub client_key: String,
    pub registered_data_client_ids: Vec<String>,
    pub registered_exec_client_ids: Vec<String>,
    pub registered_strategy_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceSourceUpdateObservation {
    pub strategy_instance_id: String,
    pub source_id: String,
    pub asset: String,
    pub provider: String,
    pub provider_instrument: String,
    pub status: String,
    pub reason: String,
    pub observed_ts_ms: Option<u64>,
    pub received_ts_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCurrentPriceHealthReport {
    pub targets: Vec<ReferenceCurrentPriceHealthTarget>,
    pub clients: Vec<ReferenceCurrentPriceHealthClientReport>,
    pub source_update_observations: Vec<ReferenceCurrentPriceSourceUpdateObservation>,
}

impl ReferenceCurrentPriceHealthReport {
    pub fn all_sources_observed(&self) -> bool {
        self.source_update_observations
            .iter()
            .all(|observation| observation.status == SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED)
    }
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
    let mut observation_timeout_ms = None::<u64>;

    for strategy in &loaded.strategies {
        let Some(reference_current_price) = strategy.config.reference_current_price.as_ref() else {
            continue;
        };
        observation_timeout_ms = Some(
            observation_timeout_ms.map_or(reference_current_price.max_source_age_ms, |current| {
                current.max(reference_current_price.max_source_age_ms)
            }),
        );
        for source_id in &reference_current_price.source_order {
            let source = reference_current_price.sources.get(source_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "strategy `{}` lists reference_current_price source `{source_id}` without a matching source block",
                    strategy.config.strategy_instance_id
                )
            })?;
            if !reference_price_source_is_runtime_available(reference_current_price, source) {
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
        observation_timeout_ms: observation_timeout_ms.ok_or_else(|| {
            anyhow::anyhow!("reference_current_price health observation timeout is not configured")
        })?,
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

pub fn prepare_reference_current_price_health_run_with_resolved(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<ReferenceCurrentPriceHealthRun> {
    let plan = reference_current_price_health_plan(loaded)?;
    let runtime = build_bolt_v3_strategy_free_live_node_with_resolved(loaded, resolved)?;

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

    let subscriptions = reference_current_price_health_subscriptions(&health_run.plan)?;
    let mut subscribed: Vec<&ReferenceCurrentPriceHealthSubscription> = Vec::new();
    for subscription in &subscriptions {
        if let Err(error) = health_run.runtime.subscribe_strategy_free_custom_data(
            subscription.client_id,
            subscription.data_type.clone(),
            subscription.params.clone(),
        ) {
            for previous in subscribed.iter().rev() {
                health_run.runtime.unsubscribe_strategy_free_custom_data(
                    previous.client_id,
                    previous.data_type.clone(),
                    previous.params.clone(),
                );
            }
            return Err(error.into());
        }
        subscribed.push(subscription);
    }

    let observer =
        ReferenceCurrentPriceHealthObserver::register(&subscriptions, health_run.runtime.handle());
    let run_result = health_run
        .runtime
        .run_strategy_free_until_stop_or_timeout(
            reference_current_price_health_run_timeout(&health_run.loaded, &health_run.plan)?,
            reference_current_price_health_stop_timeout(&health_run.loaded)?,
        )
        .await;

    for subscription in subscribed.iter().rev() {
        health_run.runtime.unsubscribe_strategy_free_custom_data(
            subscription.client_id,
            subscription.data_type.clone(),
            subscription.params.clone(),
        );
    }

    let run_timed_out = match run_result {
        Ok(run_timed_out) => run_timed_out,
        Err(error) => {
            let _ = observer.into_observations(false);
            return Err(anyhow::anyhow!(
                "strategy-free reference_current_price health failed: {error}"
            ));
        }
    };
    let source_update_observations = observer.into_observations(run_timed_out);
    let clients = health_run
        .plan
        .client_keys
        .iter()
        .map(|client_key| ReferenceCurrentPriceHealthClientReport {
            client_key: client_key.clone(),
            registered_data_client_ids: registered_data_client_ids.clone(),
            registered_exec_client_ids: registered_exec_client_ids.clone(),
            registered_strategy_ids: registered_strategy_ids.clone(),
        })
        .collect();

    Ok(ReferenceCurrentPriceHealthReport {
        targets: health_run.plan.targets.clone(),
        clients,
        source_update_observations,
    })
}

fn provider_instrument(asset: &str, source: &ReferencePriceSourceBlock) -> String {
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

#[derive(Debug, Clone, PartialEq)]
struct ReferenceCurrentPriceHealthSubscription {
    strategy_instance_id: String,
    source_id: String,
    asset: String,
    provider: String,
    provider_instrument: String,
    client_id: ClientId,
    data_type: DataType,
    params: Params,
}

#[derive(Debug, Clone, PartialEq)]
struct ReferenceCurrentPriceHealthObservedUpdate {
    observed_ts_ms: u64,
    received_ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReferenceCurrentPriceHealthObservationKey {
    strategy_instance_id: String,
    source_id: String,
    asset: String,
    provider: String,
    provider_instrument: String,
}

impl ReferenceCurrentPriceHealthObservationKey {
    fn from_subscription(subscription: &ReferenceCurrentPriceHealthSubscription) -> Self {
        Self {
            strategy_instance_id: subscription.strategy_instance_id.clone(),
            source_id: subscription.source_id.clone(),
            asset: subscription.asset.clone(),
            provider: subscription.provider.clone(),
            provider_instrument: subscription.provider_instrument.clone(),
        }
    }

    fn matches_update(&self, update: &ReferencePriceUpdate) -> bool {
        update.asset() == self.asset
            && update.source_id() == self.source_id
            && update.provider() == self.provider
            && update.provider_instrument() == self.provider_instrument
    }
}

struct ReferenceCurrentPriceHealthObserver {
    expected: Vec<ReferenceCurrentPriceHealthObservationKey>,
    observed: Rc<
        RefCell<
            BTreeMap<
                ReferenceCurrentPriceHealthObservationKey,
                ReferenceCurrentPriceHealthObservedUpdate,
            >,
        >,
    >,
    handlers: Vec<(MStr<Pattern>, ShareableMessageHandler)>,
}

impl ReferenceCurrentPriceHealthObserver {
    fn register(
        subscriptions: &[ReferenceCurrentPriceHealthSubscription],
        stop_handle: LiveNodeHandle,
    ) -> Self {
        let expected = subscriptions
            .iter()
            .map(ReferenceCurrentPriceHealthObservationKey::from_subscription)
            .collect::<Vec<_>>();
        let expected_count = expected.len();
        let observed = Rc::new(RefCell::new(BTreeMap::new()));
        let mut handlers = Vec::new();
        for subscription in subscriptions {
            let key = ReferenceCurrentPriceHealthObservationKey::from_subscription(subscription);
            let pattern: MStr<Pattern> = get_custom_topic(&subscription.data_type).into();
            let observed_ref = Rc::clone(&observed);
            let stop_handle = stop_handle.clone();
            let handler = ShareableMessageHandler::from_typed(move |custom: &CustomData| {
                let Some(update) = ReferencePriceUpdate::from_custom_data(custom) else {
                    return;
                };
                if !key.matches_update(update) {
                    return;
                }
                let mut observed = observed_ref.borrow_mut();
                observed.entry(key.clone()).or_insert_with(|| {
                    ReferenceCurrentPriceHealthObservedUpdate {
                        observed_ts_ms: update.observed_ts_ms(),
                        received_ts_ms: update.received_ts_ms(),
                    }
                });
                if observed.len() >= expected_count {
                    stop_handle.stop();
                }
            });
            msgbus::subscribe_any(pattern, handler.clone(), None);
            handlers.push((pattern, handler));
        }
        Self {
            expected,
            observed,
            handlers,
        }
    }

    fn into_observations(
        self,
        run_timed_out: bool,
    ) -> Vec<ReferenceCurrentPriceSourceUpdateObservation> {
        for (pattern, handler) in &self.handlers {
            msgbus::unsubscribe_any(*pattern, handler);
        }
        let observed = self.observed.borrow();
        self.expected
            .iter()
            .map(|key| source_update_observation_for_key(key, observed.get(key), run_timed_out))
            .collect()
    }
}

fn source_update_observation_for_key(
    key: &ReferenceCurrentPriceHealthObservationKey,
    observed: Option<&ReferenceCurrentPriceHealthObservedUpdate>,
    run_timed_out: bool,
) -> ReferenceCurrentPriceSourceUpdateObservation {
    let (status, reason, observed_ts_ms, received_ts_ms) = match observed {
        Some(update) => (
            SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED,
            SOURCE_UPDATE_OBSERVATION_REASON_OBSERVED,
            Some(update.observed_ts_ms),
            Some(update.received_ts_ms),
        ),
        None if run_timed_out => (
            SOURCE_UPDATE_OBSERVATION_STATUS_TIMED_OUT,
            SOURCE_UPDATE_OBSERVATION_REASON_TIMED_OUT,
            None,
            None,
        ),
        None => (
            SOURCE_UPDATE_OBSERVATION_STATUS_NOT_OBSERVED,
            SOURCE_UPDATE_OBSERVATION_REASON_NOT_OBSERVED,
            None,
            None,
        ),
    };
    ReferenceCurrentPriceSourceUpdateObservation {
        strategy_instance_id: key.strategy_instance_id.clone(),
        source_id: key.source_id.clone(),
        asset: key.asset.clone(),
        provider: key.provider.clone(),
        provider_instrument: key.provider_instrument.clone(),
        status: status.to_string(),
        reason: reason.to_string(),
        observed_ts_ms,
        received_ts_ms,
    }
}

fn reference_current_price_health_subscriptions(
    plan: &ReferenceCurrentPriceHealthPlan,
) -> Result<Vec<ReferenceCurrentPriceHealthSubscription>> {
    plan.targets
        .iter()
        .map(reference_current_price_health_subscription)
        .collect()
}

fn reference_current_price_health_subscription(
    target: &ReferenceCurrentPriceHealthTarget,
) -> Result<ReferenceCurrentPriceHealthSubscription> {
    let data_type = ReferencePriceUpdate::data_type_for(
        target.asset.as_str(),
        target.source_id.as_str(),
        target.provider.as_str(),
    )
    .map_err(anyhow::Error::msg)?;
    let mut params = Params::new();
    params.insert(
        REFERENCE_PRICE_ASSET_PARAM.to_string(),
        serde_json::json!(target.asset),
    );
    params.insert(
        REFERENCE_PRICE_SOURCE_KEY_PARAM.to_string(),
        serde_json::json!(target.source_id),
    );
    params.insert(
        REFERENCE_PRICE_PROVIDER_PARAM.to_string(),
        serde_json::json!(target.provider),
    );
    let provider_metadata = reference_price_provider_metadata(target.provider.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "reference_current_price health source provider `{}` is unsupported",
                target.provider
            )
        })?;
    match provider_metadata.identifier_kind {
        ReferencePriceIdentifierKind::InstrumentId => {
            params.insert(
                REFERENCE_PRICE_INSTRUMENT_ID_PARAM.to_string(),
                serde_json::json!(target.provider_instrument),
            );
        }
        ReferencePriceIdentifierKind::Symbol => {
            params.insert(
                REFERENCE_PRICE_SYMBOL_PARAM.to_string(),
                serde_json::json!(target.provider_instrument),
            );
        }
    }
    Ok(ReferenceCurrentPriceHealthSubscription {
        strategy_instance_id: target.strategy_instance_id.clone(),
        source_id: target.source_id.clone(),
        asset: target.asset.clone(),
        provider: target.provider.clone(),
        provider_instrument: target.provider_instrument.clone(),
        client_id: ClientId::from(target.client_key.as_str()),
        data_type,
        params,
    })
}

fn reference_current_price_health_run_timeout(
    loaded: &LoadedBoltV3Config,
    plan: &ReferenceCurrentPriceHealthPlan,
) -> Result<Duration> {
    let startup_secs = loaded
        .root
        .nautilus
        .timeout_connection_secs
        .checked_add(loaded.root.nautilus.timeout_reconciliation_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_portfolio_secs))
        .ok_or_else(|| {
            anyhow::anyhow!("reference_current_price health startup timeout overflow")
        })?;
    Duration::from_secs(startup_secs)
        .checked_add(Duration::from_millis(plan.observation_timeout_ms))
        .ok_or_else(|| anyhow::anyhow!("reference_current_price health run timeout overflow"))
}

fn reference_current_price_health_stop_timeout(loaded: &LoadedBoltV3Config) -> Result<Duration> {
    let stop_secs = loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .ok_or_else(|| anyhow::anyhow!("reference_current_price health stop timeout overflow"))?;
    Ok(Duration::from_secs(stop_secs))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr};

    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use rust_decimal::{Decimal, prelude::ToPrimitive};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    use crate::{
        bolt_v3_config::load_bolt_v3_config,
        bolt_v3_live_node::build_bolt_v3_strategy_free_live_node_with_summary,
        bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    };

    fn fake_bolt_v3_health_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
        match path {
            "/bolt/polymarket/private-key" => Ok(format!("0x{}", "1".repeat(64))),
            "/bolt/polymarket/api-key" => Ok("polymarket-api-key".to_string()),
            "/bolt/polymarket/api-secret" => Ok("YWJj".to_string()),
            "/bolt/polymarket/api-passphrase" => Ok("polymarket-passphrase".to_string()),
            "/bolt/testnet/chainlink/api-key" => Ok("chainlink-api-key".to_string()),
            "/bolt/testnet/chainlink/api-secret" => Ok("chainlink-api-secret".to_string()),
            "/bolt/polyresearch/api-key" => Ok("polyresearch-api-key".to_string()),
            _ => {
                Err("unexpected SSM path requested by reference-current-price health fake resolver")
            }
        }
    }

    fn reference_health_target(
        source_id: &str,
        provider: &str,
        client_key: &str,
        provider_instrument: &str,
    ) -> ReferenceCurrentPriceHealthTarget {
        ReferenceCurrentPriceHealthTarget {
            strategy_instance_id: "configured_updown_main".to_string(),
            source_id: source_id.to_string(),
            asset: "BTC".to_string(),
            provider: provider.to_string(),
            client_key: client_key.to_string(),
            provider_instrument: provider_instrument.to_string(),
            required: true,
        }
    }

    #[test]
    fn reference_current_price_health_plan_uses_enabled_strategy_sources() {
        let loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");

        let plan = reference_current_price_health_plan(&loaded)
            .expect("reference_current_price health plan should build");

        assert_eq!(
            plan.client_keys,
            vec!["chainlink_reference", "polyresearch_reference"]
        );
        assert_eq!(plan.observation_timeout_ms, 2000);
        assert_eq!(plan.targets.len(), 2);
        assert!(plan.targets.iter().any(|target| {
            target.strategy_instance_id == "configured_updown_main"
                && target.source_id == "chainlink_primary"
                && target.asset == "CONFIGURED_ASSET"
                && target.provider == "chainlink_ws"
                && target.client_key == "chainlink_reference"
                && target.provider_instrument == "CONFIGURED_ASSET-USD.CHAINLINK"
                && !target.required
        }));
        assert!(plan.targets.iter().any(|target| {
            target.strategy_instance_id == "configured_updown_main"
                && target.source_id == "polyresearch_backup"
                && target.asset == "CONFIGURED_ASSET"
                && target.provider == "polyresearch_ws"
                && target.client_key == "polyresearch_reference"
                && target.provider_instrument == "CONFIGURED_ASSET/USD"
                && !target.required
        }));
    }

    #[test]
    fn reference_current_price_health_plan_includes_configured_polyresearch_assets() {
        let mut loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");
        let reference = loaded.strategies[0]
            .config
            .reference_current_price
            .as_mut()
            .expect("fixture strategy should declare reference_current_price");
        reference.asset = "BNB".to_string();
        reference
            .sources
            .get_mut("chainlink_primary")
            .expect("chainlink source should exist")
            .instrument_id = Some("BNB-USD.CHAINLINK".to_string());
        reference
            .sources
            .get_mut("polyresearch_backup")
            .expect("polyresearch source should exist")
            .symbol = Some("BNB/USD".to_string());

        let plan = reference_current_price_health_plan(&loaded)
            .expect("reference_current_price health plan should build");

        assert_eq!(
            plan.client_keys,
            vec!["chainlink_reference", "polyresearch_reference"]
        );
        assert_eq!(plan.targets.len(), 2);
        assert!(plan.targets.iter().any(|target| {
            target.source_id == "chainlink_primary"
                && target.asset == "BNB"
                && target.provider == "chainlink_ws"
                && target.client_key == "chainlink_reference"
                && target.provider_instrument == "BNB-USD.CHAINLINK"
        }));
        assert!(plan.targets.iter().any(|target| {
            target.source_id == "polyresearch_backup"
                && target.asset == "BNB"
                && target.provider == "polyresearch_ws"
                && target.client_key == "polyresearch_reference"
                && target.provider_instrument == "BNB/USD"
        }));
    }

    #[test]
    fn reference_current_price_health_subscriptions_match_strategy_custom_data_shape() {
        let loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");
        let plan = reference_current_price_health_plan(&loaded)
            .expect("reference_current_price health plan should build");

        let subscriptions = reference_current_price_health_subscriptions(&plan)
            .expect("reference_current_price health subscriptions should build");

        assert_eq!(subscriptions.len(), 2);
        let chainlink_subscription = subscriptions
            .iter()
            .find(|subscription| subscription.source_id == "chainlink_primary")
            .expect("chainlink_primary subscription should be present");
        assert_eq!(chainlink_subscription.provider, "chainlink_ws");
        assert_eq!(
            chainlink_subscription.client_id,
            ClientId::from("chainlink_reference")
        );
        let metadata = chainlink_subscription
            .data_type
            .metadata()
            .expect("reference-current-price data type should carry metadata");
        assert_eq!(
            metadata.get_str(REFERENCE_PRICE_ASSET_PARAM),
            Some("CONFIGURED_ASSET")
        );
        assert_eq!(
            metadata.get_str(REFERENCE_PRICE_SOURCE_KEY_PARAM),
            Some("chainlink_primary")
        );
        assert_eq!(
            metadata.get_str(REFERENCE_PRICE_PROVIDER_PARAM),
            Some("chainlink_ws")
        );
        assert_eq!(
            chainlink_subscription
                .params
                .get_str(REFERENCE_PRICE_INSTRUMENT_ID_PARAM),
            Some("CONFIGURED_ASSET-USD.CHAINLINK")
        );
        assert_eq!(
            chainlink_subscription
                .params
                .get_str(REFERENCE_PRICE_SYMBOL_PARAM),
            None
        );
        let polyresearch_subscription = subscriptions
            .iter()
            .find(|subscription| subscription.source_id == "polyresearch_backup")
            .expect("polyresearch_backup subscription should be present");
        assert_eq!(polyresearch_subscription.provider, "polyresearch_ws");
        assert_eq!(
            polyresearch_subscription.client_id,
            ClientId::from("polyresearch_reference")
        );
        let metadata = polyresearch_subscription
            .data_type
            .metadata()
            .expect("reference-current-price data type should carry metadata");
        assert_eq!(
            metadata.get_str(REFERENCE_PRICE_ASSET_PARAM),
            Some("CONFIGURED_ASSET")
        );
        assert_eq!(
            metadata.get_str(REFERENCE_PRICE_SOURCE_KEY_PARAM),
            Some("polyresearch_backup")
        );
        assert_eq!(
            metadata.get_str(REFERENCE_PRICE_PROVIDER_PARAM),
            Some("polyresearch_ws")
        );
        assert_eq!(
            polyresearch_subscription
                .params
                .get_str(REFERENCE_PRICE_INSTRUMENT_ID_PARAM),
            None
        );
        assert_eq!(
            polyresearch_subscription
                .params
                .get_str(REFERENCE_PRICE_SYMBOL_PARAM),
            Some("CONFIGURED_ASSET/USD")
        );
    }

    #[test]
    fn reference_current_price_health_observation_reports_custom_update() {
        let target = reference_health_target(
            "chainlink_primary",
            "chainlink_ws",
            "chainlink_reference",
            "BTC-USD.CHAINLINK",
        );
        let subscription = reference_current_price_health_subscription(&target)
            .expect("reference-current-price health subscription should build");
        let key = ReferenceCurrentPriceHealthObservationKey::from_subscription(&subscription);
        let mut observed = BTreeMap::new();
        observed.insert(
            key.clone(),
            ReferenceCurrentPriceHealthObservedUpdate {
                observed_ts_ms: 1_700_000_000_000,
                received_ts_ms: 1_700_000_000_010,
            },
        );
        let observer = ReferenceCurrentPriceHealthObserver {
            expected: vec![key],
            observed: Rc::new(RefCell::new(observed)),
            handlers: Vec::new(),
        };

        let observations = observer.into_observations(false);

        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(
            observation.status,
            SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED
        );
        assert_eq!(observation.strategy_instance_id, "configured_updown_main");
        assert_eq!(observation.source_id, "chainlink_primary");
        assert_eq!(observation.asset, "BTC");
        assert_eq!(observation.provider, "chainlink_ws");
        assert_eq!(observation.provider_instrument, "BTC-USD.CHAINLINK");
        assert_eq!(observation.observed_ts_ms, Some(1_700_000_000_000));
        assert_eq!(observation.received_ts_ms, Some(1_700_000_000_010));
    }

    #[test]
    fn reference_current_price_health_observations_are_source_scoped() {
        let primary = reference_current_price_health_subscription(&reference_health_target(
            "chainlink_primary",
            "chainlink_ws",
            "chainlink_reference",
            "BTC-USD.CHAINLINK",
        ))
        .expect("primary health subscription should build");
        let backup = reference_current_price_health_subscription(&reference_health_target(
            "polyresearch_backup",
            "polyresearch_ws",
            "polyresearch_reference",
            "BTC/USD",
        ))
        .expect("backup health subscription should build");
        let primary_key = ReferenceCurrentPriceHealthObservationKey::from_subscription(&primary);
        let backup_key = ReferenceCurrentPriceHealthObservationKey::from_subscription(&backup);
        let mut observed = BTreeMap::new();
        observed.insert(
            primary_key.clone(),
            ReferenceCurrentPriceHealthObservedUpdate {
                observed_ts_ms: 1_700_000_000_000,
                received_ts_ms: 1_700_000_000_010,
            },
        );
        let observer = ReferenceCurrentPriceHealthObserver {
            expected: vec![primary_key, backup_key],
            observed: Rc::new(RefCell::new(observed)),
            handlers: Vec::new(),
        };

        let observations = observer.into_observations(true);

        assert_eq!(observations.len(), 2);
        assert_eq!(
            observations[0].status,
            SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED
        );
        assert_eq!(observations[0].source_id, "chainlink_primary");
        assert_eq!(observations[0].provider, "chainlink_ws");
        assert_eq!(observations[0].observed_ts_ms, Some(1_700_000_000_000));
        assert_eq!(
            observations[1].status,
            SOURCE_UPDATE_OBSERVATION_STATUS_TIMED_OUT
        );
        assert_eq!(observations[1].source_id, "polyresearch_backup");
        assert_eq!(observations[1].provider, "polyresearch_ws");
        assert_eq!(observations[1].observed_ts_ms, None);
    }

    #[test]
    fn reference_current_price_health_report_requires_every_source_observed() {
        let observed = ReferenceCurrentPriceSourceUpdateObservation {
            strategy_instance_id: "configured_updown_main".to_string(),
            source_id: "chainlink_primary".to_string(),
            asset: "BTC".to_string(),
            provider: "chainlink_ws".to_string(),
            provider_instrument: "BTC-USD.CHAINLINK".to_string(),
            status: SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED.to_string(),
            reason: SOURCE_UPDATE_OBSERVATION_REASON_OBSERVED.to_string(),
            observed_ts_ms: Some(1_700_000_000_000),
            received_ts_ms: Some(1_700_000_000_010),
        };
        let timed_out = ReferenceCurrentPriceSourceUpdateObservation {
            strategy_instance_id: "configured_updown_main".to_string(),
            source_id: "polyresearch_backup".to_string(),
            asset: "BTC".to_string(),
            provider: "polyresearch_ws".to_string(),
            provider_instrument: "BTC/USD".to_string(),
            status: SOURCE_UPDATE_OBSERVATION_STATUS_TIMED_OUT.to_string(),
            reason: SOURCE_UPDATE_OBSERVATION_REASON_TIMED_OUT.to_string(),
            observed_ts_ms: None,
            received_ts_ms: None,
        };
        let mut report = ReferenceCurrentPriceHealthReport {
            targets: Vec::new(),
            clients: Vec::new(),
            source_update_observations: vec![observed.clone(), timed_out],
        };

        assert!(
            !report.all_sources_observed(),
            "health report must fail when any configured source is not observed"
        );

        report.source_update_observations = vec![observed];
        assert!(report.all_sources_observed());
    }

    #[test]
    fn reference_current_price_health_client_report_does_not_fabricate_transport_statuses() {
        let report = ReferenceCurrentPriceHealthReport {
            targets: Vec::new(),
            clients: vec![ReferenceCurrentPriceHealthClientReport {
                client_key: "chainlink_reference".to_string(),
                registered_data_client_ids: vec!["chainlink_reference".to_string()],
                registered_exec_client_ids: Vec::new(),
                registered_strategy_ids: Vec::new(),
            }],
            source_update_observations: Vec::new(),
        };

        let json = serde_json::to_value(report).expect("health report should serialize");
        let client = json
            .get("clients")
            .and_then(serde_json::Value::as_array)
            .and_then(|clients| clients.first())
            .expect("report should include one client");
        assert!(
            client.get("connect_status").is_none() && client.get("disconnect_status").is_none(),
            "health report must not fabricate transport status fields: {client}"
        );
    }

    #[test]
    fn reference_current_price_health_prepares_strategy_free_transport_runtime() {
        let mut loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");
        let catalog_directory = std::env::temp_dir().join(format!(
            "bolt-v3-reference-price-health-{}",
            std::process::id()
        ));
        loaded.root.persistence.catalog_directory =
            catalog_directory.to_string_lossy().into_owned();
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

        assert_eq!(
            health_run.plan.client_keys,
            vec!["chainlink_reference", "polyresearch_reference"]
        );
        assert_eq!(
            sorted_strings(health_run.runtime.registered_data_client_ids()),
            vec![
                "chainlink_reference",
                "okx_data",
                "polymarket_main",
                "polyresearch_reference"
            ],
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

    #[tokio::test(flavor = "current_thread")]
    async fn chainlink_binary_loopback_observes_reference_update_through_health_msgbus() {
        let mut loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config should load");
        let frame = chainlink_health_report_frame(&loaded);
        let server = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback WebSocket server should bind");
        let port = server
            .local_addr()
            .expect("loopback WebSocket server should expose local address")
            .port();
        let server_task = tokio::spawn(async move {
            let (stream, _) = server
                .accept()
                .await
                .expect("Chainlink health client should connect to loopback server");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("loopback WebSocket handshake should complete");
            websocket
                .send(WsMessage::Binary(frame.into()))
                .await
                .expect("loopback server should send binary Chainlink report frame");
            let _ = websocket.next().await;
        });

        configure_chainlink_only_loopback_health(&mut loaded, port);
        let resolved = resolve_bolt_v3_secrets_with(&loaded, fake_bolt_v3_health_resolver)
            .expect("fixture secrets should resolve through the fake SSM resolver");
        let mut health_run =
            prepare_reference_current_price_health_run_with_resolved(&loaded, &resolved)
                .expect("strategy-free health run should build with resolved secrets");
        let server_join_timeout = reference_current_price_health_stop_timeout(&loaded)
            .expect("health stop timeout should derive from fixture config")
            + Duration::from_millis(health_run.plan.observation_timeout_ms);

        let report = run_prepared_reference_current_price_health(&mut health_run)
            .await
            .expect("loopback Chainlink frame should drive the real health observer");

        assert!(
            report.all_sources_observed(),
            "Chainlink loopback binary frame should satisfy the health report: {report:?}"
        );
        assert_eq!(report.targets.len(), 1);
        assert_eq!(
            report.source_update_observations.len(),
            1,
            "narrowed health fixture should observe exactly the Chainlink source"
        );
        let observation = &report.source_update_observations[0];
        assert_eq!(observation.source_id, "chainlink_primary");
        assert_eq!(observation.provider, "chainlink_ws");
        assert_eq!(
            observation.provider_instrument,
            "CONFIGURED_ASSET-USD.CHAINLINK"
        );
        assert_eq!(
            observation.status,
            SOURCE_UPDATE_OBSERVATION_STATUS_OBSERVED
        );

        tokio::time::timeout(server_join_timeout, server_task)
            .await
            .expect("loopback server task should finish within the configured health bound")
            .expect("loopback server task should finish after client shutdown");
    }

    fn configure_chainlink_only_loopback_health(loaded: &mut LoadedBoltV3Config, port: u16) {
        loaded.root.nautilus.timeout_connection_secs = 2;
        loaded.root.nautilus.timeout_reconciliation_secs = 1;
        loaded.root.nautilus.timeout_portfolio_secs = 1;
        loaded.root.nautilus.timeout_disconnection_secs = 1;
        loaded.root.nautilus.delay_post_stop_secs = 0;
        loaded.root.nautilus.timeout_shutdown_secs = 1;
        let catalog_directory = std::env::temp_dir().join(format!(
            "bolt-v3-chainlink-health-loopback-{}",
            std::process::id()
        ));
        loaded.root.persistence.catalog_directory =
            catalog_directory.to_string_lossy().into_owned();

        let reference = loaded.strategies[0]
            .config
            .reference_current_price
            .as_mut()
            .expect("fixture strategy should declare reference_current_price");
        reference.source_order = vec!["chainlink_primary".to_string()];
        reference
            .sources
            .retain(|source_id, _| source_id == "chainlink_primary");
        reference.min_valid_sources = 1;
        let source = reference
            .sources
            .get_mut("chainlink_primary")
            .expect("fixture should carry chainlink_primary source");
        source.required = true;

        let data = loaded
            .root
            .clients
            .get_mut("chainlink_reference")
            .and_then(|client| client.data.as_mut())
            .and_then(toml::Value::as_table_mut)
            .expect("fixture should carry clients.chainlink_reference.data");
        data.insert(
            "websocket_endpoint".to_string(),
            toml::Value::String(format!("ws://127.0.0.1:{port}")),
        );
        data.insert(
            "transport_backend".to_string(),
            toml::Value::String("tungstenite".to_string()),
        );
        data.insert("heartbeat_secs".to_string(), toml::Value::Integer(1));
        data.insert(
            "reconnect_timeout_ms".to_string(),
            toml::Value::Integer(100),
        );
        data.insert(
            "reconnect_max_attempts".to_string(),
            toml::Value::Integer(0),
        );
        data.insert("idle_timeout_ms".to_string(), toml::Value::Integer(2000));
    }

    fn chainlink_health_report_frame(loaded: &LoadedBoltV3Config) -> Vec<u8> {
        let binding = loaded
            .root
            .chainlink_data_streams
            .as_ref()
            .and_then(|catalog| catalog.feed_bindings.first())
            .and_then(toml::Value::as_table)
            .expect("fixture should carry one Chainlink feed binding table");
        let feed_id = binding
            .get("feed_id")
            .and_then(toml::Value::as_str)
            .expect("fixture Chainlink feed binding should carry feed_id");
        let decimal_scale = binding
            .get("report_decimal_scale")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .expect("fixture Chainlink feed binding should carry report_decimal_scale");
        let report_source = serde_json::json!({
            "feedID": feed_id,
            "validFromTimestamp": 600,
            "observationsTimestamp": 601,
            "fullReport": format!(
                "0x{}",
                hex::encode(chainlink_health_full_report_payload(
                    feed_id,
                    decimal_scale,
                ))
            ),
        });
        serde_json::json!({ "report": report_source })
            .to_string()
            .into_bytes()
    }

    fn chainlink_health_full_report_payload(feed_id: &str, decimal_scale: u64) -> Vec<u8> {
        let observations_seconds = 601_u32;
        let mut blob = Vec::new();
        blob.extend_from_slice(&chainlink_health_feed_id_bytes(feed_id));
        blob.extend_from_slice(&chainlink_health_abi_u32_word(600));
        blob.extend_from_slice(&chainlink_health_abi_u32_word(observations_seconds));
        blob.extend_from_slice(&chainlink_health_abi_zero_word());
        blob.extend_from_slice(&chainlink_health_abi_zero_word());
        blob.extend_from_slice(&chainlink_health_abi_u32_word(observations_seconds + 60));
        blob.extend_from_slice(&chainlink_health_abi_i192_word(
            chainlink_health_scaled_price(66_300.25, decimal_scale),
        ));
        blob.extend_from_slice(&chainlink_health_abi_i192_word(
            chainlink_health_scaled_price(66_299.50, decimal_scale),
        ));
        blob.extend_from_slice(&chainlink_health_abi_i192_word(
            chainlink_health_scaled_price(66_301.00, decimal_scale),
        ));

        let mut payload = Vec::new();
        payload.extend_from_slice(&chainlink_health_abi_zero_word());
        payload.extend_from_slice(&chainlink_health_abi_zero_word());
        payload.extend_from_slice(&chainlink_health_abi_zero_word());
        payload.extend_from_slice(&chainlink_health_abi_usize_word(128));
        payload.extend_from_slice(&chainlink_health_abi_usize_word(blob.len()));
        payload.extend_from_slice(&blob);
        payload
    }

    fn chainlink_health_abi_zero_word() -> [u8; 32] {
        [0_u8; 32]
    }

    fn chainlink_health_abi_u32_word(value: u32) -> [u8; 32] {
        let mut word = [0_u8; 32];
        word[28..32].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn chainlink_health_abi_usize_word(value: usize) -> [u8; 32] {
        let mut word = [0_u8; 32];
        word[24..32].copy_from_slice(&(value as u64).to_be_bytes());
        word
    }

    fn chainlink_health_abi_i192_word(value: i128) -> [u8; 32] {
        let mut word = if value < 0 { [0xff_u8; 32] } else { [0_u8; 32] };
        word[16..32].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn chainlink_health_feed_id_bytes(feed_id: &str) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        let decoded = hex::decode(feed_id.strip_prefix("0x").expect("feed id should have 0x"))
            .expect("feed id should decode");
        bytes.copy_from_slice(&decoded);
        bytes
    }

    fn chainlink_health_scaled_price(price: f64, decimal_scale: u64) -> i128 {
        let scale = 10_i128
            .checked_pow(u32::try_from(decimal_scale).expect("scale should fit u32"))
            .expect("scale should fit i128");
        let price = Decimal::from_str_exact(&price.to_string()).expect("price should be decimal");
        (price * Decimal::from(scale))
            .round()
            .to_i128()
            .expect("scaled price should fit i128")
    }
}
