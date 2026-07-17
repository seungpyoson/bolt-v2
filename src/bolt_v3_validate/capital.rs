use super::*;

pub(super) fn validate_capital_pools(pools: &[CapitalPoolBlock]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut pool_ids = HashSet::new();
    let mut venue_accounts = HashSet::new();
    let mut enforced_pool_count = 0usize;

    for pool in pools {
        let label = format!("risk.capital_pools[{}]", pool.pool_id);
        if pool.enforce_submit_admission {
            enforced_pool_count += 1;
        }
        if pool.pool_id.trim().is_empty() {
            errors.push("risk.capital_pools pool_id must be a non-empty string".to_string());
        } else if !pool_ids.insert(pool.pool_id.as_str()) {
            errors.push(format!("{label}.pool_id must be unique"));
        }
        if pool.venue_id.trim().is_empty() {
            errors.push(format!("{label}.venue_id must be a non-empty string"));
        } else if pool.enforce_submit_admission
            && pool.venue_id != pool.venue_id.to_ascii_uppercase()
        {
            errors.push(format!(
                "{label}.venue_id must be canonical uppercase when submit admission enforcement is enabled"
            ));
        } else if !venue_accounts.insert((pool.venue_id.clone(), pool.account_id.to_string())) {
            errors.push(format!(
                "{label}.venue_id/account_id pair must be unique across risk.capital_pools"
            ));
        }
        if pool.collateral_currency.trim().is_empty() {
            errors.push(format!(
                "{label}.collateral_currency must be a non-empty string"
            ));
        } else if crate::bolt_v3_strategy_registration::settlement_currency_from_config_code(
            pool.collateral_currency.as_str(),
        )
        .is_none()
        {
            errors.push(format!(
                "{label}.collateral_currency must be a registered currency code or pUSD alias: `{}`",
                pool.collateral_currency
            ));
        }
        if pool.product_kind != "prediction_market_binary" {
            errors.push(format!(
                "{label}.product_kind must be `prediction_market_binary`"
            ));
        }
        validate_prediction_market_binary_product_metadata(pool, &label, &mut errors);
        validate_positive_decimal(
            &format!("{label}.max_pool_liability"),
            &pool.max_pool_liability,
            &mut errors,
        );
        if pool.max_snapshot_age_ns == 0 {
            errors.push(format!(
                "{label}.max_snapshot_age_ns must be a positive integer"
            ));
        }
        if pool.dedupe_retention_ns == 0 {
            errors.push(format!(
                "{label}.dedupe_retention_ns must be a positive integer"
            ));
        }
        validate_venue_spendability_source_binding(pool, &label, &mut errors);
        if let Some(min_remaining_pool_balance) = pool
            .capital_admission_policy
            .min_remaining_pool_balance
            .as_ref()
        {
            validate_positive_decimal(
                &format!("{label}.capital_admission_policy.min_remaining_pool_balance"),
                min_remaining_pool_balance,
                &mut errors,
            );
        }
        validate_positive_decimal(
            &format!("{label}.capital_admission_policy.fee_slippage.max_fee_liability"),
            &pool.capital_admission_policy.fee_slippage.max_fee_liability,
            &mut errors,
        );
        validate_positive_decimal(
            &format!("{label}.capital_admission_policy.fee_slippage.max_slippage_liability"),
            &pool
                .capital_admission_policy
                .fee_slippage
                .max_slippage_liability,
            &mut errors,
        );
    }

    if enforced_pool_count > 1 {
        errors.push(
            "risk.capital_pools may enable submit admission enforcement for at most one pool"
                .to_string(),
        );
    }

    errors
}

fn validate_venue_spendability_source_binding(
    pool: &CapitalPoolBlock,
    label: &str,
    errors: &mut Vec<String>,
) {
    let has_binding = pool.venue_spendability_source_path.is_some()
        || pool.venue_spendability_source_sha256.is_some()
        || pool.venue_spendability_source_max_bytes.is_some();
    if !has_binding {
        return;
    }
    if !pool.enforce_submit_admission {
        errors.push(format!(
            "{label}.venue_spendability_source_path requires enforce_submit_admission = true"
        ));
    }
    match pool.venue_spendability_source_path.as_deref() {
        Some(path) if !path.trim().is_empty() => {}
        _ => errors.push(format!(
            "{label}.venue_spendability_source_path must be a non-empty string"
        )),
    }
    match pool.venue_spendability_source_sha256.as_deref() {
        Some(sha256) if is_lowercase_sha256_hex(sha256) => {}
        _ => errors.push(format!(
            "{label}.venue_spendability_source_sha256 must be a lowercase sha256 hex string"
        )),
    }
    match pool.venue_spendability_source_max_bytes {
        Some(max_bytes) if max_bytes > 0 => {}
        _ => errors.push(format!(
            "{label}.venue_spendability_source_max_bytes must be positive"
        )),
    }
}

fn validate_prediction_market_binary_product_metadata(
    pool: &CapitalPoolBlock,
    label: &str,
    errors: &mut Vec<String>,
) {
    let Some(product) = pool.prediction_market_binary.as_ref() else {
        if pool.enforce_submit_admission && pool.product_kind == "prediction_market_binary" {
            errors.push(format!(
                "{label}.prediction_market_binary is required when prediction-market submit admission is enforced"
            ));
        }
        return;
    };

    if pool.product_kind != "prediction_market_binary" {
        errors.push(format!(
            "{label}.prediction_market_binary is only supported for prediction_market_binary pools"
        ));
    }
    if product.yes_instrument_id == product.no_instrument_id {
        errors.push(format!(
            "{label}.prediction_market_binary.yes_instrument_id and no_instrument_id must differ"
        ));
    }
    if product.collateral_coupled_group_id.trim().is_empty() {
        errors.push(format!(
            "{label}.prediction_market_binary.collateral_coupled_group_id must be a non-empty string"
        ));
    }
}

fn validate_positive_decimal(label: &str, value: &str, errors: &mut Vec<String>) {
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

pub(super) fn is_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
