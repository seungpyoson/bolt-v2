use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::types::IvProductKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvAuthorizationMode {
    ProfileWide,
    SelectorScoped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvSelectorAuthorization {
    pub authorization_mode: IvAuthorizationMode,
    pub strategy_id: String,
    pub allowed_product_kinds: BTreeSet<IvProductKind>,
    pub allowed_selector_fingerprints: BTreeSet<String>,
    pub allowed_source_ids: BTreeSet<String>,
}

impl IvSelectorAuthorization {
    pub fn is_profile_wide(&self) -> bool {
        self.authorization_mode == IvAuthorizationMode::ProfileWide
    }

    pub fn is_selector_scoped(&self) -> bool {
        self.authorization_mode == IvAuthorizationMode::SelectorScoped
    }
}
