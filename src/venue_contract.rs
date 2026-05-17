use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Supported,
    Unsupported,
    Conditional,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Required,
    Optional,
    Disabled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Native,
    Derived,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamContract {
    pub capability: Capability,
    pub policy: Policy,
    pub provenance: Provenance,
    pub reason: Option<String>,
    pub derived_from: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VenueContract {
    pub schema_version: u32,
    pub venue: String,
    pub adapter_version: String,
    pub streams: BTreeMap<String, StreamContract>,
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

impl VenueContract {
    pub fn load_and_validate(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read contract {}: {e}", path.display()))?;
        let contract: VenueContract = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse contract {}: {e}", path.display()))?;
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported contract schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );

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
