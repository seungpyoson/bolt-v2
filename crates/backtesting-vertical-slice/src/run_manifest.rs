//! Gate 4 — typed `BacktestingRunManifest` and NautilusTrader config mapping.
//!
//! The run manifest is the backtest recipe. It carries run intent plus the
//! fields needed to build the NautilusTrader `BacktestRunConfig`,
//! `BacktestDataConfig`, and `BacktestVenueConfig`, and it is validated to
//! reject inline strategy code, Python strategy paths, untracked config blobs,
//! and unaccepted data before any run.
//!
//! Strategy sources are restricted to existing compiled Rust strategies selected
//! by a registry key (see [`registered_strategies`]); the manifest never carries
//! executable strategy code or a runtime path.

use std::{collections::BTreeMap, str::FromStr};

use anyhow::{Result, bail};
use nautilus_backtest::config::{
    BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::BarType,
    enums::{AccountType, BookType, OmsType},
    identifiers::InstrumentId,
    types::{Money, Quantity},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use super::source_proof::{AcceptedDataset, FixtureType, SourceProofFidelityClass};

/// Registry key for the compiled Rust trade-driven example strategy.
pub const STRATEGY_HURST_VPIN_DIRECTIONAL: &str = "hurst_vpin_directional";
/// Strategy parameter key for the bar type.
pub const STRATEGY_PARAM_BAR_TYPE: &str = "bar_type";
/// Strategy parameter key for the trade size.
pub const STRATEGY_PARAM_TRADE_SIZE: &str = "trade_size";

/// Existing compiled Rust strategies selectable from a run manifest.
///
/// This is the single source of truth shared by manifest validation (gate 4)
/// and strategy instantiation (gate 5 runner).
#[must_use]
pub fn registered_strategies() -> &'static [&'static str] {
    &[STRATEGY_HURST_VPIN_DIRECTIONAL]
}

#[must_use]
pub fn registered_strategy_parameters(registry_key: &str) -> Option<&'static [&'static str]> {
    match registry_key {
        STRATEGY_HURST_VPIN_DIRECTIONAL => {
            Some(&[STRATEGY_PARAM_BAR_TYPE, STRATEGY_PARAM_TRADE_SIZE])
        }
        _ => None,
    }
}

/// Market-structure fixture family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarketStructureFixture {
    BinaryOption,
    PerpsSpot,
}

/// Backtest run purpose. `Normal` runs cannot pin a non-latest source proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPurpose {
    Normal,
    Reproduction,
    Audit,
    Regression,
    Migration,
}

/// The only admissible strategy source: a registered compiled Rust strategy
/// selected by key, with typed string parameters. There is deliberately no
/// variant for inline code, notebook code, a Python path, or an untracked blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategySource {
    /// Key into [`registered_strategies`].
    pub registry_key: String,
    /// Typed parameters passed to the registered strategy constructor.
    pub parameters: BTreeMap<String, String>,
}

/// Simulated venue settings mapped into [`BacktestVenueConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestVenueConfig {
    /// NautilusTrader venue name, for example `BYBIT`.
    pub nt_venue: String,
    /// One of `NETTING` or `HEDGING`.
    pub oms_type: String,
    /// One of `CASH` or `MARGIN`.
    pub account_type: String,
    /// One of `L1_MBP`, `L2_MBP`, `L3_MBO`.
    pub book_type: String,
    /// Starting balances such as `["1_000_000 USDC"]`.
    pub starting_balances: Vec<String>,
}

/// Catalog input mapped into [`BacktestDataConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCatalogInput {
    pub catalog_path: String,
    /// NautilusTrader data type, currently `TradeTick`.
    pub data_type: String,
    /// NautilusTrader instrument id, for example `BNBUSDC.BYBIT`.
    pub nt_instrument_id: String,
}

/// The typed backtest run manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestingRunManifest {
    pub run_id: String,
    pub market_structure_fixture: MarketStructureFixture,
    /// TOML/registry venue/provider binding key.
    pub venue_binding_key: String,
    pub run_purpose: RunPurpose,
    /// Source proof id/version governing the catalog input.
    pub source_proof_id: String,
    pub source_proof_version: u32,
    /// True when this manifest pins a non-latest accepted proof.
    pub pins_non_latest_proof: bool,
    pub strategy: StrategySource,
    pub venue: ManifestVenueConfig,
    pub catalog_input: ManifestCatalogInput,
    /// Configured S3 artifact root (TOML/config-owned).
    pub artifact_root: String,
    /// Output prefix under `artifact_root/backtests/`.
    pub output_prefix: String,
    /// Optional inclusive start time (Unix nanos).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub start_time: Option<i64>,
    /// Optional inclusive end time (Unix nanos).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub end_time: Option<i64>,
}

/// Why a manifest is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    MissingField(&'static str),
    InlineStrategyCode {
        registry_key: String,
    },
    PythonStrategyPath {
        registry_key: String,
    },
    UntrackedConfigBlob {
        registry_key: String,
    },
    UnknownStrategy {
        registry_key: String,
    },
    InvalidStrategyRegistryKey {
        registry_key: String,
    },
    UnknownStrategyParameter {
        registry_key: String,
        parameter: String,
    },
    InvalidStartingBalance {
        balance: String,
    },
    InvalidInstrumentId {
        instrument_id: String,
    },
    UnacceptedData {
        manifest_proof: String,
        accepted_proof: String,
    },
    BindingMismatch {
        manifest_binding: String,
        accepted_binding: String,
    },
    FixtureMismatch {
        manifest_fixture: MarketStructureFixture,
        accepted_fixture: FixtureType,
    },
    DataTypeFidelityMismatch {
        data_type: String,
        fidelity_class: SourceProofFidelityClass,
    },
    NonLatestProofPinForNormalRun,
    UnsupportedDataType {
        data_type: String,
    },
    UnsupportedEnum {
        field: &'static str,
        value: String,
    },
    OutputPrefixOutsideArtifactRoot,
    NegativeTime {
        field: &'static str,
        value: i64,
    },
    InvertedTimeWindow {
        start: i64,
        end: i64,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::InlineStrategyCode { registry_key } => {
                write!(
                    f,
                    "inline strategy code is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::PythonStrategyPath { registry_key } => {
                write!(
                    f,
                    "Python strategy path is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::UntrackedConfigBlob { registry_key } => {
                write!(
                    f,
                    "untracked config blob is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::UnknownStrategy { registry_key } => {
                write!(
                    f,
                    "strategy {registry_key:?} is not a registered compiled Rust strategy"
                )
            }
            Self::InvalidStrategyRegistryKey { registry_key } => {
                write!(
                    f,
                    "strategy registry key must contain only ASCII letters, digits, '_' or '-': {registry_key:?}"
                )
            }
            Self::UnknownStrategyParameter {
                registry_key,
                parameter,
            } => write!(
                f,
                "parameter {parameter:?} is not accepted for strategy {registry_key:?}"
            ),
            Self::InvalidStartingBalance { balance } => {
                write!(f, "invalid starting balance: {balance:?}")
            }
            Self::InvalidInstrumentId { instrument_id } => {
                write!(f, "invalid instrument id: {instrument_id:?}")
            }
            Self::UnacceptedData {
                manifest_proof,
                accepted_proof,
            } => write!(
                f,
                "manifest source proof {manifest_proof:?} does not match accepted dataset proof {accepted_proof:?}"
            ),
            Self::BindingMismatch {
                manifest_binding,
                accepted_binding,
            } => write!(
                f,
                "manifest venue_binding_key {manifest_binding:?} does not match accepted source binding {accepted_binding:?}"
            ),
            Self::FixtureMismatch {
                manifest_fixture,
                accepted_fixture,
            } => write!(
                f,
                "manifest market_structure_fixture {manifest_fixture:?} does not match accepted.fixture_type {accepted_fixture:?}"
            ),
            Self::DataTypeFidelityMismatch {
                data_type,
                fidelity_class,
            } => write!(
                f,
                "catalog data type {data_type:?} is incompatible with accepted fidelity {fidelity_class:?}"
            ),
            Self::NonLatestProofPinForNormalRun => {
                write!(f, "normal runs cannot pin a non-latest source proof")
            }
            Self::UnsupportedDataType { data_type } => {
                write!(f, "unsupported catalog data type: {data_type:?}")
            }
            Self::UnsupportedEnum { field, value } => {
                write!(f, "unsupported value {value:?} for {field}")
            }
            Self::OutputPrefixOutsideArtifactRoot => {
                write!(f, "output prefix must live under artifact_root/backtests/")
            }
            Self::NegativeTime { field, value } => {
                write!(f, "{field} must not be negative: {value}")
            }
            Self::InvertedTimeWindow { start, end } => {
                write!(f, "start_time {start} must not be after end_time {end}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

fn validate_strategy_source(strategy: &StrategySource) -> Result<(), ManifestError> {
    let key = strategy.registry_key.trim();
    if key.is_empty() {
        return Err(ManifestError::MissingField("strategy.registry_key"));
    }
    // Reject executable code masquerading as a key.
    if key.contains(['{', '}', ';', '\n', '(', ')']) || key.contains("fn ") {
        return Err(ManifestError::InlineStrategyCode {
            registry_key: key.to_string(),
        });
    }
    // Reject Python runtime paths.
    if key.ends_with(".py") || key.contains(".py:") {
        return Err(ManifestError::PythonStrategyPath {
            registry_key: key.to_string(),
        });
    }
    // Reject untracked config blobs / filesystem paths; registry keys are bare.
    if key.contains('/') || key.contains('\\') || key.contains("..") || key.ends_with(".toml") {
        return Err(ManifestError::UntrackedConfigBlob {
            registry_key: key.to_string(),
        });
    }
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ManifestError::InvalidStrategyRegistryKey {
            registry_key: key.to_string(),
        });
    }
    if !registered_strategies().contains(&key) {
        return Err(ManifestError::UnknownStrategy {
            registry_key: key.to_string(),
        });
    }
    let allowed_parameters =
        registered_strategy_parameters(key).ok_or_else(|| ManifestError::UnknownStrategy {
            registry_key: key.to_string(),
        })?;
    for parameter in strategy.parameters.keys() {
        if !allowed_parameters.contains(&parameter.as_str()) {
            return Err(ManifestError::UnknownStrategyParameter {
                registry_key: key.to_string(),
                parameter: parameter.clone(),
            });
        }
    }
    match key {
        STRATEGY_HURST_VPIN_DIRECTIONAL => {
            for parameter in [STRATEGY_PARAM_BAR_TYPE, STRATEGY_PARAM_TRADE_SIZE] {
                if !strategy.parameters.contains_key(parameter) {
                    return Err(ManifestError::MissingField(match parameter {
                        STRATEGY_PARAM_BAR_TYPE => "strategy.parameters.bar_type",
                        STRATEGY_PARAM_TRADE_SIZE => "strategy.parameters.trade_size",
                        _ => unreachable!(),
                    }));
                }
            }
            let trade_size = strategy
                .parameters
                .get(STRATEGY_PARAM_TRADE_SIZE)
                .expect("presence checked above");
            Quantity::from_str(trade_size)
                .map_err(|_| ManifestError::MissingField("strategy.parameters.trade_size"))?;
            let bar_type = strategy
                .parameters
                .get(STRATEGY_PARAM_BAR_TYPE)
                .expect("presence checked above");
            bar_type
                .parse::<BarType>()
                .map_err(|_| ManifestError::MissingField("strategy.parameters.bar_type"))?;
        }
        _ => unreachable!("registered strategy was already matched"),
    }
    Ok(())
}

fn validate_starting_balances(balances: &[String]) -> Result<(), ManifestError> {
    if balances.is_empty() {
        return Err(ManifestError::MissingField("venue.starting_balances"));
    }
    for balance in balances {
        Money::from_str(balance).map_err(|_| ManifestError::InvalidStartingBalance {
            balance: balance.clone(),
        })?;
    }
    Ok(())
}

impl BacktestingRunManifest {
    /// Deterministic SHA-256 over every typed manifest field that affects a run.
    #[must_use]
    pub fn manifest_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"backtesting-run-manifest.v1");
        hash_str(&mut hasher, "run_id", &self.run_id);
        hash_str(
            &mut hasher,
            "market_structure_fixture",
            market_structure_fixture_label(self.market_structure_fixture),
        );
        hash_str(&mut hasher, "venue_binding_key", &self.venue_binding_key);
        hash_str(
            &mut hasher,
            "run_purpose",
            run_purpose_label(self.run_purpose),
        );
        hash_str(&mut hasher, "source_proof_id", &self.source_proof_id);
        hash_u32(
            &mut hasher,
            "source_proof_version",
            self.source_proof_version,
        );
        hash_bool(
            &mut hasher,
            "pins_non_latest_proof",
            self.pins_non_latest_proof,
        );
        hash_str(
            &mut hasher,
            "strategy.registry_key",
            &self.strategy.registry_key,
        );
        for (key, value) in &self.strategy.parameters {
            hash_str(&mut hasher, "strategy.parameters.key", key);
            hash_str(&mut hasher, "strategy.parameters.value", value);
        }
        hash_str(&mut hasher, "venue.nt_venue", &self.venue.nt_venue);
        hash_str(&mut hasher, "venue.oms_type", &self.venue.oms_type);
        hash_str(&mut hasher, "venue.account_type", &self.venue.account_type);
        hash_str(&mut hasher, "venue.book_type", &self.venue.book_type);
        for balance in &self.venue.starting_balances {
            hash_str(&mut hasher, "venue.starting_balances", balance);
        }
        hash_str(
            &mut hasher,
            "catalog_input.catalog_path",
            &self.catalog_input.catalog_path,
        );
        hash_str(
            &mut hasher,
            "catalog_input.data_type",
            &self.catalog_input.data_type,
        );
        hash_str(
            &mut hasher,
            "catalog_input.nt_instrument_id",
            &self.catalog_input.nt_instrument_id,
        );
        hash_str(&mut hasher, "artifact_root", &self.artifact_root);
        hash_str(&mut hasher, "output_prefix", &self.output_prefix);
        hash_i64_opt(&mut hasher, "start_time", self.start_time);
        hash_i64_opt(&mut hasher, "end_time", self.end_time);
        hex::encode(hasher.finalize())
    }

    /// Validate the manifest against gate-4 rules and bind it to an accepted
    /// dataset (the only admissible data source).
    ///
    /// # Errors
    ///
    /// Returns the first blocking [`ManifestError`].
    pub fn validate(&self, accepted: &AcceptedDataset) -> Result<(), ManifestError> {
        for (name, value) in [
            ("run_id", self.run_id.as_str()),
            ("venue_binding_key", self.venue_binding_key.as_str()),
            ("source_proof_id", self.source_proof_id.as_str()),
            ("artifact_root", self.artifact_root.as_str()),
            ("output_prefix", self.output_prefix.as_str()),
            ("venue.nt_venue", self.venue.nt_venue.as_str()),
            (
                "catalog_input.catalog_path",
                self.catalog_input.catalog_path.as_str(),
            ),
            (
                "catalog_input.nt_instrument_id",
                self.catalog_input.nt_instrument_id.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ManifestError::MissingField(name));
            }
        }
        ensure_supported_enums(self)?;
        ensure_supported_data_type(&self.catalog_input.data_type)?;
        ensure_data_type_matches_fidelity(&self.catalog_input.data_type, accepted.fidelity_class)?;
        validate_strategy_source(&self.strategy)?;
        validate_starting_balances(&self.venue.starting_balances)?;

        // Data must be accepted: the only admissible input is the accepted
        // dataset, matched by source proof id and version.
        if self.source_proof_id != accepted.source_proof_id
            || self.source_proof_version != accepted.source_proof_version
        {
            return Err(ManifestError::UnacceptedData {
                manifest_proof: format!("{}@{}", self.source_proof_id, self.source_proof_version),
                accepted_proof: format!(
                    "{}@{}",
                    accepted.source_proof_id, accepted.source_proof_version
                ),
            });
        }
        if self.venue_binding_key != accepted.source_binding {
            return Err(ManifestError::BindingMismatch {
                manifest_binding: self.venue_binding_key.clone(),
                accepted_binding: accepted.source_binding.clone(),
            });
        }
        if !market_structure_fixture_matches_source_fixture(
            self.market_structure_fixture,
            accepted.fixture_type,
        ) {
            return Err(ManifestError::FixtureMismatch {
                manifest_fixture: self.market_structure_fixture,
                accepted_fixture: accepted.fixture_type,
            });
        }
        if self.run_purpose == RunPurpose::Normal && self.pins_non_latest_proof {
            return Err(ManifestError::NonLatestProofPinForNormalRun);
        }
        for (field, value) in [("start_time", self.start_time), ("end_time", self.end_time)] {
            if let Some(value) = value
                && value < 0
            {
                return Err(ManifestError::NegativeTime { field, value });
            }
        }
        if let (Some(start), Some(end)) = (self.start_time, self.end_time)
            && start > end
        {
            return Err(ManifestError::InvertedTimeWindow { start, end });
        }
        if !self.output_prefix.starts_with(&format!(
            "{}/backtests/",
            self.artifact_root.trim_end_matches('/')
        )) {
            return Err(ManifestError::OutputPrefixOutsideArtifactRoot);
        }
        Ok(())
    }

    /// Map the venue settings into a NautilusTrader [`BacktestVenueConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if an enum value is unsupported.
    pub fn to_nt_venue_config(&self) -> Result<BacktestVenueConfig, ManifestError> {
        Ok(BacktestVenueConfig::builder()
            .name(Ustr::from(&self.venue.nt_venue))
            .oms_type(parse_oms_type(&self.venue.oms_type)?)
            .account_type(parse_account_type(&self.venue.account_type)?)
            .book_type(parse_book_type(&self.venue.book_type)?)
            .starting_balances(self.venue.starting_balances.clone())
            .build())
    }

    /// Map the catalog input into a NautilusTrader [`BacktestDataConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if the data type or instrument id is unsupported.
    pub fn to_nt_data_config(&self) -> Result<BacktestDataConfig, ManifestError> {
        let data_type = match self.catalog_input.data_type.as_str() {
            "TradeTick" => NautilusDataType::TradeTick,
            other => {
                return Err(ManifestError::UnsupportedDataType {
                    data_type: other.to_string(),
                });
            }
        };
        let instrument_id = self
            .catalog_input
            .nt_instrument_id
            .parse::<InstrumentId>()
            .map_err(|_| ManifestError::InvalidInstrumentId {
                instrument_id: self.catalog_input.nt_instrument_id.clone(),
            })?;
        Ok(BacktestDataConfig::builder()
            .data_type(data_type)
            .catalog_path(self.catalog_input.catalog_path.clone())
            .instrument_id(instrument_id)
            .build())
    }

    /// Map the manifest into a NautilusTrader [`BacktestRunConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if venue or data mapping fails.
    pub fn to_nt_run_config(&self) -> Result<BacktestRunConfig, ManifestError> {
        let venue = self.to_nt_venue_config()?;
        let data = self.to_nt_data_config()?;
        let to_nanos = |field: &'static str, value: i64| -> Result<UnixNanos, ManifestError> {
            u64::try_from(value)
                .map(UnixNanos::from)
                .map_err(|_| ManifestError::NegativeTime { field, value })
        };
        let start = self
            .start_time
            .map(|value| to_nanos("start_time", value))
            .transpose()?;
        let end = self
            .end_time
            .map(|value| to_nanos("end_time", value))
            .transpose()?;
        Ok(BacktestRunConfig::builder()
            .id(self.run_id.clone())
            .venues(vec![venue])
            .data(vec![data])
            .maybe_start(start)
            .maybe_end(end)
            .build())
    }
}

fn hash_str(hasher: &mut Sha256, field: &str, value: &str) {
    hasher.update([0]);
    hasher.update(field.as_bytes());
    hasher.update([1]);
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_bool(hasher: &mut Sha256, field: &str, value: bool) {
    hash_str(hasher, field, if value { "true" } else { "false" });
}

fn hash_u32(hasher: &mut Sha256, field: &str, value: u32) {
    hasher.update([0]);
    hasher.update(field.as_bytes());
    hasher.update([2]);
    hasher.update(value.to_le_bytes());
}

fn hash_i64_opt(hasher: &mut Sha256, field: &str, value: Option<i64>) {
    hasher.update([0]);
    hasher.update(field.as_bytes());
    match value {
        Some(value) => {
            hasher.update([3]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([4]),
    }
}

fn market_structure_fixture_label(fixture: MarketStructureFixture) -> &'static str {
    match fixture {
        MarketStructureFixture::BinaryOption => "binary-option",
        MarketStructureFixture::PerpsSpot => "perps-spot",
    }
}

fn run_purpose_label(purpose: RunPurpose) -> &'static str {
    match purpose {
        RunPurpose::Normal => "normal",
        RunPurpose::Reproduction => "reproduction",
        RunPurpose::Audit => "audit",
        RunPurpose::Regression => "regression",
        RunPurpose::Migration => "migration",
    }
}

fn ensure_supported_enums(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    parse_oms_type(&manifest.venue.oms_type)?;
    parse_account_type(&manifest.venue.account_type)?;
    parse_book_type(&manifest.venue.book_type)?;
    Ok(())
}

fn ensure_supported_data_type(value: &str) -> Result<(), ManifestError> {
    match value {
        "TradeTick" => Ok(()),
        other => Err(ManifestError::UnsupportedDataType {
            data_type: other.to_string(),
        }),
    }
}

fn ensure_data_type_matches_fidelity(
    data_type: &str,
    fidelity_class: SourceProofFidelityClass,
) -> Result<(), ManifestError> {
    match (data_type, fidelity_class) {
        ("TradeTick", SourceProofFidelityClass::TradeReplay) => Ok(()),
        (data_type, fidelity_class) => Err(ManifestError::DataTypeFidelityMismatch {
            data_type: data_type.to_string(),
            fidelity_class,
        }),
    }
}

fn market_structure_fixture_matches_source_fixture(
    manifest_fixture: MarketStructureFixture,
    accepted_fixture: FixtureType,
) -> bool {
    matches!(
        (manifest_fixture, accepted_fixture),
        (
            MarketStructureFixture::BinaryOption,
            FixtureType::PredictionMarket
        ) | (MarketStructureFixture::PerpsSpot, FixtureType::PerpsSpot)
    )
}

fn parse_oms_type(value: &str) -> Result<OmsType, ManifestError> {
    match value {
        "NETTING" => Ok(OmsType::Netting),
        "HEDGING" => Ok(OmsType::Hedging),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.oms_type",
            value: other.to_string(),
        }),
    }
}

fn parse_account_type(value: &str) -> Result<AccountType, ManifestError> {
    match value {
        "CASH" => Ok(AccountType::Cash),
        "MARGIN" => Ok(AccountType::Margin),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.account_type",
            value: other.to_string(),
        }),
    }
}

fn parse_book_type(value: &str) -> Result<BookType, ManifestError> {
    match value {
        "L1_MBP" => Ok(BookType::L1_MBP),
        "L2_MBP" => Ok(BookType::L2_MBP),
        "L3_MBO" => Ok(BookType::L3_MBO),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.book_type",
            value: other.to_string(),
        }),
    }
}

/// Build the typed manifest from TOML text.
///
/// # Errors
///
/// Returns an error if the TOML cannot be parsed into the manifest schema.
pub fn parse_manifest_toml(text: &str) -> Result<BacktestingRunManifest> {
    let manifest: BacktestingRunManifest =
        toml::from_str(text).map_err(|error| anyhow::anyhow!("invalid manifest TOML: {error}"))?;
    if manifest.run_id.trim().is_empty() {
        bail!("manifest run_id must not be empty");
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_proof::{
        AcceptanceMode, EvidenceState, FixtureType, IngestManifestObjectRecord, NtMappingStatus,
        RequiredCheck, RequiredChecks, SourceProofFidelityClass, SourceProofReport,
        SourceProofStatus, TimeRange, select_accepted_dataset,
    };

    fn accepted_dataset() -> AcceptedDataset {
        let checks = RequiredChecks {
            source_access: RequiredCheck::passed("m"),
            license: RequiredCheck::passed("m"),
            schema: RequiredCheck::passed("m"),
            time_semantics: RequiredCheck::passed("m"),
            instrument_universe: RequiredCheck::passed("m"),
            coverage: RequiredCheck::passed("m"),
            granularity: RequiredCheck::passed("m"),
            completeness: RequiredCheck::passed("m"),
            nt_mapping: RequiredCheck::passed("m"),
            storage: RequiredCheck::passed("m"),
        };
        let object = IngestManifestObjectRecord {
            s3_uri: "s3://bolt-parquet/.../object=d6af93.csv.gz".to_string(),
            source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz"
                .to_string(),
            sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            bytes: 8505,
            archive_date: "2026-03-01".to_string(),
            schema_columns: vec!["id".to_string()],
        };
        let proof = SourceProofReport {
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            contract_version: "backfill-table-contract.v1".to_string(),
            schema_version: "backfill-source-proof.v1".to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "bybit-spot-tick-trades".to_string(),
            venue: "bybit".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            table_family: "trades".to_string(),
            evidence_state: EvidenceState::OwnerArchiveBackfillable,
            fixture_type: FixtureType::PerpsSpot,
            requested_time_range: TimeRange {
                start_utc: "2025-06-01T00:00:00Z".to_string(),
                end_utc: "2026-06-01T00:00:00Z".to_string(),
            },
            coverage_time_range: TimeRange {
                start_utc: "2026-03-01T00:00:00Z".to_string(),
                end_utc: "2026-03-02T00:00:00Z".to_string(),
            },
            instrument_universe_id: "u".to_string(),
            raw_sample_uri: object.s3_uri.clone(),
            raw_sample_hash: object.sha256.clone(),
            schema_sample_uri: "s".to_string(),
            schema_sample_hash: "h".to_string(),
            license_ref: "l".to_string(),
            retention_ref: "r".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            gap_policy_id: String::new(),
            required_checks: checks,
            acceptance_mode: None,
            accepted_by: None,
            accepted_at: None,
            supersedes_source_proof_id: None,
        }
        .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
        .unwrap();
        select_accepted_dataset(&proof, &object, &object.sha256).unwrap()
    }

    fn valid_manifest() -> BacktestingRunManifest {
        BacktestingRunManifest {
            run_id: "backtesting-vertical-slice-bnbusdc-2026-03-01".to_string(),
            market_structure_fixture: MarketStructureFixture::PerpsSpot,
            venue_binding_key: "bybit-spot-tick-trades".to_string(),
            run_purpose: RunPurpose::Normal,
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            pins_non_latest_proof: false,
            strategy: StrategySource {
                registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
                parameters: BTreeMap::from([
                    ("trade_size".to_string(), "0.01".to_string()),
                    (
                        "bar_type".to_string(),
                        "BNBUSDC.BYBIT-1-MINUTE-LAST-INTERNAL".to_string(),
                    ),
                ]),
            },
            venue: ManifestVenueConfig {
                nt_venue: "BYBIT".to_string(),
                oms_type: "NETTING".to_string(),
                account_type: "CASH".to_string(),
                book_type: "L1_MBP".to_string(),
                starting_balances: vec!["1_000_000 USDC".to_string()],
            },
            catalog_input: ManifestCatalogInput {
                catalog_path: "/tmp/catalog".to_string(),
                data_type: "TradeTick".to_string(),
                nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
            },
            artifact_root: "s3://bolt-parquet/nt-research-analytics".to_string(),
            output_prefix: "s3://bolt-parquet/nt-research-analytics/backtests/bnbusdc".to_string(),
            start_time: None,
            end_time: None,
        }
    }

    #[test]
    fn valid_manifest_passes_and_maps_to_nt_configs() {
        let manifest = valid_manifest();
        manifest.validate(&accepted_dataset()).expect("valid");
        let run = manifest.to_nt_run_config().expect("run config");
        assert_eq!(run.id(), "backtesting-vertical-slice-bnbusdc-2026-03-01");
        assert_eq!(run.venues().len(), 1);
        assert_eq!(run.data().len(), 1);
    }

    #[test]
    fn rejects_empty_starting_balances() {
        let mut manifest = valid_manifest();
        manifest.venue.starting_balances.clear();
        assert!(matches!(
            manifest.validate(&accepted_dataset()),
            Err(ManifestError::MissingField("venue.starting_balances"))
        ));
    }

    #[test]
    fn rejects_malformed_starting_balance() {
        let mut manifest = valid_manifest();
        manifest.venue.starting_balances = vec!["not money".to_string()];
        assert!(matches!(
            manifest.validate(&accepted_dataset()),
            Err(ManifestError::InvalidStartingBalance { balance }) if balance == "not money"
        ));
    }

    #[test]
    fn manifest_hash_is_stable_and_content_sensitive() {
        let manifest = valid_manifest();
        let round_tripped: BacktestingRunManifest =
            toml::from_str(&toml::to_string(&manifest).expect("serialize")).expect("parse");
        assert_eq!(manifest.manifest_hash(), round_tripped.manifest_hash());

        let mut changed = manifest.clone();
        changed.start_time = Some(1_772_323_200_000_000_000);
        assert_ne!(manifest.manifest_hash(), changed.manifest_hash());
    }

    #[test]
    fn rejects_inline_strategy_code() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "fn on_trade(&mut self) { submit(); }".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InlineStrategyCode { .. }
        ));
    }

    #[test]
    fn rejects_python_strategy_path() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "strategies/my_strategy.py".to_string();
        // Filesystem path is caught first as a Python path.
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::PythonStrategyPath { .. }
        ));
    }

    #[test]
    fn rejects_untracked_config_blob() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "../untracked/config.toml".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UntrackedConfigBlob { .. }
        ));
    }

    #[test]
    fn rejects_unknown_strategy() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "nonexistent_strategy".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnknownStrategy { .. }
        ));
    }

    #[test]
    fn rejects_invalid_strategy_registry_key_syntax() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "use process Command".to_string();
        let err = manifest.validate(&accepted_dataset()).unwrap_err();
        assert!(err.to_string().contains("registry key"), "{err}");
    }

    #[test]
    fn rejects_unknown_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest
            .strategy
            .parameters
            .insert("unknown_blob".to_string(), "x".to_string());
        let err = manifest.validate(&accepted_dataset()).unwrap_err();
        assert!(err.to_string().contains("parameter"), "{err}");
    }

    #[test]
    fn rejects_missing_required_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest
            .strategy
            .parameters
            .remove(STRATEGY_PARAM_TRADE_SIZE);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.trade_size")
        );
    }

    #[test]
    fn rejects_invalid_trade_size_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_TRADE_SIZE.to_string(),
            "not-a-size".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.trade_size")
        );
    }

    #[test]
    fn rejects_invalid_bar_type_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_BAR_TYPE.to_string(),
            "not-a-bar-type".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.bar_type")
        );
    }

    #[test]
    fn rejects_unaccepted_data() {
        let mut manifest = valid_manifest();
        manifest.source_proof_id = "some-other-proof".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnacceptedData { .. }
        ));
    }

    #[test]
    fn rejects_source_binding_mismatch() {
        let mut manifest = valid_manifest();
        manifest.venue_binding_key = "some-other-binding".to_string();
        let err = manifest.validate(&accepted_dataset()).unwrap_err();
        assert!(err.to_string().contains("binding"), "{err}");
    }

    #[test]
    fn rejects_fixture_mismatch() {
        let mut manifest = valid_manifest();
        manifest.market_structure_fixture = MarketStructureFixture::BinaryOption;
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::FixtureMismatch {
                manifest_fixture: MarketStructureFixture::BinaryOption,
                accepted_fixture: FixtureType::PerpsSpot,
            }
        );
    }

    #[test]
    fn rejects_non_latest_proof_pin_for_normal_run() {
        let mut manifest = valid_manifest();
        manifest.pins_non_latest_proof = true;
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NonLatestProofPinForNormalRun
        );
    }

    #[test]
    fn rejects_output_prefix_outside_artifact_root() {
        let mut manifest = valid_manifest();
        manifest.output_prefix = "s3://other-bucket/backtests/x".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::OutputPrefixOutsideArtifactRoot
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let manifest = valid_manifest();
        let text = toml::to_string(&manifest).expect("serialize");
        let parsed = parse_manifest_toml(&text).expect("parse");
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn rejects_negative_start_time() {
        let mut manifest = valid_manifest();
        manifest.start_time = Some(-1);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NegativeTime {
                field: "start_time",
                value: -1,
            }
        );
    }

    #[test]
    fn rejects_negative_end_time() {
        let mut manifest = valid_manifest();
        manifest.end_time = Some(-42);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NegativeTime {
                field: "end_time",
                value: -42,
            }
        );
    }

    #[test]
    fn rejects_start_after_end() {
        let mut manifest = valid_manifest();
        manifest.start_time = Some(200);
        manifest.end_time = Some(100);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvertedTimeWindow {
                start: 200,
                end: 100,
            }
        );
    }

    #[test]
    fn rejects_unsupported_data_type() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.data_type = "QuoteTick".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedDataType {
                data_type: "QuoteTick".to_string(),
            }
        );
    }

    #[test]
    fn rejects_fidelity_data_type_mismatch() {
        let manifest = valid_manifest();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;
        let err = manifest.validate(&accepted).unwrap_err();
        assert!(err.to_string().contains("incompatible"), "{err}");
    }

    #[test]
    fn rejects_invalid_instrument_id_with_specific_error() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.nt_instrument_id = "not-an-instrument-id".to_string();
        assert!(matches!(
            manifest.to_nt_data_config(),
            Err(ManifestError::InvalidInstrumentId { instrument_id })
                if instrument_id == "not-an-instrument-id"
        ));
    }

    #[test]
    fn rejects_unsupported_oms_type() {
        let mut manifest = valid_manifest();
        manifest.venue.oms_type = "INVALID".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.oms_type",
                value: "INVALID".to_string(),
            }
        );
    }

    #[test]
    fn rejects_malformed_manifest_toml() {
        assert!(parse_manifest_toml("this is not = valid = toml").is_err());
    }

    #[test]
    fn rejects_unknown_top_level_manifest_toml_fields() {
        let text = format!(
            "unknown_blob = \"x\"\n{}",
            toml::to_string(&valid_manifest()).expect("serialize")
        );
        assert!(parse_manifest_toml(&text).is_err());
    }

    #[test]
    fn rejects_unknown_nested_manifest_toml_fields() {
        let text = toml::to_string(&valid_manifest())
            .expect("serialize")
            .replace("[strategy]\n", "[strategy]\nunknown_blob = \"x\"\n");
        assert!(parse_manifest_toml(&text).is_err());
    }
}
