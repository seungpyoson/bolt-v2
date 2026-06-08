use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
