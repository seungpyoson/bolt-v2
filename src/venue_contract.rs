use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 3;
/// Absolute sanity bound for static contract-sourced fee or rebate rates.
/// Positive values are fees; negative values are rebates.
pub const STATIC_FEE_BPS_ABSOLUTE_LIMIT: u32 = 10_000;

pub const STREAM_CLASS_QUOTES: &str = "quotes";
pub const STREAM_CLASS_TRADES: &str = "trades";
pub const STREAM_CLASS_ORDER_BOOK_DELTAS: &str = "order_book_deltas";
pub const STREAM_CLASS_ORDER_BOOK_DEPTHS: &str = "order_book_depths";
pub const STREAM_CLASS_INDEX_PRICES: &str = "index_prices";
pub const STREAM_CLASS_MARK_PRICES: &str = "mark_prices";
pub const STREAM_CLASS_INSTRUMENT_CLOSES: &str = "instrument_closes";

const SUPPORTED_STREAM_CLASSES: &[&str] = &[
    STREAM_CLASS_QUOTES,
    STREAM_CLASS_TRADES,
    STREAM_CLASS_ORDER_BOOK_DELTAS,
    STREAM_CLASS_ORDER_BOOK_DEPTHS,
    STREAM_CLASS_INDEX_PRICES,
    STREAM_CLASS_MARK_PRICES,
    STREAM_CLASS_INSTRUMENT_CLOSES,
];

pub fn supported_stream_classes() -> &'static [&'static str] {
    SUPPORTED_STREAM_CLASSES
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Supported,
    Unsupported,
    Conditional,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Required,
    Optional,
    Disabled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Native,
    Derived,
}

/// Payout/settlement structure for instruments on a venue.
///
/// This describes only the *payout* shape (what an outcome is worth at
/// resolution), not the *resolution rule* (at-expiry vs path-dependent), which
/// is instrument-type math owned by the market-family layer.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettlementKind {
    /// Winner-take-all outcome tokens that settle to 0 or 1 at resolution
    /// (e.g. Polymarket binary markets).
    Binary,
}

/// Per-minute REST request budget and batch limits the strategy must respect
/// when pacing order traffic to a venue.
///
/// These values mirror the venue's published limits and inform strategy-side
/// pacing. `validate()` only enforces positivity; the execution adapter remains
/// the authoritative enforcer of the physical ceiling, so a budget set above the
/// adapter's real limit is clamped or rejected by the adapter at runtime rather
/// than here.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RateBudget {
    /// Sustained CLOB REST requests permitted per minute.
    pub clob_per_minute: u32,
    /// Sustained Gamma REST requests permitted per minute.
    pub gamma_per_minute: u32,
    /// Maximum number of orders accepted in a single batch submit.
    pub batch_submit_limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCapabilities {
    /// Whether the venue supports in-place order modification. When `false`,
    /// requoting must cancel + resubmit.
    pub supports_modify: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaintenancePolicy {
    NoneConfigured,
    Scheduled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    // Keep this schema-owned enum stable instead of inheriting chrono's wire
    // representation for contract TOML.
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScheduledMaintenanceWindow {
    pub weekday: Weekday,
    /// UTC start time in HH:MM format.
    pub start_time_utc: String,
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceWindow {
    /// Whether the venue exposes a scheduled pull window the maker must honor.
    pub policy: MaintenancePolicy,
    /// Seconds before a scheduled window when resting quotes must be pulled.
    pub pull_before_start_seconds: u64,
    /// Concrete scheduled windows, empty only when `policy = "none_configured"`.
    pub windows: Vec<ScheduledMaintenanceWindow>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BookDepthSource {
    OrderBookDeltas,
    OrderBookDepths,
}

impl BookDepthSource {
    fn stream_class(&self) -> &'static str {
        match self {
            Self::OrderBookDeltas => STREAM_CLASS_ORDER_BOOK_DELTAS,
            Self::OrderBookDepths => STREAM_CLASS_ORDER_BOOK_DEPTHS,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DepthAvailability {
    /// Stream class that supplies maker-side book depth for quote decisions.
    pub book_depth_source: BookDepthSource,
    /// Whether the venue provides native queue-position identity.
    pub native_queue_position: Capability,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeeRateSource {
    Contract,
    Instrument,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FeeSchedule {
    /// Source of maker-side fee or rebate rates. The contract names the source;
    /// downstream fee providers fetch per-instrument rates from that source.
    pub maker_fee_rate_source: FeeRateSource,
    /// Source of taker-side fee rates for fill and backtest accounting.
    pub taker_fee_rate_source: FeeRateSource,
    /// Settlement/fee currency used by this venue adapter.
    pub settlement_currency: String,
    /// Static maker fee or rebate rate in basis points when sourced from the
    /// contract. Positive values are fees, negative values are rebates, and
    /// magnitude is capped by `STATIC_FEE_BPS_ABSOLUTE_LIMIT`.
    pub maker_fee_bps: Option<i32>,
    /// Static taker fee or rebate rate in basis points when sourced from the
    /// contract. Positive values are fees, negative values are rebates, and
    /// magnitude is capped by `STATIC_FEE_BPS_ABSOLUTE_LIMIT`.
    pub taker_fee_bps: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SettlementCapabilities {
    /// Venue-level settlement kind. Market-family modules own payout math.
    pub kind: SettlementKind,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamContract {
    pub capability: Capability,
    pub policy: Policy,
    pub provenance: Provenance,
    pub reason: Option<String>,
    pub derived_from: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VenueContract {
    pub schema_version: u32,
    pub venue: String,
    pub adapter_version: String,
    pub execution: ExecutionCapabilities,
    /// REST request budget and batch limits for order-traffic pacing.
    pub rate_budget: RateBudget,
    pub maintenance_window: MaintenanceWindow,
    pub depth_availability: DepthAvailability,
    pub fee_schedule: FeeSchedule,
    pub settlement: SettlementCapabilities,
    pub streams: BTreeMap<String, StreamContract>,
}

#[derive(Debug, Deserialize)]
struct SchemaVersionEnvelope {
    schema_version: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClassReportStatus {
    Pass,
    PassUnsupported,
    PassDisabled,
    WarnOptionalAbsent,
    SpoolPresentConversionEmpty,
    FailUnknown,
    FailContractViolation,
    FailRequiredAbsent,
}

impl ClassReportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::PassUnsupported => "pass_unsupported",
            Self::PassDisabled => "pass_disabled",
            Self::WarnOptionalAbsent => "warn_optional_absent",
            Self::SpoolPresentConversionEmpty => "spool_present_conversion_empty",
            Self::FailUnknown => "fail_unknown",
            Self::FailContractViolation => "fail_contract_violation",
            Self::FailRequiredAbsent => "fail_required_absent",
        }
    }
}

impl fmt::Display for ClassReportStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PartialEq<&str> for ClassReportStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletenessOutcome {
    Pass,
    Fail,
}

impl CompletenessOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}

impl fmt::Display for CompletenessOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PartialEq<&str> for CompletenessOutcome {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClassReportCapability {
    Supported,
    Unsupported,
    Conditional,
    Unknown,
}

impl ClassReportCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Conditional => "conditional",
            Self::Unknown => "unknown",
        }
    }
}

impl From<&Capability> for ClassReportCapability {
    fn from(capability: &Capability) -> Self {
        match capability {
            Capability::Supported => Self::Supported,
            Capability::Unsupported => Self::Unsupported,
            Capability::Conditional => Self::Conditional,
        }
    }
}

impl fmt::Display for ClassReportCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PartialEq<&str> for ClassReportCapability {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassReport {
    pub capability: ClassReportCapability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<Policy>,
    pub spool_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_converted: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_converted: Option<u64>,
    pub status: ClassReportStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletenessReport {
    pub schema_version: u32,
    pub venue: String,
    pub contract_version: u32,
    pub instance_id: String,
    pub outcome: CompletenessOutcome,
    pub classes: BTreeMap<String, ClassReport>,
}

/// Test-only contract fixture: discover whichever venue contract(s) ship under
/// the repo's `contracts/` directory, load the first via the production loader,
/// then swap in caller-supplied `streams`. No venue name, budget value,
/// settlement kind, or policy is written here — the envelope is sourced entirely
/// from the shipped config (the single source of truth), so the fixtures carry no
/// literals. It selects the lexically-first contract under `contracts/` as an
/// arbitrary valid envelope; tests that assert venue-specific facts load their
/// contract explicitly rather than relying on this selection. Integration
/// tests use the mirror in `tests/support`; Rust's lib/integration-test boundary
/// forces the two copies of this discovery logic (it carries no config values,
/// only the lookup).
#[cfg(test)]
pub(crate) fn sample_contract_with_streams(
    streams: BTreeMap<String, StreamContract>,
) -> VenueContract {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("contracts dir {} must be readable: {error}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect();
    paths.sort();
    let path = paths
        .first()
        .unwrap_or_else(|| panic!("at least one venue contract must ship in {}", dir.display()));
    let mut contract = VenueContract::load_and_validate(path)
        .unwrap_or_else(|error| panic!("shipped contract {} must load: {error}", path.display()));
    contract.streams = streams;
    contract
}

impl VenueContract {
    pub fn load_and_validate(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read contract {}: {e}", path.display()))?;
        let envelope: SchemaVersionEnvelope = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse contract {}: {e}", path.display()))?;
        ensure_current_schema_version(envelope.schema_version)?;
        let contract: VenueContract = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse contract {}: {e}", path.display()))?;
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<()> {
        ensure_current_schema_version(self.schema_version)?;

        ensure!(
            self.rate_budget.clob_per_minute > 0,
            "rate_budget.clob_per_minute must be positive"
        );
        ensure!(
            self.rate_budget.gamma_per_minute > 0,
            "rate_budget.gamma_per_minute must be positive"
        );
        ensure!(
            self.rate_budget.batch_submit_limit > 0,
            "rate_budget.batch_submit_limit must be positive"
        );

        match self.maintenance_window.policy {
            MaintenancePolicy::NoneConfigured => {
                ensure!(
                    self.maintenance_window.pull_before_start_seconds == 0,
                    "maintenance_window.pull_before_start_seconds must be 0 when policy is none_configured"
                );
                ensure!(
                    self.maintenance_window.windows.is_empty(),
                    "maintenance_window.windows must be empty when policy is none_configured"
                );
            }
            MaintenancePolicy::Scheduled => {
                ensure!(
                    self.maintenance_window.pull_before_start_seconds > 0,
                    "scheduled maintenance requires positive pull_before_start_seconds"
                );
                ensure!(
                    !self.maintenance_window.windows.is_empty(),
                    "scheduled maintenance requires at least one window"
                );
            }
        }

        for (idx, window) in self.maintenance_window.windows.iter().enumerate() {
            ensure!(
                is_hh_mm_utc(&window.start_time_utc),
                "maintenance_window.windows[{idx}].start_time_utc must be HH:MM"
            );
            ensure!(
                window.duration_seconds > 0,
                "maintenance_window.windows[{idx}].duration_seconds must be positive"
            );
        }

        let depth_stream_class = self.depth_availability.book_depth_source.stream_class();
        let Some(depth_stream) = self.streams.get(depth_stream_class) else {
            anyhow::bail!(
                "depth_availability.book_depth_source references missing stream {depth_stream_class}"
            );
        };
        ensure!(
            depth_stream.capability != Capability::Unsupported,
            "depth_availability.book_depth_source references unsupported stream {depth_stream_class}"
        );
        ensure!(
            depth_stream.policy != Policy::Disabled,
            "depth_availability.book_depth_source references disabled stream {depth_stream_class}"
        );

        ensure!(
            !self.fee_schedule.settlement_currency.trim().is_empty(),
            "fee_schedule.settlement_currency must be non-empty"
        );
        validate_fee_rate_source(
            "maker",
            &self.fee_schedule.maker_fee_rate_source,
            self.fee_schedule.maker_fee_bps,
        )?;
        validate_fee_rate_source(
            "taker",
            &self.fee_schedule.taker_fee_rate_source,
            self.fee_schedule.taker_fee_bps,
        )?;

        for cls in supported_stream_classes() {
            ensure!(
                self.streams.contains_key(*cls),
                "contract missing required stream class: {cls}"
            );
        }

        for (name, stream) in &self.streams {
            ensure!(
                supported_stream_classes().contains(&name.as_str()),
                "adapter does not implement stream class: {name}"
            );
            match stream.capability {
                Capability::Unsupported => {
                    ensure!(
                        stream.policy == Policy::Disabled,
                        "stream {name}: unsupported capability must have disabled policy"
                    );
                }
                Capability::Supported | Capability::Conditional => {
                    ensure!(
                        stream.policy == Policy::Required
                            || stream.policy == Policy::Optional
                            || stream.policy == Policy::Disabled,
                        "stream {name}: supported capability has invalid policy {:?}",
                        stream.policy
                    );
                }
            }

            if stream.provenance == Provenance::Derived {
                let derived_from = stream.derived_from.as_ref();
                ensure!(
                    derived_from.is_some_and(|v| !v.is_empty()),
                    "stream {name}: derived provenance requires \
                     non-empty derived_from"
                );
                for source in derived_from.unwrap() {
                    let source_stream = self.streams.get(source);
                    ensure!(
                        source_stream.is_some_and(|s| s.capability == Capability::Supported),
                        "stream {name}: derived_from references {source} \
                         which is not supported"
                    );
                }
            }
        }

        Ok(())
    }

    pub fn effective_policy(&self, class: &str) -> Option<Policy> {
        self.streams.get(class).map(|s| s.policy.clone())
    }
}

fn is_hh_mm_utc(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.chars().all(|c| c.is_ascii_digit())
        && minute.chars().all(|c| c.is_ascii_digit())
        && hour.parse::<u8>().is_ok_and(|v| v < 24)
        && minute.parse::<u8>().is_ok_and(|v| v < 60)
}

fn ensure_current_schema_version(schema_version: u32) -> Result<()> {
    ensure!(
        schema_version == CURRENT_SCHEMA_VERSION,
        "unsupported contract schema_version {schema_version}, expected {CURRENT_SCHEMA_VERSION}"
    );
    Ok(())
}

fn validate_fee_rate_source(side: &str, source: &FeeRateSource, bps: Option<i32>) -> Result<()> {
    match source {
        FeeRateSource::Contract => {
            let Some(bps) = bps else {
                anyhow::bail!(
                    "fee_schedule.{side}_fee_bps required when {side}_fee_rate_source is contract"
                );
            };
            ensure!(
                bps.unsigned_abs() <= STATIC_FEE_BPS_ABSOLUTE_LIMIT,
                "fee_schedule.{side}_fee_bps must be within {STATIC_FEE_BPS_ABSOLUTE_LIMIT} bps of zero when {side}_fee_rate_source is contract"
            );
        }
        FeeRateSource::Instrument => ensure!(
            bps.is_none(),
            "fee_schedule.{side}_fee_bps must be absent when {side}_fee_rate_source is instrument"
        ),
    }

    Ok(())
}

pub fn normalize_local_absolute_contract_path(path: &Path) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();
    ensure!(
        !path_str.contains("://"),
        "contract_path must be a local absolute path, got `{}`",
        path.display()
    );
    ensure!(
        path.is_absolute(),
        "contract_path must be a local absolute path, got `{}`",
        path.display()
    );

    normalize_absolute_path(path)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }

    let mut tail = Vec::<OsString>::new();
    let mut cursor = path;
    while !cursor.exists() {
        let name = cursor
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("unable to normalize path {}", path.display()))?;
        tail.push(name.to_os_string());
        cursor = cursor.parent().ok_or_else(|| {
            anyhow::anyhow!("unable to find existing ancestor for {}", path.display())
        })?;
    }

    let mut resolved = fs::canonicalize(cursor)?;
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}
