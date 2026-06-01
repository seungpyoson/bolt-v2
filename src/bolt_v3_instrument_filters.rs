//! Shared market-family target error surface.
//!
//! This module owns the family-agnostic `InstrumentFilterError` enum and
//! its operator-facing prefix formatter. Both are consumed by the market-
//! family target bindings so target deserialization, cadence, and
//! dispatch failures render one consistent operator message regardless
//! of family.
//!
//! Source-level guard tests keep provider-specific filter construction
//! under `crate::bolt_v3_providers`.

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
