//! Per-provider binding for `binance` venue config block shapes and
//! per-venue startup validation.
//!
//! Owns the concrete shape of `[venues.<name>.data]` and
//! `[venues.<name>.secrets]` for any venue whose `kind = "binance"`
//! provider key is configured. Core config in `crate::bolt_v3_config`
//! only owns the root/strategy envelope and raw provider-key field; the
//! provider-shaped block types and their
//! serde rules live here so provider-specific schema evolution does
//! not reach back into the envelope module.
//!
//! This module also owns the per-venue startup-validation policy for
//! Binance venues: the no-execution rule for the current bolt-v3
//! scope, typed deserialization of each present block, cross-block
//! presence rules ([data] requires [secrets]; [secrets] is only
//! allowed alongside [data]), Binance data bounds, and Binance
//! secret-path ownership. Core startup validation in
//! `crate::bolt_v3_validate` dispatches into
//! `bolt_v3_providers::validate_venue_block`, which routes Binance
//! venues here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

use serde::Deserialize;

use crate::bolt_v3_config::VenueBlock;

pub const KEY: &str = "binance";
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_binance::common::credential"];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceDataConfig {
    pub product_types: Vec<BinanceProductType>,
    pub environment: BinanceEnvironment,
    /// Required HTTP base URL passed through to
    /// `nautilus_binance::config::BinanceDataClientConfig.base_url_http`
    /// as `Some(...)` so NT does not silently fall back to the
    /// compiled-in default endpoint.
    pub base_url_http: String,
    /// Required WebSocket base URL passed through to
    /// `nautilus_binance::config::BinanceDataClientConfig.base_url_ws`
    /// as `Some(...)` so NT does not silently fall back to the
    /// compiled-in default endpoint.
    pub base_url_ws: String,
    pub instrument_status_poll_seconds: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceProductType {
    Spot,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceEnvironment {
    Mainnet,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceSecretsConfig {
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
}

pub fn validate_venue(key: &str, venue: &VenueBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if venue.execution.is_some() {
        errors.push(format!(
            "venues.{key} (kind=binance) is not allowed to declare an [execution] block in the current bolt-v3 scope"
        ));
    }
    if let Some(data) = &venue.data {
        match data.clone().try_into::<BinanceDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.data: {message}")),
        }
        // Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3
        // and docs/bolt-v3/2026-04-25-bolt-v3-schema.md Section 5, every
        // Binance reference-data venue must resolve credentials through SSM.
        // Mirror the Polymarket-execution rule in the polymarket binding:
        // the secret-block requirement is the gate that makes the env-var
        // blocklist effective.
        if venue.secrets.is_none() {
            errors.push(format!(
                "venues.{key} (kind=binance) declares [data] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every Binance reference-data venue"
            ));
        }
    }
    if let Some(secrets) = &venue.secrets {
        if venue.data.is_none() {
            errors.push(format!(
                "venues.{key} (kind=binance) declares [secrets] but no [data] block is configured; \
                 Binance [secrets] are only allowed alongside the data adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<BinanceSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &BinanceDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let url_fields: &[(&str, &str)] = &[
        ("base_url_http", data.base_url_http.as_str()),
        ("base_url_ws", data.base_url_ws.as_str()),
    ];
    for (field, value) in url_fields {
        if value.trim().is_empty() {
            errors.push(format!("venues.{key}.data.{field} must be a non-empty URL"));
        }
    }
    // The bolt-v3 schema deliberately rejects `0` rather than treating
    // it as "polling disabled": NT's `BinanceDataClientConfig` consumes
    // this as a poll interval and a missing/zero value would leave NT
    // free to fall back to its own default cadence. Failing closed
    // here keeps the bolt-v3 instrument-status-poll cadence explicit.
    if data.instrument_status_poll_seconds == 0 {
        errors.push(format!(
            "venues.{key}.data.instrument_status_poll_seconds must be a positive integer"
        ));
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &BinanceSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("api_key_ssm_path", &secrets.api_key_ssm_path),
        ("api_secret_ssm_path", &secrets.api_secret_ssm_path),
    ];
    for (field, value) in path_fields {
        errors.extend(crate::bolt_v3_validate::validate_ssm_parameter_path(
            key, field, value,
        ));
    }
    errors
}
