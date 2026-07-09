//! Source-backed Polymarket instrument metadata gate.
//!
//! This gate prevents selected Polymarket source rows from being projected into
//! NautilusTrader data unless a matching Gamma market can be parsed by NT's own
//! Polymarket parser. It deliberately does not synthesize `BinaryOption`
//! metadata from CLOB-only or row-only fields.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::hashing::sha256_hex;
use crate::path_resolution::resolve_existing_path;
use anyhow::{Context, Result};
use nautilus_polymarket::http::{
    models::GammaMarket,
    parse::{PolymarketInstrumentDef, parse_gamma_market},
};
use serde::{Deserialize, Serialize};

pub const POLYMARKET_METADATA_GATE_REPORT_SCHEMA_VERSION: &str =
    "polymarket-nt-metadata-gate-report.v1";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolymarketMetadataGateSpec {
    pub source_binding: String,
    pub selected_token_id: String,
    pub selected_condition_id: String,
    pub gamma_markets_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolymarketMetadataGateStatus {
    Accepted,
    BlockedMissingGammaMarket,
    BlockedInvalidGammaMarket,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolymarketMetadataGateReport {
    pub schema_version: String,
    pub source_binding: String,
    pub selected_token_id: String,
    pub selected_condition_id: String,
    pub gamma_markets_path: String,
    pub gamma_markets_sha256: String,
    pub gamma_market_count: u64,
    pub matching_gamma_market_count: u64,
    pub nt_instrument_def_count: u64,
    pub selected_token_nt_def_count: u64,
    pub status: PolymarketMetadataGateStatus,
    pub nt_parser_surface: String,
    pub blocking_issues: Vec<String>,
}

pub fn evaluate_polymarket_metadata_gate(
    spec: &PolymarketMetadataGateSpec,
) -> Result<PolymarketMetadataGateReport> {
    evaluate_polymarket_metadata_gate_with_base(spec, Path::new("."))
}

pub fn evaluate_polymarket_metadata_gate_with_base(
    spec: &PolymarketMetadataGateSpec,
    base_dir: &Path,
) -> Result<PolymarketMetadataGateReport> {
    let gamma_markets_path = resolve_existing_path(base_dir, &spec.gamma_markets_path);
    let gamma_bytes = fs::read(&gamma_markets_path).with_context(|| {
        format!(
            "read Gamma markets JSON {}",
            spec.gamma_markets_path.display()
        )
    })?;
    let gamma_markets_sha256 = sha256_hex(&gamma_bytes);
    let markets: Vec<GammaMarket> = serde_json::from_slice(&gamma_bytes).with_context(|| {
        format!(
            "parse Gamma markets JSON {}",
            spec.gamma_markets_path.display()
        )
    })?;

    let mut matching_gamma_market_count = 0_u64;
    let mut nt_instrument_def_count = 0_u64;
    let mut selected_token_nt_def_count = 0_u64;
    let mut blocking_issues = Vec::new();

    for market in &markets {
        if !gamma_market_matches_selection(
            market,
            &spec.selected_condition_id,
            &spec.selected_token_id,
        )? {
            continue;
        }
        matching_gamma_market_count += 1;
        match parse_gamma_market(market) {
            Ok(defs) => {
                nt_instrument_def_count += defs.len() as u64;
                selected_token_nt_def_count +=
                    selected_token_def_count(&defs, &spec.selected_token_id);
            }
            Err(error) => blocking_issues.push(format!(
                "NT Polymarket parse_gamma_market rejected matching Gamma market: {error}"
            )),
        }
    }

    let status = if matching_gamma_market_count == 0 {
        blocking_issues.push(format!(
            "Gamma markets did not contain selected token {:?} and selected condition {:?}",
            spec.selected_token_id, spec.selected_condition_id
        ));
        PolymarketMetadataGateStatus::BlockedMissingGammaMarket
    } else if selected_token_nt_def_count == 0 {
        blocking_issues.push(format!(
            "NT Polymarket parser did not produce a definition for selected token {:?}",
            spec.selected_token_id
        ));
        PolymarketMetadataGateStatus::BlockedInvalidGammaMarket
    } else {
        PolymarketMetadataGateStatus::Accepted
    };

    Ok(PolymarketMetadataGateReport {
        schema_version: POLYMARKET_METADATA_GATE_REPORT_SCHEMA_VERSION.to_string(),
        source_binding: spec.source_binding.clone(),
        selected_token_id: spec.selected_token_id.clone(),
        selected_condition_id: spec.selected_condition_id.clone(),
        gamma_markets_path: spec.gamma_markets_path.display().to_string(),
        gamma_markets_sha256,
        gamma_market_count: markets.len() as u64,
        matching_gamma_market_count,
        nt_instrument_def_count,
        selected_token_nt_def_count,
        status,
        nt_parser_surface: "nautilus_polymarket::http::parse::parse_gamma_market".to_string(),
        blocking_issues,
    })
}

fn gamma_market_matches_selection(
    market: &GammaMarket,
    selected_condition_id: &str,
    selected_token_id: &str,
) -> Result<bool> {
    if market.condition_id != selected_condition_id {
        return Ok(false);
    }
    // The parse only runs on condition-matched markets, so a malformed
    // clob_token_ids field is corrupt source metadata for the very market
    // under selection: surface it instead of reporting a token mismatch.
    let tokens: Vec<String> = serde_json::from_str(&market.clob_token_ids).with_context(|| {
        format!(
            "Gamma market {} matched condition {selected_condition_id} but its clob_token_ids field failed to parse",
            market.condition_id
        )
    })?;
    Ok(tokens.iter().any(|token| token == selected_token_id))
}

fn selected_token_def_count(defs: &[PolymarketInstrumentDef], selected_token_id: &str) -> u64 {
    defs.iter()
        .filter(|def| def.token_id.as_str() == selected_token_id)
        .count() as u64
}
