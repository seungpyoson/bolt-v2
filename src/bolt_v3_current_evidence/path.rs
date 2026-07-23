use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CanonicalRelativeEvidencePath {
    display: String,
    components: Box<[OsString]>,
}

impl CanonicalRelativeEvidencePath {
    pub(crate) fn parse(field: &str, raw: &str) -> Result<Self, String> {
        let normalized = raw.trim();
        let components = normalized
            .split('/')
            .map(OsString::from)
            .collect::<Box<[_]>>();
        if raw != normalized
            || normalized.is_empty()
            || Path::new(normalized).is_absolute()
            || components
                .iter()
                .any(|component| component.is_empty() || component == "." || component == "..")
        {
            return Err(format!(
                "persistence.decision_evidence.{field} must be non-empty, relative, normalized, and stay under catalog_directory"
            ));
        }
        Ok(Self {
            display: normalized.to_string(),
            components,
        })
    }

    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.display
    }

    pub(crate) fn components(&self) -> impl ExactSizeIterator<Item = &OsStr> {
        self.components.iter().map(OsString::as_os_str)
    }

    #[must_use]
    pub(crate) fn is_ancestor_of(&self, other: &Self) -> bool {
        self.components.len() < other.components.len()
            && self
                .components
                .iter()
                .zip(other.components.iter())
                .all(|(left, right)| left == right)
    }
}
