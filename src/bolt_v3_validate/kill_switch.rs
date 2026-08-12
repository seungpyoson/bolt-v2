use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KillSwitchFlattenResolutionError {
    MissingEnabledBlock,
    UnsupportedRouteKind,
}

impl std::fmt::Display for KillSwitchFlattenResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEnabledBlock => f.write_str(
                "risk.kill_switch.flatten_open_positions_on_breach=true requires risk.kill_switch.flatten.enabled=true",
            ),
            Self::UnsupportedRouteKind => f.write_str(
                "risk.kill_switch.flatten_open_positions_on_breach=true requires risk.kill_switch.flatten.route_kind=live_node_command_router",
            ),
        }
    }
}

enum KillSwitchFlattenBlockResolution<'a> {
    Disabled,
    Enabled(&'a KillSwitchFlattenConfigBlock),
}

fn resolve_kill_switch_flatten_block(
    block: &KillSwitchConfigBlock,
) -> Result<KillSwitchFlattenBlockResolution<'_>, KillSwitchFlattenResolutionError> {
    if !block.enabled || !block.flatten_open_positions_on_breach {
        return Ok(KillSwitchFlattenBlockResolution::Disabled);
    }
    let flatten = block
        .flatten
        .as_ref()
        .filter(|flatten| flatten.enabled)
        .ok_or(KillSwitchFlattenResolutionError::MissingEnabledBlock)?;
    if flatten.route_kind != KillSwitchFlattenRouteKindConfig::LiveNodeCommandRouter {
        return Err(KillSwitchFlattenResolutionError::UnsupportedRouteKind);
    }
    Ok(KillSwitchFlattenBlockResolution::Enabled(flatten))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoadedKillSwitchFlattenResolution<'a> {
    Disabled,
    Enabled {
        flatten: &'a KillSwitchFlattenConfigBlock,
        execution_clients_by_venue: BTreeMap<Venue, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoadedKillSwitchFlattenResolutionError {
    InvalidBlock(KillSwitchFlattenResolutionError),
    DuplicateExecutionClient {
        venue: Venue,
        first_client: String,
        second_client: String,
    },
    UnsupportedProvider {
        client_id: String,
        provider_key: String,
    },
    ProviderWithoutEconomics {
        client_id: String,
        provider_key: String,
    },
    InvalidEconomics {
        client_id: String,
        message: String,
    },
    MissingEconomics {
        client_id: String,
    },
    QuoteOnlyEconomics {
        client_id: String,
    },
}

impl std::fmt::Display for LoadedKillSwitchFlattenResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBlock(error) => error.fmt(f),
            Self::DuplicateExecutionClient {
                venue,
                first_client,
                second_client,
            } => write!(
                f,
                "kill switch flatten requires one execution client per venue; venue={venue} clients={first_client},{second_client}"
            ),
            Self::UnsupportedProvider {
                client_id,
                provider_key,
            } => write!(
                f,
                "kill switch flatten execution client `{client_id}` uses unsupported provider `{provider_key}`"
            ),
            Self::ProviderWithoutEconomics {
                client_id,
                provider_key,
            } => write!(
                f,
                "kill switch flatten execution client `{client_id}` provider `{provider_key}` has no economics binding"
            ),
            Self::InvalidEconomics { client_id, message } => write!(
                f,
                "kill switch flatten execution client `{client_id}` has invalid economics: {message}"
            ),
            Self::MissingEconomics { client_id } => write!(
                f,
                "kill switch flatten execution client `{client_id}` has no economics config"
            ),
            Self::QuoteOnlyEconomics { client_id } => write!(
                f,
                "kill switch flatten execution client `{client_id}` cannot route forced reductions while economics_slice=quote_only"
            ),
        }
    }
}

pub(crate) fn resolve_loaded_kill_switch_flatten(
    loaded: &crate::bolt_v3_config::LoadedBoltV3Config,
) -> Result<LoadedKillSwitchFlattenResolution<'_>, LoadedKillSwitchFlattenResolutionError> {
    let Some(block) = loaded.root.risk.kill_switch.as_ref() else {
        return Ok(LoadedKillSwitchFlattenResolution::Disabled);
    };
    let flatten = match resolve_kill_switch_flatten_block(block)
        .map_err(LoadedKillSwitchFlattenResolutionError::InvalidBlock)?
    {
        KillSwitchFlattenBlockResolution::Disabled => {
            return Ok(LoadedKillSwitchFlattenResolution::Disabled);
        }
        KillSwitchFlattenBlockResolution::Enabled(flatten) => flatten,
    };

    let mut execution_clients_by_venue = BTreeMap::new();
    for (client_id, client) in &loaded.root.clients {
        let Some(execution) = client.execution.as_ref() else {
            continue;
        };
        let venue = Venue::from(client.venue.as_str());
        if let Some(first_client) = execution_clients_by_venue.insert(venue, client_id.clone()) {
            return Err(
                LoadedKillSwitchFlattenResolutionError::DuplicateExecutionClient {
                    venue,
                    first_client,
                    second_client: client_id.clone(),
                },
            );
        }
        let binding = crate::bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
            .ok_or_else(
                || LoadedKillSwitchFlattenResolutionError::UnsupportedProvider {
                    client_id: client_id.clone(),
                    provider_key: client.venue.to_string(),
                },
            )?;
        let economics_binding = binding.execution_economics.ok_or_else(|| {
            LoadedKillSwitchFlattenResolutionError::ProviderWithoutEconomics {
                client_id: client_id.clone(),
                provider_key: binding.key.to_string(),
            }
        })?;
        let economics = economics_binding
            .load_and_validate(execution)
            .map_err(
                |message| LoadedKillSwitchFlattenResolutionError::InvalidEconomics {
                    client_id: client_id.clone(),
                    message,
                },
            )?
            .ok_or_else(
                || LoadedKillSwitchFlattenResolutionError::MissingEconomics {
                    client_id: client_id.clone(),
                },
            )?;
        match economics.economics_slice {
            crate::bolt_v3_config::EconomicsSliceConfig::QuoteOnly => {
                return Err(LoadedKillSwitchFlattenResolutionError::QuoteOnlyEconomics {
                    client_id: client_id.clone(),
                });
            }
        }
    }
    Ok(LoadedKillSwitchFlattenResolution::Enabled {
        flatten,
        execution_clients_by_venue,
    })
}

pub(super) fn validate_kill_switch_block(block: &KillSwitchConfigBlock) -> Vec<String> {
    let mut errors = validate_kill_switch_store_bootstrap_fields(block);
    if !block.enabled {
        return errors;
    }

    match parse_decimal_string(&block.max_utc_daily_realized_loss) {
        Ok(limit) if limit > Decimal::ZERO => {}
        Ok(_) => {
            errors.push("risk.kill_switch.max_utc_daily_realized_loss must be positive".to_string());
        }
        Err(reason) => errors.push(format!(
            "risk.kill_switch.max_utc_daily_realized_loss is not a valid decimal string ({reason}): `{}`",
            block.max_utc_daily_realized_loss
        )),
    }
    if let Err(error) = resolve_kill_switch_flatten_block(block) {
        errors.push(error.to_string());
    }
    if block.action_retry_interval_ms == 0 {
        errors.push("risk.kill_switch.action_retry_interval_ms must be positive".to_string());
    }
    if block.action_retry_timeout_ms == 0 {
        errors.push("risk.kill_switch.action_retry_timeout_ms must be positive".to_string());
    }
    if block.action_retry_interval_ms > block.action_retry_timeout_ms
        && block.action_retry_timeout_ms > 0
    {
        errors.push(
            "risk.kill_switch.action_retry_interval_ms must be <= action_retry_timeout_ms"
                .to_string(),
        );
    }
    if block.mandatory_proof_max_age_ms == 0 {
        errors.push("risk.kill_switch.mandatory_proof_max_age_ms must be positive".to_string());
    }
    if block.manual_reset_evidence_max_age_ms == 0 {
        errors
            .push("risk.kill_switch.manual_reset_evidence_max_age_ms must be positive".to_string());
    }
    if !is_sha256_hex_digest(&block.forced_reduction_policy_sha256) {
        errors.push(
            "risk.kill_switch.forced_reduction_policy_sha256 must be a 64-character SHA-256 hex digest"
                .to_string(),
        );
    }
    if block.forced_reduction_max_live_order_count == 0 {
        errors.push(
            "risk.kill_switch.forced_reduction_max_live_order_count must be positive".to_string(),
        );
    }
    match parse_decimal_string(&block.forced_reduction_max_notional_per_order) {
        Ok(notional) if notional > Decimal::ZERO => {}
        Ok(_) => errors.push(
            "risk.kill_switch.forced_reduction_max_notional_per_order must be positive"
                .to_string(),
        ),
        Err(reason) => errors.push(format!(
            "risk.kill_switch.forced_reduction_max_notional_per_order is not a valid decimal string ({reason}): `{}`",
            block.forced_reduction_max_notional_per_order
        )),
    }
    if block.authorized_operator_ids.is_empty() {
        errors.push(
            "risk.kill_switch.authorized_operator_ids must not be empty when enabled".to_string(),
        );
    }
    if block
        .authorized_operator_ids
        .iter()
        .any(|operator_id| operator_id.trim().is_empty())
    {
        errors.push(
            "risk.kill_switch.authorized_operator_ids must not contain empty values".to_string(),
        );
    }
    if block.account_ids.is_empty() {
        errors.push("risk.kill_switch.account_ids must not be empty when enabled".to_string());
    }
    if block
        .account_ids
        .iter()
        .any(|account_id| account_id.trim().is_empty())
    {
        errors.push("risk.kill_switch.account_ids must not contain empty values".to_string());
    }
    for account_id in &block.account_ids {
        if AccountId::new_checked(account_id).is_err() {
            errors.push(format!(
                "risk.kill_switch.account_ids[`{account_id}`] is not a valid Nautilus account ID"
            ));
        }
    }
    if block.instrument_ids.is_empty() {
        errors.push("risk.kill_switch.instrument_ids must not be empty when enabled".to_string());
    }
    for instrument_id in &block.instrument_ids {
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "risk.kill_switch.instrument_ids[`{instrument_id}`] is not a valid Nautilus instrument ID ({error})"
            ));
        }
    }
    if let Some(cancel) = &block.cancel {
        errors.extend(validate_kill_switch_cancel_block(cancel));
    }
    if let Some(flatten) = &block.flatten {
        errors.extend(validate_kill_switch_flatten_block(
            flatten,
            block.forced_reduction_max_live_order_count,
            &block.forced_reduction_max_notional_per_order,
        ));
    }
    errors
}

fn validate_kill_switch_store_bootstrap_fields(block: &KillSwitchConfigBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let state_path = Path::new(block.state_path.trim());
    if state_path.as_os_str().is_empty()
        || state_path.is_absolute()
        || state_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        errors.push(
            "risk.kill_switch.state_path must be a non-empty relative path under the configured root"
                .to_string(),
        );
    }
    if block.max_state_file_bytes == 0 {
        errors.push("risk.kill_switch.max_state_file_bytes must be positive".to_string());
    }
    errors
}

fn validate_kill_switch_cancel_block(block: &KillSwitchCancelConfigBlock) -> Vec<String> {
    if !block.enabled {
        return Vec::new();
    }

    let mut errors = Vec::new();
    let mut configured_surfaces = BTreeSet::new();
    for surface in &block.mandatory_surfaces {
        match parse_kill_switch_cancel_surface(surface.trim()) {
            Some(surface) => {
                configured_surfaces.insert(surface);
            }
            None => errors.push(format!(
                "risk.kill_switch.cancel.mandatory_surfaces[`{surface}`] is not a supported outstanding order risk surface"
            )),
        }
    }
    let required_surfaces = BoltV3KillSwitchOutstandingOrderRiskSurface::mandatory_surfaces()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if !required_surfaces.is_subset(&configured_surfaces) {
        errors.push(
            "risk.kill_switch.cancel.mandatory_surfaces must include every mandatory outstanding order risk surface"
                .to_string(),
        );
    }

    errors
}

fn parse_kill_switch_cancel_surface(
    value: &str,
) -> Option<BoltV3KillSwitchOutstandingOrderRiskSurface> {
    match value {
        "open" => Some(BoltV3KillSwitchOutstandingOrderRiskSurface::Open),
        "inflight" => Some(BoltV3KillSwitchOutstandingOrderRiskSurface::Inflight),
        "pending-cancel" => Some(BoltV3KillSwitchOutstandingOrderRiskSurface::PendingCancel),
        "emulated" => Some(BoltV3KillSwitchOutstandingOrderRiskSurface::Emulated),
        "algorithm-managed" => Some(BoltV3KillSwitchOutstandingOrderRiskSurface::AlgorithmManaged),
        "contingent" => Some(BoltV3KillSwitchOutstandingOrderRiskSurface::Contingent),
        "accepted-but-not-terminal" => {
            Some(BoltV3KillSwitchOutstandingOrderRiskSurface::AcceptedButNotTerminal)
        }
        _ => None,
    }
}

fn validate_kill_switch_flatten_block(
    block: &KillSwitchFlattenConfigBlock,
    global_max_live_order_count: u32,
    global_max_notional_per_order: &str,
) -> Vec<String> {
    if !block.enabled {
        return Vec::new();
    }

    let mut errors = Vec::new();
    if block.max_live_order_count == 0 {
        errors.push("risk.kill_switch.flatten.max_live_order_count must be positive".to_string());
    }
    if global_max_live_order_count > 0 && block.max_live_order_count > global_max_live_order_count {
        errors.push(
            "risk.kill_switch.flatten.max_live_order_count must be <= risk.kill_switch.forced_reduction_max_live_order_count"
                .to_string(),
        );
    }

    match (
        parse_decimal_string(&block.max_notional_per_order),
        parse_decimal_string(global_max_notional_per_order),
    ) {
        (Ok(local), Ok(global)) if local > Decimal::ZERO && local <= global => {}
        (Ok(local), Ok(_)) if local <= Decimal::ZERO => {
            errors.push(
                "risk.kill_switch.flatten.max_notional_per_order must be positive".to_string(),
            );
        }
        (Ok(_), Ok(_)) => errors.push(
            "risk.kill_switch.flatten.max_notional_per_order must be <= risk.kill_switch.forced_reduction_max_notional_per_order"
                .to_string(),
        ),
        (Err(reason), _) => errors.push(format!(
            "risk.kill_switch.flatten.max_notional_per_order is not a valid decimal string ({reason}): `{}`",
            block.max_notional_per_order
        )),
        (_, Err(_)) => {}
    }

    if !block.is_reduce_only {
        errors.push("risk.kill_switch.flatten.is_reduce_only must be true".to_string());
    }
    if block.is_quote_quantity {
        errors.push("risk.kill_switch.flatten.is_quote_quantity must be false".to_string());
    }

    let order_template = NtOrderTemplateConfig {
        order_type: block.order_type,
        time_in_force: block.time_in_force,
        expire_time_unix_nanos: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        is_post_only: block.is_post_only,
        is_reduce_only: block.is_reduce_only,
        is_quote_quantity: block.is_quote_quantity,
    };
    errors.extend(check_nt_order_template_config(
        "risk.kill_switch.flatten",
        "order_template",
        &order_template,
    ));
    errors
}
