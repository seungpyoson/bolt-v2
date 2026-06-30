use super::*;

pub(super) fn validate_risk_block(block: &RiskBlock) -> Vec<String> {
    let mut errors = Vec::new();
    match parse_decimal_string(&block.default_max_notional_per_order) {
        Ok(value) if value <= Decimal::ZERO => {
            errors.push(format!(
                "risk.default_max_notional_per_order must be a positive decimal string: `{value}`",
                value = block.default_max_notional_per_order
            ));
        }
        Ok(_) => {}
        Err(reason) => {
            errors.push(format!(
                "risk.default_max_notional_per_order is not a valid decimal string ({reason}): `{value}`",
                value = block.default_max_notional_per_order
            ));
        }
    }
    if let Some(loss_governor) = block.loss_governor.as_ref() {
        if loss_governor.enabled && loss_governor.max_snapshot_age_ns == 0 {
            errors.push(
                "risk.loss_governor.max_snapshot_age_ns must be a positive integer".to_string(),
            );
        }
        if loss_governor.enabled && loss_governor.rolling_window_ns == 0 {
            errors.push(
                "risk.loss_governor.rolling_window_ns must be a positive integer".to_string(),
            );
        }
        if loss_governor.enabled
            && loss_governor
                .active_position_pnl_max_entries
                .is_none_or(|value| value == 0)
        {
            errors.push(
                "risk.loss_governor.active_position_pnl_max_entries must be a positive integer"
                    .to_string(),
            );
        }
        if loss_governor.enabled {
            for (label, threshold) in [
                (
                    "risk.loss_governor.max_per_trade_loss",
                    loss_governor.max_per_trade_loss.as_deref(),
                ),
                (
                    "risk.loss_governor.max_daily_loss",
                    loss_governor.max_daily_loss.as_deref(),
                ),
                (
                    "risk.loss_governor.max_rolling_loss",
                    loss_governor.max_rolling_loss.as_deref(),
                ),
                (
                    "risk.loss_governor.max_drawdown",
                    loss_governor.max_drawdown.as_deref(),
                ),
            ] {
                if threshold.is_none() {
                    errors.push(format!("{label} must be configured when enabled"));
                }
            }
            for (label, configured) in [
                (
                    "risk.loss_governor.on_loss_breach_trading_state",
                    loss_governor.on_loss_breach_trading_state.is_some(),
                ),
                (
                    "risk.loss_governor.on_untrusted_snapshot_trading_state",
                    loss_governor.on_untrusted_snapshot_trading_state.is_some(),
                ),
                (
                    "risk.loss_governor.recovery_mode",
                    loss_governor.recovery_mode.is_some(),
                ),
                (
                    "risk.loss_governor.manual_recovery_evidence_max_path_bytes",
                    loss_governor
                        .manual_recovery_evidence_max_path_bytes
                        .is_some(),
                ),
            ] {
                if !configured {
                    errors.push(format!("{label} must be configured when enabled"));
                }
            }
            if matches!(
                loss_governor.on_untrusted_snapshot_trading_state,
                Some(LossGovernorTradingStateAction::None)
            ) {
                errors.push(
                    "risk.loss_governor.on_untrusted_snapshot_trading_state must be reducing or halted when enabled"
                        .to_string(),
                );
            }
            if loss_governor
                .manual_recovery_evidence_max_path_bytes
                .is_some_and(|limit| limit == usize::MIN)
            {
                errors.push(
                    "risk.loss_governor.manual_recovery_evidence_max_path_bytes must be a positive integer"
                        .to_string(),
                );
            }
        }
        for (label, threshold) in [
            (
                "risk.loss_governor.max_per_trade_loss",
                loss_governor.max_per_trade_loss.as_deref(),
            ),
            (
                "risk.loss_governor.max_daily_loss",
                loss_governor.max_daily_loss.as_deref(),
            ),
            (
                "risk.loss_governor.max_rolling_loss",
                loss_governor.max_rolling_loss.as_deref(),
            ),
            (
                "risk.loss_governor.max_drawdown",
                loss_governor.max_drawdown.as_deref(),
            ),
        ] {
            let Some(value) = threshold else {
                continue;
            };
            match parse_decimal_string(value) {
                Ok(decimal) if decimal <= Decimal::ZERO => {
                    errors.push(format!(
                        "{label} must be a positive decimal string: `{value}`"
                    ));
                }
                Ok(_) => {}
                Err(reason) => {
                    errors.push(format!(
                        "{label} is not a valid decimal string ({reason}): `{value}`"
                    ));
                }
            }
        }
    }
    if let Some(capital_pools) = block.capital_pools.as_ref() {
        errors.extend(validate_capital_pools(capital_pools));
    }
    let risk_reservation_substrate_enabled = block
        .risk_reservation_substrate
        .as_ref()
        .is_some_and(|substrate| substrate.enabled);
    if risk_reservation_substrate_enabled {
        errors.push(
            "risk.risk_reservation_substrate.enabled must remain false until live admission arming is implemented; current live submit admission is controlled by risk.capital_pools[].enforce_submit_admission"
                .to_string(),
        );
    }
    if risk_reservation_substrate_enabled && block.capital_pools.as_ref().is_none_or(Vec::is_empty)
    {
        errors.push(
            "risk.risk_reservation_substrate requires at least one configured capital pool when enabled"
                .to_string(),
        );
    }
    let nt_risk_default = nautilus_live::config::LiveRiskEngineConfig::default();
    if block.nautilus.qsize != nt_risk_default.qsize {
        errors.push(format!(
            "risk.nautilus.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_risk_default.qsize
        ));
    }
    for (label, value) in [
        (
            "risk.nautilus.max_order_submit_rate",
            block.nautilus.max_order_submit_rate.as_str(),
        ),
        (
            "risk.nautilus.max_order_modify_rate",
            block.nautilus.max_order_modify_rate.as_str(),
        ),
    ] {
        if let Err(reason) = validate_rate_limit_string(value) {
            errors.push(format!(
                "{label} is not a valid Nautilus rate limit ({reason}): `{value}`"
            ));
        }
    }
    for (instrument_id, notional) in &block.nautilus.max_notional_per_order {
        // Mirrors NT's `LiveRiskEngineConfig::validate_runtime_support`;
        // keep this early-bound config validation aligned on pin bumps.
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "risk.nautilus.max_notional_per_order key `{instrument_id}` is not a valid Nautilus instrument ID ({error})"
            ));
        }
        match parse_decimal_string(notional) {
            Ok(value) if value <= Decimal::ZERO => {
                errors.push(format!(
                    "risk.nautilus.max_notional_per_order[`{instrument_id}`] must be a positive decimal string: `{notional}`"
                ));
            }
            Ok(_) => {}
            Err(reason) => {
                errors.push(format!(
                    "risk.nautilus.max_notional_per_order[`{instrument_id}`] is not a valid decimal string ({reason}): `{notional}`"
                ));
            }
        }
    }
    if let Some(kill_switch) = &block.kill_switch {
        errors.extend(validate_kill_switch_block(kill_switch));
    }
    if let Some(basket_execution) = &block.basket_execution {
        errors.extend(
            crate::bolt_v3_outcome_group_sources::validate_basket_execution(basket_execution),
        );
    }
    errors
}
