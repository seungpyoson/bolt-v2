//! Checked identity types shared by configuration, selection, and evidence seams.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ConfiguredTargetId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredTargetIdError;

#[must_use]
pub fn stable_identity_field_is_canonical(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
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
        Self::try_from(value).map_err(|_| {
            de::Error::custom("configured_target_id must be a non-empty, unpadded string")
        })
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
        formatter.write_str("configured_target_id must be a non-empty, unpadded string")
    }
}

impl std::error::Error for ConfiguredTargetIdError {}

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
}
