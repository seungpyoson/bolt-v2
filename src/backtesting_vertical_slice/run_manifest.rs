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

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use nautilus_backtest::config::{
    BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{AccountType, BookType, OmsType},
    identifiers::InstrumentId,
};
use serde::{Deserialize, Serialize};
use ustr::Ustr;

use super::source_proof::AcceptedDataset;

/// Registry key for the compiled Rust trade-driven example strategy.
pub const STRATEGY_HURST_VPIN_DIRECTIONAL: &str = "hurst_vpin_directional";

/// Existing compiled Rust strategies selectable from a run manifest.
///
/// This is the single source of truth shared by manifest validation (gate 4)
/// and strategy instantiation (gate 5 runner).
#[must_use]
pub fn registered_strategies() -> &'static [&'static str] {
    &[STRATEGY_HURST_VPIN_DIRECTIONAL]
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
pub struct StrategySource {
    /// Key into [`registered_strategies`].
    pub registry_key: String,
    /// Typed parameters passed to the registered strategy constructor.
    pub parameters: BTreeMap<String, String>,
}

/// Simulated venue settings mapped into [`BacktestVenueConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct ManifestCatalogInput {
    pub catalog_path: String,
    /// NautilusTrader data type, currently `TradeTick`.
    pub data_type: String,
    /// NautilusTrader instrument id, for example `BNBUSDC.BYBIT`.
    pub nt_instrument_id: String,
}

/// The typed backtest run manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Optional exclusive end time (Unix nanos).
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
    UnacceptedData {
        manifest_proof: String,
        accepted_proof: String,
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
            Self::UnacceptedData {
                manifest_proof,
                accepted_proof,
            } => write!(
                f,
                "manifest source proof {manifest_proof:?} does not match accepted dataset proof {accepted_proof:?}"
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
    if !registered_strategies().contains(&key) {
        return Err(ManifestError::UnknownStrategy {
            registry_key: key.to_string(),
        });
    }
    Ok(())
}

impl BacktestingRunManifest {
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
        validate_strategy_source(&self.strategy)?;

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
        if self.run_purpose == RunPurpose::Normal && self.pins_non_latest_proof {
            return Err(ManifestError::NonLatestProofPinForNormalRun);
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
            .map_err(|_| ManifestError::MissingField("catalog_input.nt_instrument_id"))?;
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
        let to_nanos = |value: i64| UnixNanos::from(u64::try_from(value).unwrap_or_default());
        Ok(BacktestRunConfig::builder()
            .id(self.run_id.clone())
            .venues(vec![venue])
            .data(vec![data])
            .maybe_start(self.start_time.map(to_nanos))
            .maybe_end(self.end_time.map(to_nanos))
            .build())
    }
}

fn ensure_supported_enums(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    parse_oms_type(&manifest.venue.oms_type)?;
    parse_account_type(&manifest.venue.account_type)?;
    parse_book_type(&manifest.venue.book_type)?;
    Ok(())
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
    use crate::backtesting_vertical_slice::source_proof::{
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
                        "BNBUSDC.BYBIT-1-MINUTE-LAST-EXTERNAL".to_string(),
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
    fn rejects_unaccepted_data() {
        let mut manifest = valid_manifest();
        manifest.source_proof_id = "some-other-proof".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnacceptedData { .. }
        ));
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
}
