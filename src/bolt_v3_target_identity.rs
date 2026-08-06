//! Checked identity types shared by configuration, selection, and evidence seams.

use std::fmt;

use nautilus_model::identifiers::StrategyId;
use serde::{Deserialize, Deserializer, Serialize, de};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ConfiguredTargetId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredTargetIdError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StrategyIdentityError {
    field: &'static str,
    message: String,
}

#[must_use]
pub fn stable_identity_field_is_canonical(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

pub fn checked_nt_strategy_id(
    strategy_id: &str,
    order_id_tag: &str,
) -> Result<StrategyId, BoltV3StrategyIdentityError> {
    if !stable_identity_field_is_canonical(strategy_id) {
        return Err(BoltV3StrategyIdentityError::new(
            "strategy_id",
            "must be a non-empty, unpadded string",
        ));
    }
    if !stable_identity_field_is_canonical(order_id_tag) {
        return Err(BoltV3StrategyIdentityError::new(
            "order_id_tag",
            "must be a non-empty, unpadded string",
        ));
    }
    let strategy_id = StrategyId::new_checked(strategy_id).map_err(|error| {
        BoltV3StrategyIdentityError::new(
            "strategy_id",
            format!("must be a valid NT StrategyId: {error}"),
        )
    })?;
    if strategy_id.get_tag() != order_id_tag {
        return Err(BoltV3StrategyIdentityError::new(
            "order_id_tag",
            format!("must match strategy_id tag `{}`", strategy_id.get_tag()),
        ));
    }
    Ok(strategy_id)
}

impl BoltV3StrategyIdentityError {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn field(&self) -> &'static str {
        self.field
    }
}

impl ConfiguredTargetId {
    pub fn new(value: String) -> Result<Self, ConfiguredTargetIdError> {
        stable_identity_field_is_canonical(value.as_str())
            .then_some(Self(value))
            .ok_or(ConfiguredTargetIdError)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<&str> for ConfiguredTargetId {
    type Error = ConfiguredTargetIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value.to_string())
    }
}

impl TryFrom<String> for ConfiguredTargetId {
    type Error = ConfiguredTargetIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ConfiguredTargetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(|_| de::Error::custom("must be a non-empty, unpadded string"))
    }
}

impl fmt::Display for ConfiguredTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PartialEq<str> for ConfiguredTargetId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ConfiguredTargetId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for ConfiguredTargetIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("must be a non-empty, unpadded string")
    }
}

impl std::error::Error for ConfiguredTargetIdError {}

impl fmt::Display for BoltV3StrategyIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.field, self.message)
    }
}

impl std::error::Error for BoltV3StrategyIdentityError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_target_id_rejects_every_malformed_class() {
        for value in ["", "   ", " target", "target "] {
            assert!(
                ConfiguredTargetId::try_from(value).is_err(),
                "accepted {value:?}"
            );
        }
    }

    #[test]
    fn configured_target_id_round_trips_checked_serde() {
        let id = ConfiguredTargetId::try_from("target-a").expect("canonical id");
        let encoded = serde_json::to_string(&id).expect("serialize");
        assert_eq!(
            serde_json::from_str::<ConfiguredTargetId>(&encoded).expect("deserialize"),
            id
        );
        assert!(serde_json::from_str::<ConfiguredTargetId>(r#"" target""#).is_err());
    }

    #[test]
    fn stable_fields_allow_internal_whitespace() {
        assert!(stable_identity_field_is_canonical("New York Yes"));
    }

    #[test]
    fn checked_identity_error_is_field_neutral() {
        let error = ConfiguredTargetId::try_from(" target").unwrap_err();
        assert_eq!(error.to_string(), "must be a non-empty, unpadded string");
        assert!(!error.to_string().contains("configured_target_id"));
    }

    #[test]
    fn checked_nt_strategy_identity_enforces_the_effective_nt_contract() {
        assert!(checked_nt_strategy_id("Maker New York-001", "001").is_ok());
        for (strategy_id, order_id_tag) in [
            ("Maker New York", "001"),
            ("Mäker-001", "001"),
            ("Maker-001", "002"),
        ] {
            assert!(
                checked_nt_strategy_id(strategy_id, order_id_tag).is_err(),
                "accepted ({strategy_id:?}, {order_id_tag:?})"
            );
        }
    }
}
