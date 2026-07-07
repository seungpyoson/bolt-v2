use super::*;

pub fn build_bolt_v3_live_node_with_resolved(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    // RV source-client validation is owned by the strategy-registration
    // chokepoint; trade transport must retain the clients it will validate.
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::Subscribed)?;
    check_no_forbidden_credential_env_vars(&transport_loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    build_bolt_v3_live_node_from_resolved_transport(&transport_loaded, resolved)
}

fn build_bolt_v3_live_node_from_resolved_transport(
    transport_loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let bundle =
        live_node_adapter_bundle_with_provider_live_submit_approvals(transport_loaded, resolved)?;
    let (runtime, _summary) = build_live_node_with_clients_and_submit_approval_limits(
        transport_loaded,
        resolved,
        bundle.configs,
        bundle.live_submit_approval_limits,
    )?;
    Ok(runtime)
}

fn resolve_bolt_v3_live_node_secrets(
    loaded: &LoadedBoltV3Config,
) -> Result<ResolvedBoltV3Secrets, BoltV3LiveNodeError> {
    check_no_forbidden_credential_env_vars(&loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    // Per #252 design review: own the resolver session at the bolt-v3
    // startup boundary so a single AWS SDK config + SsmClient cache covers
    // every secret resolution in this build, and so the session lifetime is
    // visible to the caller of `resolve_bolt_v3_secrets`. Session-setup
    // failure surfaces as the dedicated `SecretResolverSetup` variant
    // (#255-2) so operator-facing messages don't pretend a venue or SSM
    // path is involved before any path has been read.
    let session = SsmResolverSession::new().map_err(BoltV3LiveNodeError::SecretResolverSetup)?;
    resolve_bolt_v3_secrets(&session, loaded).map_err(BoltV3LiveNodeError::SecretResolution)
}

pub(super) fn live_node_adapter_bundle_with_provider_live_submit_approvals(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeAdapterBundle, BoltV3LiveNodeError> {
    if configured_provider_live_submit_client_count(loaded)? == 0 {
        return Ok(BoltV3LiveNodeAdapterBundle {
            configs: map_bolt_v3_adapters(loaded, resolved)
                .map_err(BoltV3LiveNodeError::AdapterMapping)?,
            live_submit_approval_limits: BTreeMap::new(),
        });
    }
    let build_head_sha = current_build_head_sha().ok_or_else(|| {
        BoltV3LiveNodeError::OperatorApprovalConsumption(anyhow::anyhow!(
            "bolt-v3 build head_sha is unavailable or invalid"
        ))
    })?;
    let now_unix_seconds = current_unix_seconds_u64()?;
    live_node_adapter_bundle_with_provider_approvals_at(
        loaded,
        resolved,
        now_unix_seconds,
        build_head_sha,
    )
}

pub(super) fn live_node_adapter_bundle_with_provider_approvals_at(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    now_unix_seconds: u64,
    build_head_sha: &str,
) -> Result<BoltV3LiveNodeAdapterBundle, BoltV3LiveNodeError> {
    let approvals = load_provider_live_submit_approvals_for_live_node(
        loaded,
        resolved,
        now_unix_seconds,
        build_head_sha,
    )?;
    if approvals.is_empty() {
        return Ok(BoltV3LiveNodeAdapterBundle {
            configs: map_bolt_v3_adapters(loaded, resolved)
                .map_err(BoltV3LiveNodeError::AdapterMapping)?,
            live_submit_approval_limits: BTreeMap::new(),
        });
    }
    let configs = map_bolt_v3_adapters_with_runtime_approvals(
        loaded,
        resolved,
        ProviderRuntimeApprovals {
            live_submit: Some(&approvals),
        },
    )
    .map_err(BoltV3LiveNodeError::AdapterMapping)?;
    Ok(BoltV3LiveNodeAdapterBundle {
        configs,
        live_submit_approval_limits: live_submit_approval_limits_for_submit_admission(&approvals),
    })
}

fn live_submit_approval_limits_for_submit_admission(
    approvals: &ProviderLiveSubmitApprovals,
) -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
    approvals
        .order_limits()
        .map(|(client_key, order_limits)| {
            (
                client_key.clone(),
                BoltV3LiveSubmitApprovalLimits {
                    max_order_count: order_limits.max_order_count,
                    max_order_notional: order_limits.max_order_notional,
                },
            )
        })
        .collect()
}

fn configured_provider_live_submit_client_count(
    loaded: &LoadedBoltV3Config,
) -> Result<usize, BoltV3LiveNodeError> {
    let mut count = 0;
    for client in loaded.root.clients.values() {
        let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
        else {
            continue;
        };
        if binding.load_live_submit_approval.is_some() && client.execution.is_some() {
            count += 1;
        }
    }
    Ok(count)
}

pub(super) fn load_provider_live_submit_approvals_for_live_node(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    now_unix_seconds: u64,
    build_head_sha: &str,
) -> Result<ProviderLiveSubmitApprovals, BoltV3LiveNodeError> {
    let mut approvals = ProviderLiveSubmitApprovals::empty();
    for (client_key, client) in &loaded.root.clients {
        let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
        else {
            continue;
        };
        let Some(load_live_submit_approval) = binding.load_live_submit_approval else {
            continue;
        };
        if let Some(approval) = load_live_submit_approval(ProviderLiveSubmitApprovalContext {
            loaded,
            client_key,
            client,
            resolved,
            product_surface: None,
            now_unix_seconds,
            build_head_sha,
        })
        .map_err(BoltV3LiveNodeError::OperatorApprovalConsumption)?
        {
            approvals.insert(client_key.clone(), approval);
        }
    }
    Ok(approvals)
}

fn current_unix_seconds_u64() -> Result<u64, BoltV3LiveNodeError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| {
            BoltV3LiveNodeError::OperatorApprovalConsumption(anyhow::Error::new(source))
        })?
        .as_secs())
}

pub(super) fn current_unix_nanos() -> Result<u64> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    u64::try_from(nanos).map_err(|_| anyhow::anyhow!("current unix nanoseconds exceed u64"))
}

pub fn build_bolt_v3_strategy_free_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::NotSubscribed)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    build_bolt_v3_strategy_free_live_node_from_resolved_transport(&transport_loaded, &resolved)
}

pub fn build_bolt_v3_strategy_free_live_node_for_data_clients(
    loaded: &LoadedBoltV3Config,
    data_client_keys: &[String],
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let scoped_loaded =
        strategy_free_data_client_transport_loaded_config(loaded, data_client_keys)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&scoped_loaded)?;
    build_bolt_v3_strategy_free_live_node_from_resolved_transport(&scoped_loaded, &resolved)
}

pub fn check_bolt_v3_strategy_free_live_node_for_data_clients_forbidden_env_vars_with<F>(
    loaded: &LoadedBoltV3Config,
    data_client_keys: &[String],
    env_is_set: F,
) -> Result<(), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
{
    let scoped_loaded =
        strategy_free_data_client_transport_loaded_config(loaded, data_client_keys)?;
    check_no_forbidden_credential_env_vars_with(&scoped_loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)
}

pub fn build_bolt_v3_strategy_free_live_node_with_resolved(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::NotSubscribed)?;
    check_no_forbidden_credential_env_vars(&transport_loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    build_bolt_v3_strategy_free_live_node_from_resolved_transport(&transport_loaded, resolved)
}

pub fn build_bolt_v3_strategy_free_live_node_with_resolved_for_data_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    data_client_keys: &[String],
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let scoped_loaded =
        strategy_free_data_client_transport_loaded_config(loaded, data_client_keys)?;
    check_no_forbidden_credential_env_vars(&scoped_loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let scoped_resolved = resolved_secrets_for_loaded_clients(resolved, &scoped_loaded);
    build_bolt_v3_strategy_free_live_node_from_resolved_transport(&scoped_loaded, &scoped_resolved)
}

fn build_bolt_v3_strategy_free_live_node_from_resolved_transport(
    transport_loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let adapters = strategy_free_transport_adapter_configs(transport_loaded, resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(transport_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, resolved, adapters)?;
    Ok(runtime)
}

pub fn build_bolt_v3_strategy_free_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::NotSubscribed)?;
    check_no_forbidden_credential_env_vars_with(&transport_loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(&transport_loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters = strategy_free_transport_adapter_configs(&transport_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&transport_loaded);
    build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)
}

#[cfg(test)]
pub(crate) fn build_bolt_v3_strategy_free_live_node_for_data_clients_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
    data_client_keys: &[String],
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let scoped_loaded =
        strategy_free_data_client_transport_loaded_config(loaded, data_client_keys)?;
    check_no_forbidden_credential_env_vars_with(&scoped_loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(&scoped_loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters = strategy_free_transport_adapter_configs(&scoped_loaded, &resolved)?;
    build_live_node_with_clients(&scoped_loaded, &resolved, adapters)
}

fn strategy_free_data_client_transport_loaded_config(
    loaded: &LoadedBoltV3Config,
    data_client_keys: &[String],
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    let requested = data_client_keys.iter().cloned().collect::<BTreeSet<_>>();
    let missing_clients = requested
        .iter()
        .filter(|client_key| !loaded.root.clients.contains_key(*client_key))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_clients.is_empty() {
        let reason = stringify!(strategy_free_data_client_transport_unconfigured_clients);
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!("{reason}: {missing_clients:?}"),
        });
    }
    let missing_data_clients = requested
        .iter()
        .filter(|client_key| {
            loaded
                .root
                .clients
                .get(*client_key)
                .and_then(|client| client.data.as_ref())
                .is_none()
        })
        .cloned()
        .collect::<Vec<_>>();
    if !missing_data_clients.is_empty() {
        let reason = stringify!(strategy_free_data_client_transport_clients_without_data);
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!("{reason}: {missing_data_clients:?}"),
        });
    }

    let mut scoped_loaded = loaded.clone();
    scoped_loaded.strategies.clear();
    scoped_loaded
        .root
        .clients
        .retain(|client_key, _| requested.contains(client_key));
    for client in scoped_loaded.root.clients.values_mut() {
        client.execution = None;
        if !data_client_scope_requires_secrets(client) {
            client.secrets = None;
        }
    }
    disable_strategy_free_data_client_live_subsystems(&mut scoped_loaded);
    Ok(scoped_loaded)
}

fn disable_strategy_free_data_client_live_subsystems(loaded: &mut LoadedBoltV3Config) {
    loaded.root.iv = None;
    let crate::bolt_v3_config::RiskBlock {
        default_max_notional_per_order: _,
        live_submit_governance,
        loss_governor,
        capital_pools,
        risk_reservation_substrate,
        nautilus: _,
        kill_switch,
        basket_execution,
    } = &mut loaded.root.risk;
    *live_submit_governance = None;
    *loss_governor = None;
    *capital_pools = None;
    *risk_reservation_substrate = None;
    *kill_switch = None;
    *basket_execution = None;
    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = None;
}

fn data_client_scope_requires_secrets(client: &ClientBlock) -> bool {
    let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str()) else {
        return client.secrets.is_some();
    };
    binding
        .required_secret_blocks
        .iter()
        .any(|requirement| match requirement.block {
            bolt_v3_providers::ProviderCredentialedBlock::Data => client.data.is_some(),
            bolt_v3_providers::ProviderCredentialedBlock::Execution => client.execution.is_some(),
        })
}

fn resolved_secrets_for_loaded_clients(
    resolved: &ResolvedBoltV3Secrets,
    loaded: &LoadedBoltV3Config,
) -> ResolvedBoltV3Secrets {
    ResolvedBoltV3Secrets {
        clients: resolved
            .clients
            .iter()
            .filter(|(client_key, _)| {
                loaded
                    .root
                    .clients
                    .get(*client_key)
                    .and_then(|client| client.secrets.as_ref())
                    .is_some()
            })
            .map(|(client_key, secrets)| (client_key.clone(), secrets.clone()))
            .collect(),
    }
}

pub fn build_bolt_v3_strategy_free_data_client_probe_live_node(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<(BoltV3LiveNodeRuntime, LoadedBoltV3Config), BoltV3LiveNodeError> {
    let probe_loaded = data_client_probe_loaded_config(loaded, client_key)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&probe_loaded)?;
    let adapters = strategy_free_transport_adapter_configs(&probe_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&probe_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)?;
    Ok((runtime, strategy_free_loaded))
}
