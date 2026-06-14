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
#[serde(deny_unknown_fields)]
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

    pub fn authorizes(
        &self,
        strategy_id: &str,
        product_kind: IvProductKind,
        source_id: Option<&str>,
        selector_fingerprint: &str,
    ) -> bool {
        if self.strategy_id != strategy_id || !self.allowed_product_kinds.contains(&product_kind) {
            return false;
        }

        if let Some(source_id) = source_id
            && !self.allowed_source_ids.is_empty()
            && !self.allowed_source_ids.contains(source_id)
        {
            return false;
        }

        match self.authorization_mode {
            IvAuthorizationMode::ProfileWide => true,
            IvAuthorizationMode::SelectorScoped if product_kind == IvProductKind::SourceHealth => {
                source_id.is_some_and(|source_id| self.allowed_source_ids.contains(source_id))
            }
            IvAuthorizationMode::SelectorScoped => self
                .allowed_selector_fingerprints
                .contains(selector_fingerprint),
        }
    }
}
