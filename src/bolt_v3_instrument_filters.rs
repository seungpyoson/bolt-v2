//! Configured target fields used to build NT `InstrumentFilter`s.
//!
//! This module carries the TOML fields needed by provider bindings to
//! set NT adapter instrument filters. The facts are derived once from
//! validated strategy config and then passed through provider bindings
//! without reparsing the TOML tree.
//!
//! Source-level guard tests keep provider-specific filter construction
//! under `crate::bolt_v3_providers`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentFilterConfig {
    targets: Vec<InstrumentFilterTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentFilterTarget {
    pub strategy_instance_id: String,
    pub family_key: &'static str,
    pub configured_target_id: String,
    pub venue: String,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_slug_token: String,
}

pub struct InstrumentFilterTargetRef<'a> {
    pub strategy_instance_id: &'a str,
    pub family_key: &'static str,
    pub configured_target_id: &'a str,
    pub venue: &'a str,
    pub underlying_asset: &'a str,
    pub cadence_seconds: i64,
    pub cadence_slug_token: &'a str,
}

impl InstrumentFilterConfig {
    pub fn new(targets: Vec<InstrumentFilterTarget>) -> Self {
        Self { targets }
    }

    pub fn empty() -> Self {
        Self {
            targets: Vec::new(),
        }
    }

    pub fn target_refs(&self) -> impl Iterator<Item = InstrumentFilterTargetRef<'_>> {
        self.targets.iter().map(|target| InstrumentFilterTargetRef {
            strategy_instance_id: target.strategy_instance_id.as_str(),
            family_key: target.family_key,
            configured_target_id: target.configured_target_id.as_str(),
            venue: target.venue.as_str(),
            underlying_asset: target.underlying_asset.as_str(),
            cadence_seconds: target.cadence_seconds,
            cadence_slug_token: target.cadence_slug_token.as_str(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstrumentFilterError {
    NonPositiveCadenceSeconds {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_seconds: i64,
    },
    NegativeNowUnixSeconds {
        now_unix_seconds: i64,
    },
    PeriodPairOverflow {
        now_unix_seconds: i64,
        cadence_seconds: i64,
    },
    TargetParseFailed {
        strategy_instance_id: String,
        message: String,
    },
    UnsupportedFamily {
        context: Option<String>,
        family_key: String,
        supported: Vec<&'static str>,
    },
    TargetValidationFailure {
        message: String,
    },
    Other {
        message: String,
    },
}

impl std::fmt::Display for InstrumentFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            } => write!(
                f,
                "{prefix}target.cadence_seconds must be a positive integer (got {cadence_seconds})",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            Self::NegativeNowUnixSeconds { now_unix_seconds } => write!(
                f,
                "now_unix_seconds must be non-negative (got {now_unix_seconds})"
            ),
            Self::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            } => write!(
                f,
                "updown period pair overflows i64 (now_unix_seconds={now_unix_seconds}, cadence_seconds={cadence_seconds})"
            ),
            Self::TargetParseFailed {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategy `{strategy_instance_id}`: target failed updown typed deserialization after validation: {message}"
            ),
            Self::UnsupportedFamily {
                context,
                family_key,
                ..
            } => match context {
                Some(context) => write!(
                    f,
                    "{context}: target.rotating_market_family `{family_key}` is not supported by this build"
                ),
                None => write!(
                    f,
                    "target.rotating_market_family `{family_key}` is not supported by this build"
                ),
            },
            Self::TargetValidationFailure { message } => f.write_str(message),
            Self::Other { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for InstrumentFilterError {}

pub(crate) fn format_target_prefix(
    strategy_instance_id: &Option<String>,
    configured_target_id: &Option<String>,
) -> String {
    match (strategy_instance_id, configured_target_id) {
        (Some(strategy), Some(target)) => format!("strategy `{strategy}` target `{target}`: "),
        (Some(strategy), None) => format!("strategy `{strategy}`: "),
        (None, Some(target)) => format!("target `{target}`: "),
        (None, None) => String::new(),
    }
}
