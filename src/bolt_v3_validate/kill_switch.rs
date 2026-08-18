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

enum KillSwitchFlattenBlockResolution {
    Disabled,
    Enabled,
}

fn resolve_kill_switch_flatten_block(
    block: &KillSwitchConfigBlock,
) -> Result<KillSwitchFlattenBlockResolution, KillSwitchFlattenResolutionError> {
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
    Ok(KillSwitchFlattenBlockResolution::Enabled)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoadedKillSwitchFlattenResolutionError {
    InvalidBlock(KillSwitchFlattenResolutionError),
    ForcedReductionRouteUnavailable,
}

impl std::fmt::Display for LoadedKillSwitchFlattenResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBlock(error) => error.fmt(f),
            Self::ForcedReductionRouteUnavailable => write!(
                f,
                "kill switch flatten cannot route forced reductions because Slice 1 has no live forced-reduction route"
            ),
        }
    }
}

pub(crate) fn validate_loaded_kill_switch_flatten(
    loaded: &crate::bolt_v3_config::LoadedBoltV3Config,
) -> Result<(), LoadedKillSwitchFlattenResolutionError> {
    let Some(block) = loaded.root.risk.kill_switch.as_ref() else {
        return Ok(());
    };
    match resolve_kill_switch_flatten_block(block)
        .map_err(LoadedKillSwitchFlattenResolutionError::InvalidBlock)?
    {
        KillSwitchFlattenBlockResolution::Disabled => Ok(()),
        KillSwitchFlattenBlockResolution::Enabled => {
            Err(LoadedKillSwitchFlattenResolutionError::ForcedReductionRouteUnavailable)
        }
    }
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
        errors.extend(validate_kill_switch_flatten_block(flatten));
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

fn validate_kill_switch_flatten_block(block: &KillSwitchFlattenConfigBlock) -> Vec<String> {
    if !block.enabled {
        return Vec::new();
    }

    let mut errors = Vec::new();
    if block.max_live_order_count == 0 {
        errors.push("risk.kill_switch.flatten.max_live_order_count must be positive".to_string());
    }
    match parse_decimal_string(&block.max_notional_per_order) {
        Ok(notional) if notional > Decimal::ZERO => {}
        Ok(_) => {
            errors.push(
                "risk.kill_switch.flatten.max_notional_per_order must be positive".to_string(),
            );
        }
        Err(reason) => errors.push(format!(
            "risk.kill_switch.flatten.max_notional_per_order is not a valid decimal string ({reason}): `{}`",
            block.max_notional_per_order
        )),
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
