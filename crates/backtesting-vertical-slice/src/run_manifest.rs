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
    enums::{AccountType, BookType, OmsType, OtoTriggerMode},
    identifiers::InstrumentId,
    types::{Currency, Money, Quantity},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use super::source_proof::{AcceptedDataset, SourceProofFidelityClass};

/// Registry key for the compiled Rust trade-driven example strategy.
pub const STRATEGY_HURST_VPIN_DIRECTIONAL: &str = "hurst_vpin_directional";
/// Strategy parameter key for the bar type.
pub const STRATEGY_PARAM_BAR_TYPE: &str = "bar_type";
/// Strategy parameter key for the trade size.
pub const STRATEGY_PARAM_TRADE_SIZE: &str = "trade_size";
/// Explicit manifest value for no catalog filesystem protocol.
pub const CATALOG_FS_PROTOCOL_NONE: &str = "NONE";
/// NT venue-model surfaces declared in TOML but rejected until typed mappings exist.
pub const UNSUPPORTED_NT_VENUE_SURFACES: &[&str] = &[
    "leverages",
    "margin_model",
    "modules",
    "fill_model",
    "latency_model",
    "fee_model",
    "settlement_prices",
];

const CATALOG_STORAGE_OPTIONS_SHADOWED: &str =
    "cannot be combined with catalog_fs_rust_storage_options";
const S3_OPTION_CONDITIONAL_PUT: &str = "conditional_put";
const S3_CONDITIONAL_PUT_ETAG: &str = "etag";
const S3_CONDITIONAL_PUT_DISABLED: &str = "disabled";

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

/// Structured reason for pinning a non-latest accepted source proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofPinReasonCode {
    BaselineReproduction,
    PublishedResultReproduction,
    RegressionComparison,
    AuditOrInvestigation,
    MigrationValidation,
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
    /// NautilusTrader venue name.
    pub nt_venue: String,
    /// One of `NETTING` or `HEDGING`.
    pub oms_type: String,
    /// One of `CASH` or `MARGIN`.
    pub account_type: String,
    /// One of `L1_MBP`, `L2_MBP`, `L3_MBO`.
    pub book_type: String,
    /// Starting balances such as `["1_000_000 USDC"]`.
    pub starting_balances: Vec<String>,
    /// If multi-venue routing should be enabled for the execution client.
    pub routing: bool,
    /// If the account for this exchange is frozen.
    pub frozen_account: bool,
    /// If stop orders are rejected when trigger price is in the market.
    pub reject_stop_orders: bool,
    /// If GTD time-in-force orders are supported by the venue.
    pub support_gtd_orders: bool,
    /// If contingent orders are supported/respected by the venue.
    pub support_contingent_orders: bool,
    /// If venue position IDs are generated on fills.
    pub use_position_ids: bool,
    /// If venue order IDs and position IDs are random UUID4s.
    pub use_random_ids: bool,
    /// If reduce-only execution instructions are honored.
    pub use_reduce_only: bool,
    /// If bars should be processed by the matching engine.
    pub bar_execution: bool,
    /// If bar high/low ordering should use NT's adaptive heuristic.
    pub bar_adaptive_high_low_ordering: bool,
    /// If trades should be processed by the matching engine.
    pub trade_execution: bool,
    /// If market orders should emit `OrderAccepted` events.
    pub use_market_order_acks: bool,
    /// If order book liquidity consumption should be tracked per level.
    pub liquidity_consumption: bool,
    /// If negative cash balances are allowed.
    pub allow_cash_borrowing: bool,
    /// If limit order queue-position tracking is enabled.
    pub queue_position: bool,
    /// One of `PARTIAL` or `FULL`.
    pub oto_trigger_mode: String,
    /// Account base currency, or `NONE` for a multi-currency account.
    pub base_currency: String,
    /// Account default leverage as a decimal string.
    pub default_leverage: String,
    /// Exchange-calculated market-order price protection boundary in points.
    pub price_protection_points: u32,
    /// NT per-instrument leverage map. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leverages: Option<BTreeMap<String, String>>,
    /// NT margin model selector. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_model: Option<String>,
    /// NT simulation module selectors. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules: Option<Vec<String>>,
    /// NT fill model selector. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_model: Option<String>,
    /// NT latency model selector. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_model: Option<String>,
    /// NT fee model selector. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_model: Option<String>,
    /// NT settlement prices keyed by instrument id. Unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_prices: Option<BTreeMap<String, String>>,
}

/// Catalog input mapped into [`BacktestDataConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCatalogInput {
    pub catalog_path: String,
    /// Catalog filesystem protocol, or `NONE` when `catalog_path` is already complete.
    pub catalog_fs_protocol: String,
    /// NT filesystem storage options for Python/fsspec-compatible paths.
    pub catalog_fs_storage_options: BTreeMap<String, String>,
    /// NT Rust object-store options for cloud-backed catalog paths.
    pub catalog_fs_rust_storage_options: BTreeMap<String, String>,
    /// NautilusTrader data type, currently `TradeTick`.
    pub data_type: String,
    /// NautilusTrader instrument id, such as `SYMBOL.VENUE`.
    pub nt_instrument_id: String,
}

/// Artifact output store options used for publishing and published-catalog proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestArtifactStore {
    /// NT/object-store options for Python/fsspec-compatible artifact writes.
    pub storage_options: BTreeMap<String, String>,
    /// NT/object-store options for the Rust cloud-backed artifact path.
    pub rust_storage_options: BTreeMap<String, String>,
    /// SSM parameters resolved into the Rust object-store options at runtime.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ssm_parameters: Option<ManifestArtifactStoreSsmParameters>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactSubpath {
    Raw,
    NtCatalog,
    SourceProofs,
    Backtests,
    ArtifactIndex,
    ResearchAnalytics,
}

impl ArtifactSubpath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::NtCatalog => "nt-catalog",
            Self::SourceProofs => "source-proofs",
            Self::Backtests => "backtests",
            Self::ArtifactIndex => "artifact-index",
            Self::ResearchAnalytics => "research-analytics",
        }
    }
}

/// SSM parameter paths for artifact-store credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestArtifactStoreSsmParameters {
    /// AWS region used when resolving the configured SSM parameters.
    pub region: String,
    /// SSM parameter path whose decrypted value is the S3 access key id.
    pub access_key_id: String,
    /// SSM parameter path whose decrypted value is the S3 secret access key.
    pub secret_access_key: String,
    /// Optional SSM parameter path whose decrypted value is the S3 session token.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_token: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proof_pin_reason_code: Option<ProofPinReasonCode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proof_pin_reason_detail: Option<String>,
    pub strategy: StrategySource,
    pub venue: ManifestVenueConfig,
    pub catalog_input: ManifestCatalogInput,
    /// Configured S3 artifact root (TOML/config-owned).
    pub artifact_root: String,
    /// Output prefix under `artifact_root/backtests/`.
    pub output_prefix: String,
    /// Artifact-store options for output publication and direct catalog proof.
    pub artifact_store: ManifestArtifactStore,
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
    InvalidBaseCurrency {
        currency: String,
    },
    InvalidDefaultLeverage {
        leverage: String,
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
    UnsupportedNtSurface {
        field: &'static str,
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
    RawArtifactStoreCredential {
        field: &'static str,
        key: String,
    },
    ArtifactStoreS3CredentialsNotResolved,
    InvalidArtifactStoreSsmParameter {
        field: &'static str,
    },
    ArtifactStoreSecretResolution {
        field: &'static str,
        source: String,
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
            Self::InvalidBaseCurrency { currency } => {
                write!(f, "invalid base currency: {currency:?}")
            }
            Self::InvalidDefaultLeverage { leverage } => {
                write!(f, "invalid default leverage: {leverage:?}")
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
            Self::UnsupportedNtSurface { field } => write!(
                f,
                "unsupported NT venue surface {field}: add typed BacktestVenueConfig mapping before use"
            ),
            Self::OutputPrefixOutsideArtifactRoot => {
                write!(f, "output prefix must live under artifact_root/backtests/")
            }
            Self::NegativeTime { field, value } => {
                write!(f, "{field} must not be negative: {value}")
            }
            Self::InvertedTimeWindow { start, end } => {
                write!(f, "start_time {start} must not be after end_time {end}")
            }
            Self::RawArtifactStoreCredential { field, key } => write!(
                f,
                "{field}.{key} contains artifact_store credential material; configure artifact_store.ssm_parameters and resolve through SSM"
            ),
            Self::ArtifactStoreS3CredentialsNotResolved => write!(
                f,
                "artifact_store.ssm_parameters must resolve access_key_id and secret_access_key before publishing to an s3 output_prefix"
            ),
            Self::InvalidArtifactStoreSsmParameter { field } => write!(
                f,
                "artifact_store.ssm_parameters.{field} must be an absolute SSM parameter path without whitespace"
            ),
            Self::ArtifactStoreSecretResolution { field, source } => write!(
                f,
                "artifact_store.ssm_parameters.{field} SSM resolution failed: {source}"
            ),
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
        hasher.update(
            serde_json::to_vec(self)
                .expect("BacktestingRunManifest JSON serialization must be infallible"),
        );
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
            ("venue.oms_type", self.venue.oms_type.as_str()),
            ("venue.account_type", self.venue.account_type.as_str()),
            ("venue.book_type", self.venue.book_type.as_str()),
            (
                "venue.oto_trigger_mode",
                self.venue.oto_trigger_mode.as_str(),
            ),
            ("venue.base_currency", self.venue.base_currency.as_str()),
            (
                "venue.default_leverage",
                self.venue.default_leverage.as_str(),
            ),
            (
                "catalog_input.catalog_path",
                self.catalog_input.catalog_path.as_str(),
            ),
            (
                "catalog_input.catalog_fs_protocol",
                self.catalog_input.catalog_fs_protocol.as_str(),
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
        ensure_unsupported_nt_venue_surfaces_absent(&self.venue)?;
        ensure_supported_data_type(&self.catalog_input.data_type)?;
        let catalog_fs_protocol =
            parse_catalog_fs_protocol(&self.catalog_input.catalog_fs_protocol)?;
        validate_catalog_storage_options(
            catalog_fs_protocol.as_deref(),
            &self.catalog_input.catalog_fs_storage_options,
            &self.catalog_input.catalog_fs_rust_storage_options,
        )?;
        validate_catalog_storage_options(
            output_prefix_protocol(&self.output_prefix),
            &self.artifact_store.storage_options,
            &self.artifact_store.rust_storage_options,
        )?;
        validate_artifact_store_secrets(&self.artifact_store)?;
        ensure_artifact_store_conditional_put_enabled(
            output_prefix_protocol(&self.output_prefix),
            &self.artifact_store.storage_options,
            &self.artifact_store.rust_storage_options,
        )?;
        validate_artifact_root_protocol(&self.artifact_root)?;
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
        if self.run_purpose == RunPurpose::Normal && self.pins_non_latest_proof {
            return Err(ManifestError::NonLatestProofPinForNormalRun);
        }
        if self.pins_non_latest_proof && self.proof_pin_reason_code.is_none() {
            return Err(ManifestError::MissingField("proof_pin_reason_code"));
        }
        if self.proof_pin_reason_code == Some(ProofPinReasonCode::AuditOrInvestigation)
            && self
                .proof_pin_reason_detail
                .as_deref()
                .is_none_or(|detail| detail.trim().is_empty())
        {
            return Err(ManifestError::MissingField("proof_pin_reason_detail"));
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

    /// Resolve a typed artifact subpath under the configured artifact root.
    ///
    /// # Errors
    ///
    /// Returns an error when `artifact_root` uses an unsupported scheme.
    pub fn artifact_subpath_uri(&self, subpath: ArtifactSubpath) -> Result<String, ManifestError> {
        validate_artifact_root_protocol(&self.artifact_root)?;
        Ok(format!(
            "{}/{}",
            self.artifact_root.trim_end_matches('/'),
            subpath.as_str()
        ))
    }

    /// Map the venue settings into a NautilusTrader [`BacktestVenueConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if an enum value is unsupported.
    pub fn to_nt_venue_config(&self) -> Result<BacktestVenueConfig, ManifestError> {
        ensure_unsupported_nt_venue_surfaces_absent(&self.venue)?;
        Ok(BacktestVenueConfig::builder()
            .name(Ustr::from(&self.venue.nt_venue))
            .oms_type(parse_oms_type(&self.venue.oms_type)?)
            .account_type(parse_account_type(&self.venue.account_type)?)
            .book_type(parse_book_type(&self.venue.book_type)?)
            .starting_balances(self.venue.starting_balances.clone())
            .routing(self.venue.routing)
            .frozen_account(self.venue.frozen_account)
            .reject_stop_orders(self.venue.reject_stop_orders)
            .support_gtd_orders(self.venue.support_gtd_orders)
            .support_contingent_orders(self.venue.support_contingent_orders)
            .use_position_ids(self.venue.use_position_ids)
            .use_random_ids(self.venue.use_random_ids)
            .use_reduce_only(self.venue.use_reduce_only)
            .bar_execution(self.venue.bar_execution)
            .bar_adaptive_high_low_ordering(self.venue.bar_adaptive_high_low_ordering)
            .trade_execution(self.venue.trade_execution)
            .use_market_order_acks(self.venue.use_market_order_acks)
            .liquidity_consumption(self.venue.liquidity_consumption)
            .allow_cash_borrowing(self.venue.allow_cash_borrowing)
            .queue_position(self.venue.queue_position)
            .oto_trigger_mode(parse_oto_trigger_mode(&self.venue.oto_trigger_mode)?)
            .maybe_base_currency(parse_base_currency(&self.venue.base_currency)?)
            .default_leverage(parse_default_leverage(&self.venue.default_leverage)?)
            .price_protection_points(self.venue.price_protection_points)
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
        let catalog_fs_protocol =
            parse_catalog_fs_protocol(&self.catalog_input.catalog_fs_protocol)?;
        validate_catalog_storage_options(
            catalog_fs_protocol.as_deref(),
            &self.catalog_input.catalog_fs_storage_options,
            &self.catalog_input.catalog_fs_rust_storage_options,
        )?;
        Ok(BacktestDataConfig::builder()
            .data_type(data_type)
            .catalog_path(self.catalog_input.catalog_path.clone())
            .maybe_catalog_fs_protocol(catalog_fs_protocol)
            .maybe_catalog_fs_storage_options(
                if self.catalog_input.catalog_fs_storage_options.is_empty() {
                    None
                } else {
                    Some(
                        self.catalog_input
                            .catalog_fs_storage_options
                            .clone()
                            .into_iter()
                            .collect(),
                    )
                },
            )
            .maybe_catalog_fs_rust_storage_options(
                if self
                    .catalog_input
                    .catalog_fs_rust_storage_options
                    .is_empty()
                {
                    None
                } else {
                    Some(
                        self.catalog_input
                            .catalog_fs_rust_storage_options
                            .clone()
                            .into_iter()
                            .collect(),
                    )
                },
            )
            .instrument_id(instrument_id)
            .build())
    }

    /// Return the single effective artifact-store option map for publication.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured artifact-store options conflict with
    /// the output prefix protocol or use an unsupported S3 option key.
    pub fn artifact_store_storage_options(
        &self,
    ) -> Result<Option<BTreeMap<String, String>>, ManifestError> {
        if self.artifact_store.ssm_parameters.is_some() {
            return Err(ManifestError::UnsupportedEnum {
                field: "artifact_store.ssm_parameters",
                value: "requires SSM resolver".to_string(),
            });
        }
        let options = self
            .artifact_store_base_storage_options()?
            .unwrap_or_default();
        ensure_artifact_store_s3_credentials_resolved(
            output_prefix_protocol(&self.output_prefix),
            &options,
        )?;
        if options.is_empty() {
            Ok(None)
        } else {
            Ok(Some(options))
        }
    }

    pub fn artifact_store_storage_options_resolved<F>(
        &self,
        resolver: &mut F,
    ) -> Result<Option<BTreeMap<String, String>>, ManifestError>
    where
        F: FnMut(&str, &str) -> Result<String, String>,
    {
        let mut options = self
            .artifact_store_base_storage_options()?
            .unwrap_or_default();
        if let Some(parameters) = &self.artifact_store.ssm_parameters {
            validate_artifact_store_ssm_parameters(parameters)?;
            options.insert(
                "access_key_id".to_string(),
                resolve_artifact_store_secret(
                    resolver,
                    &parameters.region,
                    "access_key_id",
                    &parameters.access_key_id,
                )?,
            );
            options.insert(
                "secret_access_key".to_string(),
                resolve_artifact_store_secret(
                    resolver,
                    &parameters.region,
                    "secret_access_key",
                    &parameters.secret_access_key,
                )?,
            );
            if let Some(session_token) = &parameters.session_token {
                options.insert(
                    "session_token".to_string(),
                    resolve_artifact_store_secret(
                        resolver,
                        &parameters.region,
                        "session_token",
                        session_token,
                    )?,
                );
            }
        }
        ensure_artifact_store_s3_credentials_resolved(
            output_prefix_protocol(&self.output_prefix),
            &options,
        )?;
        if options.is_empty() {
            Ok(None)
        } else {
            Ok(Some(options))
        }
    }

    fn artifact_store_base_storage_options(
        &self,
    ) -> Result<Option<BTreeMap<String, String>>, ManifestError> {
        validate_catalog_storage_options(
            output_prefix_protocol(&self.output_prefix),
            &self.artifact_store.storage_options,
            &self.artifact_store.rust_storage_options,
        )?;
        validate_artifact_store_secrets(&self.artifact_store)?;
        ensure_artifact_store_conditional_put_enabled(
            output_prefix_protocol(&self.output_prefix),
            &self.artifact_store.storage_options,
            &self.artifact_store.rust_storage_options,
        )?;
        if !self.artifact_store.rust_storage_options.is_empty() {
            Ok(Some(self.artifact_store.rust_storage_options.clone()))
        } else if !self.artifact_store.storage_options.is_empty() {
            Ok(Some(self.artifact_store.storage_options.clone()))
        } else {
            Ok(None)
        }
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

fn ensure_supported_enums(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    parse_oms_type(&manifest.venue.oms_type)?;
    parse_account_type(&manifest.venue.account_type)?;
    parse_book_type(&manifest.venue.book_type)?;
    parse_oto_trigger_mode(&manifest.venue.oto_trigger_mode)?;
    parse_base_currency(&manifest.venue.base_currency)?;
    parse_default_leverage(&manifest.venue.default_leverage)?;
    Ok(())
}

fn ensure_unsupported_nt_venue_surfaces_absent(
    venue: &ManifestVenueConfig,
) -> Result<(), ManifestError> {
    for (field, present) in [
        (UNSUPPORTED_NT_VENUE_SURFACES[0], venue.leverages.is_some()),
        (
            UNSUPPORTED_NT_VENUE_SURFACES[1],
            venue.margin_model.is_some(),
        ),
        (UNSUPPORTED_NT_VENUE_SURFACES[2], venue.modules.is_some()),
        (UNSUPPORTED_NT_VENUE_SURFACES[3], venue.fill_model.is_some()),
        (
            UNSUPPORTED_NT_VENUE_SURFACES[4],
            venue.latency_model.is_some(),
        ),
        (UNSUPPORTED_NT_VENUE_SURFACES[5], venue.fee_model.is_some()),
        (
            UNSUPPORTED_NT_VENUE_SURFACES[6],
            venue.settlement_prices.is_some(),
        ),
    ] {
        if present {
            return Err(ManifestError::UnsupportedNtSurface { field });
        }
    }
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

fn parse_catalog_fs_protocol(value: &str) -> Result<Option<String>, ManifestError> {
    match value {
        CATALOG_FS_PROTOCOL_NONE => Ok(None),
        "s3" | "gs" | "gcs" | "az" | "abfs" | "http" | "https" => Ok(Some(value.to_string())),
        other => Err(ManifestError::UnsupportedEnum {
            field: "catalog_input.catalog_fs_protocol",
            value: other.to_string(),
        }),
    }
}

fn output_prefix_protocol(output_prefix: &str) -> Option<&str> {
    output_prefix
        .split_once("://")
        .map(|(protocol, _)| protocol)
}

fn validate_artifact_root_protocol(artifact_root: &str) -> Result<(), ManifestError> {
    match output_prefix_protocol(artifact_root) {
        Some("s3" | "file") => Ok(()),
        Some(other) => Err(ManifestError::UnsupportedEnum {
            field: "artifact_root",
            value: other.to_string(),
        }),
        None => Err(ManifestError::UnsupportedEnum {
            field: "artifact_root",
            value: CATALOG_FS_PROTOCOL_NONE.to_string(),
        }),
    }
}

fn validate_catalog_storage_options(
    protocol: Option<&str>,
    storage_options: &BTreeMap<String, String>,
    rust_storage_options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if !storage_options.is_empty() && !rust_storage_options.is_empty() {
        return Err(ManifestError::UnsupportedEnum {
            field: "catalog_input.catalog_fs_storage_options",
            value: CATALOG_STORAGE_OPTIONS_SHADOWED.to_string(),
        });
    }
    if protocol.is_none() && (!storage_options.is_empty() || !rust_storage_options.is_empty()) {
        return Err(ManifestError::UnsupportedEnum {
            field: "catalog_input.catalog_fs_protocol",
            value: format!("{CATALOG_FS_PROTOCOL_NONE} cannot carry storage options"),
        });
    }
    if protocol == Some("s3") {
        for (key, value) in storage_options {
            ensure_supported_s3_storage_option(
                "catalog_input.catalog_fs_storage_options",
                key,
                value,
            )?;
        }
        for (key, value) in rust_storage_options {
            ensure_supported_s3_storage_option(
                "catalog_input.catalog_fs_rust_storage_options",
                key,
                value,
            )?;
        }
    }
    Ok(())
}

fn ensure_supported_s3_storage_option(
    field: &'static str,
    key: &str,
    value: &str,
) -> Result<(), ManifestError> {
    match key {
        "endpoint_url" | "region" | "access_key_id" | "key" | "secret_access_key" | "secret"
        | "session_token" | "token" | "allow_http" => Ok(()),
        S3_OPTION_CONDITIONAL_PUT => validate_s3_conditional_put_value(field, value),
        other => Err(ManifestError::UnsupportedEnum {
            field,
            value: other.to_string(),
        }),
    }
}

fn validate_s3_conditional_put_value(
    field: &'static str,
    value: &str,
) -> Result<(), ManifestError> {
    match value {
        S3_CONDITIONAL_PUT_ETAG | S3_CONDITIONAL_PUT_DISABLED => Ok(()),
        other => Err(ManifestError::UnsupportedEnum {
            field,
            value: format!("{S3_OPTION_CONDITIONAL_PUT}={other}"),
        }),
    }
}

fn ensure_artifact_store_conditional_put_enabled(
    protocol: Option<&str>,
    storage_options: &BTreeMap<String, String>,
    rust_storage_options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if protocol != Some("s3") {
        return Ok(());
    }
    for options in [storage_options, rust_storage_options] {
        if options.get(S3_OPTION_CONDITIONAL_PUT).map(String::as_str)
            == Some(S3_CONDITIONAL_PUT_DISABLED)
        {
            return Err(ManifestError::UnsupportedEnum {
                field: "artifact_store.rust_storage_options.conditional_put",
                value: "disabled cannot support Artifact Index create-only event writes or conditional latest-pointer updates".to_string(),
            });
        }
    }
    Ok(())
}

fn validate_artifact_store_secrets(
    artifact_store: &ManifestArtifactStore,
) -> Result<(), ManifestError> {
    reject_raw_artifact_store_credentials(
        "artifact_store.storage_options",
        &artifact_store.storage_options,
    )?;
    reject_raw_artifact_store_credentials(
        "artifact_store.rust_storage_options",
        &artifact_store.rust_storage_options,
    )?;
    if let Some(parameters) = &artifact_store.ssm_parameters {
        validate_artifact_store_ssm_parameters(parameters)?;
    }
    Ok(())
}

fn ensure_artifact_store_s3_credentials_resolved(
    protocol: Option<&str>,
    options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if protocol != Some("s3") {
        return Ok(());
    }
    let has_access_key = options.contains_key("access_key_id") || options.contains_key("key");
    let has_secret_key =
        options.contains_key("secret_access_key") || options.contains_key("secret");
    if has_access_key && has_secret_key {
        Ok(())
    } else {
        Err(ManifestError::ArtifactStoreS3CredentialsNotResolved)
    }
}

fn reject_raw_artifact_store_credentials(
    field: &'static str,
    options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    for key in options.keys() {
        if is_s3_credential_option(key) {
            return Err(ManifestError::RawArtifactStoreCredential {
                field,
                key: key.clone(),
            });
        }
    }
    Ok(())
}

fn is_s3_credential_option(key: &str) -> bool {
    matches!(
        key,
        "access_key_id" | "key" | "secret_access_key" | "secret" | "session_token" | "token"
    )
}

fn validate_artifact_store_ssm_parameters(
    parameters: &ManifestArtifactStoreSsmParameters,
) -> Result<(), ManifestError> {
    if parameters.region.trim().is_empty() {
        return Err(ManifestError::MissingField(
            "artifact_store.ssm_parameters.region",
        ));
    }
    for (field, value) in [
        ("access_key_id", parameters.access_key_id.as_str()),
        ("secret_access_key", parameters.secret_access_key.as_str()),
    ] {
        validate_artifact_store_ssm_parameter(field, value)?;
    }
    if let Some(session_token) = &parameters.session_token {
        validate_artifact_store_ssm_parameter("session_token", session_token)?;
    }
    Ok(())
}

fn validate_artifact_store_ssm_parameter(
    field: &'static str,
    value: &str,
) -> Result<(), ManifestError> {
    if !value.starts_with('/') || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(ManifestError::InvalidArtifactStoreSsmParameter { field });
    }
    Ok(())
}

fn resolve_artifact_store_secret<F>(
    resolver: &mut F,
    region: &str,
    field: &'static str,
    path: &str,
) -> Result<String, ManifestError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    resolver(region, path)
        .map_err(|source| ManifestError::ArtifactStoreSecretResolution { field, source })
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

fn parse_oto_trigger_mode(value: &str) -> Result<OtoTriggerMode, ManifestError> {
    match value {
        "PARTIAL" => Ok(OtoTriggerMode::Partial),
        "FULL" => Ok(OtoTriggerMode::Full),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.oto_trigger_mode",
            value: other.to_string(),
        }),
    }
}

fn parse_base_currency(value: &str) -> Result<Option<Currency>, ManifestError> {
    if value == "NONE" {
        return Ok(None);
    }
    Currency::from_str(value)
        .map(Some)
        .map_err(|_| ManifestError::InvalidBaseCurrency {
            currency: value.to_string(),
        })
}

fn parse_default_leverage(value: &str) -> Result<Decimal, ManifestError> {
    let leverage = Decimal::from_str(value).map_err(|_| ManifestError::InvalidDefaultLeverage {
        leverage: value.to_string(),
    })?;
    if leverage <= Decimal::ZERO {
        return Err(ManifestError::InvalidDefaultLeverage {
            leverage: value.to_string(),
        });
    }
    Ok(leverage)
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
        AcceptanceMode, AcceptanceScope, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
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
            acceptance_scope: Some(AcceptanceScope {
                planned_objects: 1,
                completed_objects: 1,
                failed_objects: 0,
                skipped_objects: 0,
                accepted_bytes: object.bytes,
                selector_scope_violations: 0,
            }),
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
            proof_pin_reason_code: None,
            proof_pin_reason_detail: None,
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
                routing: false,
                frozen_account: false,
                reject_stop_orders: true,
                support_gtd_orders: true,
                support_contingent_orders: true,
                use_position_ids: true,
                use_random_ids: false,
                use_reduce_only: true,
                bar_execution: true,
                bar_adaptive_high_low_ordering: false,
                trade_execution: true,
                use_market_order_acks: false,
                liquidity_consumption: false,
                allow_cash_borrowing: false,
                queue_position: false,
                oto_trigger_mode: "PARTIAL".to_string(),
                base_currency: "NONE".to_string(),
                default_leverage: "1".to_string(),
                price_protection_points: 0,
                leverages: None,
                margin_model: None,
                modules: None,
                fill_model: None,
                latency_model: None,
                fee_model: None,
                settlement_prices: None,
            },
            catalog_input: ManifestCatalogInput {
                catalog_path: "/tmp/catalog".to_string(),
                catalog_fs_protocol: CATALOG_FS_PROTOCOL_NONE.to_string(),
                catalog_fs_storage_options: BTreeMap::new(),
                catalog_fs_rust_storage_options: BTreeMap::new(),
                data_type: "TradeTick".to_string(),
                nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
            },
            artifact_root: "s3://bolt-parquet/nt-research-analytics".to_string(),
            output_prefix: "s3://bolt-parquet/nt-research-analytics/backtests/bnbusdc".to_string(),
            artifact_store: ManifestArtifactStore {
                storage_options: BTreeMap::new(),
                rust_storage_options: BTreeMap::new(),
                ssm_parameters: None,
            },
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
    fn venue_config_maps_explicit_nt_venue_controls() {
        let mut manifest = valid_manifest();
        manifest.venue.routing = true;
        manifest.venue.frozen_account = true;
        manifest.venue.reject_stop_orders = false;
        manifest.venue.support_gtd_orders = false;
        manifest.venue.support_contingent_orders = false;
        manifest.venue.use_position_ids = false;
        manifest.venue.use_random_ids = true;
        manifest.venue.use_reduce_only = false;
        manifest.venue.bar_execution = false;
        manifest.venue.bar_adaptive_high_low_ordering = true;
        manifest.venue.trade_execution = false;
        manifest.venue.use_market_order_acks = true;
        manifest.venue.liquidity_consumption = true;
        manifest.venue.allow_cash_borrowing = true;
        manifest.venue.queue_position = true;
        manifest.venue.oto_trigger_mode = "FULL".to_string();
        manifest.venue.base_currency = "USDC".to_string();
        manifest.venue.default_leverage = "2".to_string();
        manifest.venue.price_protection_points = 7;

        let venue = manifest.to_nt_venue_config().expect("venue config");
        assert!(venue.routing());
        assert!(venue.frozen_account());
        assert!(!venue.reject_stop_orders());
        assert!(!venue.support_gtd_orders());
        assert!(!venue.support_contingent_orders());
        assert!(!venue.use_position_ids());
        assert!(venue.use_random_ids());
        assert!(!venue.use_reduce_only());
        assert!(!venue.bar_execution());
        assert!(venue.bar_adaptive_high_low_ordering());
        assert!(!venue.trade_execution());
        assert!(venue.use_market_order_acks());
        assert!(venue.liquidity_consumption());
        assert!(venue.allow_cash_borrowing());
        assert!(venue.queue_position());
        assert_eq!(
            venue.oto_trigger_mode(),
            nautilus_model::enums::OtoTriggerMode::Full
        );
        assert_eq!(
            venue.base_currency().expect("base currency").to_string(),
            "USDC"
        );
        assert_eq!(venue.default_leverage(), rust_decimal::Decimal::from(2));
        assert_eq!(venue.price_protection_points(), 7);
    }

    #[test]
    fn alternate_venue_provider_swap_is_toml_only() {
        let mut accepted = accepted_dataset();
        accepted.source_binding = "alt-native-trades".to_string();
        accepted.source_proof_id = "source-proof-alt-native-trades".to_string();
        accepted.venue = "altvenue".to_string();

        let mut manifest = valid_manifest();
        manifest.run_id = "backtesting-vertical-slice-altvenue-2026-03-01".to_string();
        manifest.venue_binding_key = accepted.source_binding.clone();
        manifest.source_proof_id = accepted.source_proof_id.clone();
        manifest.venue.nt_venue = "ALTVENUE".to_string();
        manifest.strategy.parameters.insert(
            "bar_type".to_string(),
            "ALTUSD.ALTVENUE-1-MINUTE-LAST-INTERNAL".to_string(),
        );
        manifest.catalog_input.nt_instrument_id = "ALTUSD.ALTVENUE".to_string();
        manifest.output_prefix =
            "s3://bolt-parquet/nt-research-analytics/backtests/altvenue".to_string();

        manifest
            .validate(&accepted)
            .expect("alternate TOML-only venue/provider binding");
        let venue = manifest.to_nt_venue_config().expect("venue config");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(venue.name().as_str(), "ALTVENUE");
        assert_eq!(
            data.instrument_id().expect("instrument id").to_string(),
            "ALTUSD.ALTVENUE"
        );
    }

    #[test]
    fn data_config_maps_catalog_cloud_options() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_input.catalog_fs_protocol = "s3".to_string();
        manifest.catalog_input.catalog_fs_rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("allow_http".to_string(), "false".to_string()),
        ]);

        let data = manifest.to_nt_data_config().expect("data config");
        assert_eq!(data.catalog_path(), manifest.catalog_input.catalog_path);
        assert_eq!(data.catalog_fs_protocol(), Some("s3"));
        assert!(data.catalog_fs_storage_options().is_none());
        assert_eq!(
            data.catalog_fs_rust_storage_options()
                .expect("storage options")
                .get("region"),
            Some(&"us-east-1".to_string())
        );
        assert_eq!(
            data.catalog_fs_rust_storage_options()
                .expect("rust storage options")
                .get("allow_http"),
            Some(&"false".to_string())
        );
    }

    #[test]
    fn data_config_preserves_configured_object_store_conditional_put() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_input.catalog_fs_protocol = "s3".to_string();
        manifest.catalog_input.catalog_fs_rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("conditional_put".to_string(), "etag".to_string()),
        ]);

        let data = manifest.to_nt_data_config().expect("data config");
        assert_eq!(
            data.catalog_fs_rust_storage_options()
                .expect("rust storage options")
                .get("conditional_put"),
            Some(&"etag".to_string())
        );
    }

    #[test]
    fn artifact_store_options_are_toml_owned_for_publish_and_catalog_proof() {
        let mut manifest = valid_manifest();
        manifest.artifact_root = "file:///bolt-artifacts".to_string();
        manifest.output_prefix = "file:///bolt-artifacts/backtests/bnbusdc".to_string();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("allow_http".to_string(), "false".to_string()),
        ]);

        let options = manifest
            .artifact_store_storage_options()
            .expect("artifact store options")
            .expect("rust storage options present");
        assert_eq!(options.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(options.get("allow_http"), Some(&"false".to_string()));
    }

    #[test]
    fn artifact_store_preserves_conditional_put_after_ssm_resolution() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("conditional_put".to_string(), "etag".to_string()),
        ]);
        manifest.artifact_store.ssm_parameters = Some(ManifestArtifactStoreSsmParameters {
            region: "us-east-1".to_string(),
            access_key_id: "/bolt/artifacts/access-key-id".to_string(),
            secret_access_key: "/bolt/artifacts/secret-access-key".to_string(),
            session_token: None,
        });

        let options = manifest
            .artifact_store_storage_options_resolved(&mut |_region, path| match path {
                "/bolt/artifacts/access-key-id" => Ok("AKIATEST".to_string()),
                "/bolt/artifacts/secret-access-key" => Ok("secret-value".to_string()),
                other => Err(format!("unexpected path {other}")),
            })
            .expect("resolved artifact store options")
            .expect("artifact store options");

        assert_eq!(options.get("conditional_put"), Some(&"etag".to_string()));
        assert_eq!(options.get("access_key_id"), Some(&"AKIATEST".to_string()));
        assert_eq!(
            options.get("secret_access_key"),
            Some(&"secret-value".to_string())
        );
    }

    #[test]
    fn artifact_store_rejects_disabled_conditional_put_for_s3_commit_path() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("conditional_put".to_string(), "disabled".to_string()),
        ]);

        let err = manifest
            .artifact_store_storage_options_resolved(&mut |_region, _path| {
                Ok("unused-secret".to_string())
            })
            .unwrap_err();

        assert!(
            err.to_string().contains("conditional_put")
                && err.to_string().contains("Artifact Index"),
            "{err}"
        );
    }

    #[test]
    fn artifact_store_rejects_s3_publish_without_resolved_ssm_credentials() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        let mut resolver_called = false;

        let err = manifest
            .artifact_store_storage_options_resolved(&mut |_region, _path| {
                resolver_called = true;
                Ok("unexpected-secret".to_string())
            })
            .unwrap_err();

        assert!(
            !resolver_called,
            "missing SSM parameter config must fail before any secret lookup"
        );
        assert!(
            err.to_string().contains("artifact_store.ssm_parameters")
                && err.to_string().contains("s3 output_prefix")
                && !err.to_string().contains("unexpected-secret"),
            "error must explain the missing SSM credential binding without exposing values: {err}"
        );
    }

    #[test]
    fn artifact_store_resolves_s3_credentials_from_ssm_parameters() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        manifest.artifact_store.ssm_parameters = Some(ManifestArtifactStoreSsmParameters {
            region: "us-east-1".to_string(),
            access_key_id: "/bolt/artifacts/access-key-id".to_string(),
            secret_access_key: "/bolt/artifacts/secret-access-key".to_string(),
            session_token: Some("/bolt/artifacts/session-token".to_string()),
        });

        let mut requested_paths = Vec::new();
        let options = manifest
            .artifact_store_storage_options_resolved(&mut |region, path| {
                assert_eq!(region, "us-east-1");
                requested_paths.push(path.to_string());
                match path {
                    "/bolt/artifacts/access-key-id" => Ok("AKIATEST".to_string()),
                    "/bolt/artifacts/secret-access-key" => Ok("secret-value".to_string()),
                    "/bolt/artifacts/session-token" => Ok("session-value".to_string()),
                    other => Err(format!("unexpected path {other}")),
                }
            })
            .expect("resolved artifact store options")
            .expect("s3 options");

        assert_eq!(
            requested_paths,
            vec![
                "/bolt/artifacts/access-key-id".to_string(),
                "/bolt/artifacts/secret-access-key".to_string(),
                "/bolt/artifacts/session-token".to_string(),
            ]
        );
        assert_eq!(options.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(options.get("access_key_id"), Some(&"AKIATEST".to_string()));
        assert_eq!(
            options.get("secret_access_key"),
            Some(&"secret-value".to_string())
        );
        assert_eq!(
            options.get("session_token"),
            Some(&"session-value".to_string())
        );
    }

    #[test]
    fn artifact_store_rejects_raw_s3_credentials_in_toml() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("secret_access_key".to_string(), "not-from-ssm".to_string()),
        ]);

        let err = manifest.artifact_store_storage_options().unwrap_err();

        assert!(
            err.to_string().contains("artifact_store")
                && err.to_string().contains("SSM")
                && !err.to_string().contains("not-from-ssm"),
            "error must reject raw credentials without rendering secret value: {err}"
        );
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
    fn manifest_hash_covers_venue_controls_and_catalog_cloud_fields() {
        fn assert_hash_changes(label: &str, mutate: impl FnOnce(&mut BacktestingRunManifest)) {
            let manifest = valid_manifest();
            let mut changed = manifest.clone();
            mutate(&mut changed);
            assert_ne!(
                manifest.manifest_hash(),
                changed.manifest_hash(),
                "{label} must affect the manifest hash"
            );
        }

        assert_hash_changes("venue.routing", |manifest| {
            manifest.venue.routing = true;
        });
        assert_hash_changes("venue.reject_stop_orders", |manifest| {
            manifest.venue.reject_stop_orders = false;
        });
        assert_hash_changes("venue.base_currency", |manifest| {
            manifest.venue.base_currency = "USDC".to_string();
        });
        assert_hash_changes("venue.price_protection_points", |manifest| {
            manifest.venue.price_protection_points = 7;
        });
        assert_hash_changes("catalog_input.catalog_fs_protocol", |manifest| {
            manifest.catalog_input.catalog_path =
                "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
            manifest.catalog_input.catalog_fs_protocol = "s3".to_string();
        });
        assert_hash_changes(
            "catalog_input.catalog_fs_rust_storage_options",
            |manifest| {
                manifest.catalog_input.catalog_fs_rust_storage_options =
                    BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
            },
        );
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
    fn rejects_non_latest_proof_pin_for_normal_run() {
        let mut manifest = valid_manifest();
        manifest.pins_non_latest_proof = true;
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NonLatestProofPinForNormalRun
        );
    }

    #[test]
    fn rejects_non_latest_proof_pin_without_reason_code() {
        let mut manifest = valid_manifest();
        manifest.run_purpose = RunPurpose::Audit;
        manifest.pins_non_latest_proof = true;

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("proof_pin_reason_code")
        );
    }

    #[test]
    fn rejects_audit_non_latest_proof_pin_without_reason_detail() {
        let mut manifest = valid_manifest();
        manifest.run_purpose = RunPurpose::Audit;
        manifest.pins_non_latest_proof = true;
        manifest.proof_pin_reason_code = Some(ProofPinReasonCode::AuditOrInvestigation);

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("proof_pin_reason_detail")
        );
    }

    #[test]
    fn accepts_non_latest_reproduction_pin_with_reason_code() {
        let mut manifest = valid_manifest();
        manifest.run_purpose = RunPurpose::Reproduction;
        manifest.pins_non_latest_proof = true;
        manifest.proof_pin_reason_code = Some(ProofPinReasonCode::BaselineReproduction);

        manifest
            .validate(&accepted_dataset())
            .expect("reproduction pin with structured reason should validate");
    }

    #[test]
    fn accepts_all_configured_non_latest_proof_pin_reason_codes_from_toml() {
        for (run_purpose, reason_code) in [
            ("reproduction", "published_result_reproduction"),
            ("regression", "regression_comparison"),
        ] {
            let toml = toml::to_string(&valid_manifest())
                .expect("serialize manifest")
                .replace(
                    "run_purpose = \"normal\"",
                    &format!("run_purpose = \"{run_purpose}\""),
                )
                .replace(
                    "pins_non_latest_proof = false",
                    &format!(
                        "pins_non_latest_proof = true\nproof_pin_reason_code = \"{reason_code}\""
                    ),
                );
            let manifest: BacktestingRunManifest =
                toml::from_str(&toml).expect("parse allowed proof-pin reason code");

            manifest
                .validate(&accepted_dataset())
                .expect("allowed proof-pin reason code should validate");
        }
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
    fn artifact_root_resolves_typed_subpaths_without_extra_root_knobs() {
        let manifest = valid_manifest();
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::Raw)
                .expect("raw subpath"),
            "s3://bolt-parquet/nt-research-analytics/raw"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::NtCatalog)
                .expect("nt catalog subpath"),
            "s3://bolt-parquet/nt-research-analytics/nt-catalog"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::SourceProofs)
                .expect("source proof subpath"),
            "s3://bolt-parquet/nt-research-analytics/source-proofs"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::Backtests)
                .expect("backtest subpath"),
            "s3://bolt-parquet/nt-research-analytics/backtests"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::ArtifactIndex)
                .expect("artifact index subpath"),
            "s3://bolt-parquet/nt-research-analytics/artifact-index"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::ResearchAnalytics)
                .expect("research analytics subpath"),
            "s3://bolt-parquet/nt-research-analytics/research-analytics"
        );
    }

    #[test]
    fn rejects_unsupported_artifact_root_scheme() {
        let mut manifest = valid_manifest();
        manifest.artifact_root = "http://example.test/nt-research-analytics".to_string();
        manifest.output_prefix =
            "http://example.test/nt-research-analytics/backtests/run".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "artifact_root",
                value: "http".to_string(),
            }
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
    fn rejects_unsupported_catalog_fs_protocol() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.catalog_fs_protocol = "ftp".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "catalog_input.catalog_fs_protocol",
                value: "ftp".to_string(),
            }
        );
    }

    #[test]
    fn rejects_shadowed_catalog_storage_options_before_nt_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_input.catalog_fs_protocol = "s3".to_string();
        manifest.catalog_input.catalog_fs_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        manifest.catalog_input.catalog_fs_rust_storage_options =
            BTreeMap::from([("allow_http".to_string(), "false".to_string())]);

        let expected = ManifestError::UnsupportedEnum {
            field: "catalog_input.catalog_fs_storage_options",
            value: "cannot be combined with catalog_fs_rust_storage_options".to_string(),
        };
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            expected
        );
        assert_eq!(manifest.to_nt_data_config().unwrap_err(), expected);
    }

    #[test]
    fn rejects_unknown_s3_catalog_rust_storage_option_before_nt_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_input.catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_input.catalog_fs_protocol = "s3".to_string();
        manifest.catalog_input.catalog_fs_rust_storage_options = BTreeMap::from([(
            "aws_virtual_hosted_style_request".to_string(),
            "false".to_string(),
        )]);

        let expected = ManifestError::UnsupportedEnum {
            field: "catalog_input.catalog_fs_rust_storage_options",
            value: "aws_virtual_hosted_style_request".to_string(),
        };
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            expected
        );
        assert_eq!(manifest.to_nt_data_config().unwrap_err(), expected);
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
    fn rejects_unsupported_oto_trigger_mode() {
        let mut manifest = valid_manifest();
        manifest.venue.oto_trigger_mode = "INVALID".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.oto_trigger_mode",
                value: "INVALID".to_string(),
            }
        );
    }

    #[test]
    fn rejects_invalid_base_currency() {
        let mut manifest = valid_manifest();
        manifest.venue.base_currency = "NOT_A_CURRENCY".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidBaseCurrency {
                currency: "NOT_A_CURRENCY".to_string(),
            }
        );
    }

    #[test]
    fn rejects_non_positive_default_leverage() {
        let mut manifest = valid_manifest();
        manifest.venue.default_leverage = "0".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidDefaultLeverage {
                leverage: "0".to_string(),
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

    #[test]
    fn rejects_unsupported_nt_venue_model_surface_requests_before_nt_config() {
        let serialized = toml::to_string(&valid_manifest()).expect("serialize");
        for (field, value) in [
            ("leverages", "{}"),
            ("margin_model", "\"standard\""),
            ("modules", "[]"),
            ("fill_model", "\"probabilistic\""),
            ("latency_model", "\"static\""),
            ("fee_model", "\"maker_taker\""),
            ("settlement_prices", "{}"),
        ] {
            let text = serialized.replace("[venue]\n", &format!("[venue]\n{field} = {value}\n"));
            let manifest = parse_manifest_toml(&text)
                .expect("unsupported NT venue surface should be represented in schema");
            let err = manifest
                .validate(&accepted_dataset())
                .expect_err("unsupported NT venue surface must fail validation");
            assert!(
                matches!(err, ManifestError::UnsupportedNtSurface { field: actual } if actual == field),
                "unsupported venue surface {field:?} must fail fast, got {err}"
            );
        }
    }

    #[test]
    fn typed_unsupported_nt_venue_model_surfaces_parse_then_fail_before_nt_config() {
        let serialized = toml::to_string(&valid_manifest()).expect("serialize");
        for (field, value) in [
            ("leverages", "{ \"BTCUSDT.BINANCE\" = \"2\" }"),
            ("margin_model", "\"standard\""),
            ("modules", "[\"latency-probe\"]"),
            ("fill_model", "\"probabilistic\""),
            ("latency_model", "\"static\""),
            ("fee_model", "\"maker_taker\""),
            ("settlement_prices", "{ \"BTCUSDT.BINANCE\" = \"65000\" }"),
        ] {
            let text = serialized.replace("[venue]\n", &format!("[venue]\n{field} = {value}\n"));
            let manifest = parse_manifest_toml(&text)
                .expect("unsupported NT venue surface should be represented in schema");
            let err = manifest
                .to_nt_venue_config()
                .expect_err("unsupported NT venue surface must not reach NT config");
            assert!(
                matches!(err, ManifestError::UnsupportedNtSurface { field: actual } if actual == field),
                "unsupported venue surface {field:?} should fail with a structured error, got {err}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_nt_engine_surface_requests_before_nt_config() {
        let text = format!(
            "[engine]\nrisk_engine = {{ bypass = true }}\n{}",
            toml::to_string(&valid_manifest()).expect("serialize")
        );
        let err = parse_manifest_toml(&text).unwrap_err().to_string();
        assert!(
            err.contains("engine"),
            "unsupported engine surface must fail fast, got {err}"
        );
    }
}
