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
