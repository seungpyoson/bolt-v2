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
pub struct InstrumentFilterError {
    message: String,
}

impl InstrumentFilterError {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for InstrumentFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InstrumentFilterError {}
