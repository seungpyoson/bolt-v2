use super::*;
use crate::bolt_v3_providers;

/// Prunes the loaded config to the client set needed by the trade transport.
///
/// RV source-client validation is intentionally enforced at the strategy
/// registration chokepoint, where the shared RV runtime is about to subscribe.
/// Callers that will register strategies must pass
/// [`RealizedVolatilityTransportScope::Subscribed`] so that chokepoint validates
/// clients present on the node transport, not merely clients present in TOML.
pub(super) fn trade_transport_loaded_config(
    loaded: &LoadedBoltV3Config,
    rv_scope: RealizedVolatilityTransportScope,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    let required_clients = trade_transport_client_keys(loaded, rv_scope)?;
    if required_clients.is_empty() {
        let mut transport_loaded = loaded.clone();
        transport_loaded.root.clients.clear();
        return Ok(transport_loaded);
    }
    let missing_clients = required_clients
        .iter()
        .filter(|client_key| !loaded.root.clients.contains_key(*client_key))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_clients.is_empty() {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy references unconfigured client(s): {}",
                missing_clients.join(", ")
            ),
        });
    }

    let mut transport_loaded = loaded.clone();
    transport_loaded
        .root
        .clients
        .retain(|client_key, _| required_clients.contains(client_key));
    validate_trade_transport_execution_venue_cardinality(&transport_loaded)?;
    Ok(transport_loaded)
}

pub(super) fn validate_trade_transport_execution_venue_cardinality(
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let mut execution_clients_by_venue: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (client_key, client) in &loaded.root.clients {
        if client.execution.is_some() {
            if let Some(client_keys) = execution_clients_by_venue.get_mut(client.venue.as_str()) {
                client_keys.push(client_key.clone());
            } else {
                execution_clients_by_venue
                    .insert(client.venue.as_str().to_string(), vec![client_key.clone()]);
            }
        }
    }
    for (venue, client_keys) in execution_clients_by_venue {
        if client_keys.len() > 1 {
            return Err(BoltV3LiveNodeError::LiveTransportScope {
                reason: format!(
                    "multiple execution clients share venue `{}` in the live transport scope: {}; only one execution client may be active per venue",
                    venue,
                    client_keys.join(", ")
                ),
            });
        }
    }
    Ok(())
}

/// Whether this transport build path will register strategies whose shared RV
/// runtime subscribes configured realized-volatility source data.
///
/// Trade builders use [`Subscribed`](Self::Subscribed). Strategy-free health and
/// probe builders use [`NotSubscribed`](Self::NotSubscribed): they may receive a
/// loaded config with strategies for adapter planning, but they clear strategy
/// actors before runtime and never subscribe RV. A future strategy-free path
/// that does subscribe RV must explicitly choose `Subscribed`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RealizedVolatilityTransportScope {
    Subscribed,
    NotSubscribed,
}

pub(super) fn trade_transport_client_keys(
    loaded: &LoadedBoltV3Config,
    rv_scope: RealizedVolatilityTransportScope,
) -> Result<BTreeSet<String>, BoltV3LiveNodeError> {
    let mut client_keys = BTreeSet::new();
    for strategy in &loaded.strategies {
        client_keys.insert(strategy.config.execution_client_id.to_string());
        if let Some(reference_current_price) = strategy.config.reference_current_price.as_ref() {
            client_keys.extend(
                reference_current_price
                    .sources
                    .values()
                    .filter(|source| {
                        reference_price_source_is_runtime_available(reference_current_price, source)
                    })
                    .map(|source| source.client_id.to_string()),
            );
        }
        for signal in strategy.config.signal_data.values() {
            let client_key = signal.data_client_id.to_string();
            if new_risk_market_data_client_available(loaded, client_key.as_str())? {
                client_keys.insert(client_key);
            }
        }
        if let Some(resolution) = strategy.config.resolution_data.as_ref() {
            client_keys.insert(resolution.data_client_id.to_string());
        }
    }
    insert_capital_admission_execution_client_keys(&mut client_keys, loaded)?;
    insert_outcome_group_source_client_keys(&mut client_keys, loaded)?;
    insert_gate_provider_client_keys(&mut client_keys, loaded)?;
    insert_realized_volatility_surface_client_keys(&mut client_keys, loaded, rv_scope)?;
    insert_iv_source_client_keys(&mut client_keys, loaded);
    Ok(client_keys)
}

fn insert_capital_admission_execution_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let Some(pools) = loaded.root.risk.capital_pools.as_ref() else {
        return Ok(());
    };
    for pool in pools.iter().filter(|pool| pool.enforce_submit_admission) {
        let matching_client_keys = loaded
            .root
            .clients
            .iter()
            .filter(|(_, client)| {
                client.venue.as_str() == pool.venue_id && client.execution.is_some()
            })
            .map(|(client_key, _)| client_key.clone())
            .collect::<Vec<_>>();
        match matching_client_keys.as_slice() {
            [] => {
                return Err(BoltV3LiveNodeError::LiveTransportScope {
                    reason: format!(
                        "capital admission pool `{}` requires one execution client for venue `{}`",
                        pool.pool_id, pool.venue_id
                    ),
                });
            }
            [client_key] => {
                client_keys.insert(client_key.clone());
            }
            _ => {
                return Err(BoltV3LiveNodeError::LiveTransportScope {
                    reason: format!(
                        "capital admission pool `{}` has multiple execution clients for venue `{}`: {}; one venue-truth source must be unambiguous",
                        pool.pool_id,
                        pool.venue_id,
                        matching_client_keys.join(", ")
                    ),
                });
            }
        }
    }
    Ok(())
}

fn insert_iv_source_client_keys(client_keys: &mut BTreeSet<String>, loaded: &LoadedBoltV3Config) {
    if let Some(iv_root) = loaded.root.iv.as_ref() {
        for profile in &iv_root.profiles {
            for source in &profile.sources {
                client_keys.insert(source.client_id.clone());
            }
        }
    }
}

fn insert_outcome_group_source_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let plan = crate::bolt_v3_market_families::market_identity_plan_from_config(loaded).map_err(
        |source| BoltV3LiveNodeError::LiveTransportScope {
            reason: source.to_string(),
        },
    )?;
    let Some(sources) = loaded.root.outcome_group_sources.as_ref() else {
        return Ok(());
    };
    for target in crate::bolt_v3_market_families::outcome_group::target_plans(&plan) {
        for source_id in &target.group_sources {
            if let Some(source) = sources.iter().find(|source| source.source_id == *source_id)
                && source.enabled
            {
                client_keys.insert(source.client_id.to_string());
            }
        }
    }
    Ok(())
}

fn insert_gate_provider_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    for strategy in &loaded.strategies {
        let Some(target) = target_gate_references(strategy)? else {
            continue;
        };
        let Some(subscriptions) = target.gate_subscriptions.as_ref() else {
            continue;
        };
        insert_gate_subscription_client_keys(
            client_keys,
            loaded.root.gate_providers.as_ref(),
            subscriptions,
            strategy.relative_path.as_str(),
        )?;
    }
    Ok(())
}

fn target_gate_references(
    strategy: &LoadedStrategy,
) -> Result<Option<TargetGateReferences>, BoltV3LiveNodeError> {
    if strategy.config.target.as_table().is_none() {
        return Ok(None);
    }
    strategy
        .config
        .target
        .clone()
        .try_into::<TargetGateReferences>()
        .map(Some)
        .map_err(|source| BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy `{}` target.gate_subscriptions could not be parsed for trade transport scoping: {source}",
                strategy.relative_path
            ),
        })
}

fn insert_gate_subscription_client_keys(
    client_keys: &mut BTreeSet<String>,
    providers: Option<&BTreeMap<String, crate::bolt_v3_config::GateProviderBlock>>,
    subscriptions: &BTreeMap<String, crate::bolt_v3_market_families::TargetGateSubscription>,
    strategy_path: &str,
) -> Result<(), BoltV3LiveNodeError> {
    for subscription in subscriptions.values() {
        let _ = (
            subscription.required,
            &subscription.allowed_provider_kinds,
            &subscription.allowed_value_kinds,
            subscription.allow_no_resolution,
        );
        if let Some(provider_ids) = &subscription.allowed_provider_ids {
            for provider_id in provider_ids {
                insert_gate_provider_client_key(
                    client_keys,
                    providers,
                    provider_id,
                    strategy_path,
                )?;
            }
        }
        if let Some(provider_ids) = &subscription.provider_preference {
            for provider_id in provider_ids {
                insert_gate_provider_client_key(
                    client_keys,
                    providers,
                    provider_id,
                    strategy_path,
                )?;
            }
        }
        if let Some(mappings) = &subscription.market_mappings {
            for mapping in mappings {
                let _ = (
                    &mapping.family_key,
                    &mapping.market_class,
                    &mapping.resolution_kind,
                    &mapping.resolution_identity,
                    &mapping.value_kind,
                );
                if let Some(provider_id) = &mapping.provider_id {
                    insert_gate_provider_client_key(
                        client_keys,
                        providers,
                        provider_id,
                        strategy_path,
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TargetGateReferences {
    gate_subscriptions:
        Option<BTreeMap<String, crate::bolt_v3_market_families::TargetGateSubscription>>,
}

fn insert_gate_provider_client_key(
    client_keys: &mut BTreeSet<String>,
    providers: Option<&BTreeMap<String, crate::bolt_v3_config::GateProviderBlock>>,
    provider_id: &str,
    strategy_path: &str,
) -> Result<(), BoltV3LiveNodeError> {
    let Some(providers) = providers else {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy `{strategy_path}` target.gate_subscriptions references provider_id `{provider_id}` but [gate_providers] is not configured"
            ),
        });
    };
    let Some(provider) = providers.get(provider_id) else {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy `{strategy_path}` target.gate_subscriptions references provider_id `{provider_id}` but [gate_providers.{provider_id}] is not configured"
            ),
        });
    };
    if let Some(client_id) = &provider.client_id {
        client_keys.insert(client_id.to_string());
    }
    Ok(())
}

fn insert_realized_volatility_surface_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
    rv_scope: RealizedVolatilityTransportScope,
) -> Result<(), BoltV3LiveNodeError> {
    if !matches!(rv_scope, RealizedVolatilityTransportScope::Subscribed)
        || loaded.strategies.is_empty()
    {
        return Ok(());
    }
    let Some(surfaces) = loaded.root.realized_volatility_surfaces.as_ref() else {
        return Ok(());
    };
    for surface in surfaces.values() {
        for source in surface.sources.iter().filter(|source| source.enabled) {
            let client_key = source.data_client_id.to_string();
            if new_risk_market_data_client_available(loaded, client_key.as_str())? {
                client_keys.insert(client_key);
            }
        }
    }
    Ok(())
}

fn new_risk_market_data_client_available(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<bool, BoltV3LiveNodeError> {
    let Some(client) = loaded.root.clients.get(client_key) else {
        return Ok(true);
    };
    bolt_v3_providers::new_risk_market_data_available(client_key, client).map_err(|reason| {
        BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "client `{client_key}` new-risk market-data capability could not be resolved: {reason}"
            ),
        }
    })
}
