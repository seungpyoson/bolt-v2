use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::ingest::IvPayloadKind;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IvAuditHandleId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvRawProductKind {
    OptionGreeks,
    OptionChainSlice,
    AggregateGreeks,
    CustomImpliedVolatility,
}

impl From<IvPayloadKind> for IvRawProductKind {
    fn from(value: IvPayloadKind) -> Self {
        match value {
            IvPayloadKind::OptionGreeks => Self::OptionGreeks,
            IvPayloadKind::OptionChainSlice => Self::OptionChainSlice,
            IvPayloadKind::AggregateGreeks => Self::AggregateGreeks,
            IvPayloadKind::CustomImpliedVolatility => Self::CustomImpliedVolatility,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvAuditRetention {
    pub max_events: Option<usize>,
    pub max_age_ns: Option<u64>,
}

impl IvAuditRetention {
    pub fn empty() -> Self {
        Self {
            max_events: None,
            max_age_ns: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvAuditPolicy {
    pub enabled_raw_products: BTreeSet<IvRawProductKind>,
    pub authorized_audit_handles: BTreeSet<IvAuditHandleId>,
    pub access_purposes: BTreeSet<String>,
    pub eligible_sources: BTreeSet<String>,
    pub audit_retention: IvAuditRetention,
}

impl IvAuditPolicy {
    pub fn raw_product_enabled(&self, product_kind: IvRawProductKind) -> bool {
        self.enabled_raw_products.contains(&product_kind)
    }

    pub fn authorizes(
        &self,
        raw_product_kind: IvRawProductKind,
        source_id: &str,
        audit_handle_id: &str,
        access_purpose: &str,
    ) -> bool {
        self.raw_product_enabled(raw_product_kind)
            && self.eligible_sources.contains(source_id)
            && self
                .authorized_audit_handles
                .contains(&IvAuditHandleId(audit_handle_id.to_string()))
            && self.access_purposes.contains(access_purpose)
    }
}
