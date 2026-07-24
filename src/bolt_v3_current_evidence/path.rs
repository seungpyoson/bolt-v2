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
        let component_text = normalized.split('/').collect::<Vec<_>>();
        let root_component = component_text.first().copied().unwrap_or_default();
        if raw != normalized
            || normalized.is_empty()
            || Path::new(normalized).is_absolute()
            || component_text
                .iter()
                .any(|component| !is_portable_catalog_component(component))
            || root_component == crate::bolt_v3_operator_artifacts::LAUNCH_IDENTITY_FILE_NAME
            || root_component
                .starts_with(crate::bolt_v3_operator_artifacts::PRESTART_WRITE_PROBE_PREFIX)
        {
            return Err(format!(
                "persistence.decision_evidence.{field} must be non-empty, relative, normalized lowercase ASCII, and stay under catalog_directory"
            ));
        }
        let components = component_text
            .into_iter()
            .map(OsString::from)
            .collect::<Box<[_]>>();
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

fn is_portable_catalog_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::CanonicalRelativeEvidencePath;

    #[test]
    fn rejects_nonportable_namespace_components_before_catalog_access() {
        for raw in [
            "bolt-v3/decision-evidence/MACHINE.jsonl",
            "bolt-v3/decision-évidence/machine.jsonl",
            "bolt-v3/decision-e\u{301}vidence/machine.jsonl",
            "bolt-v3/decision\0evidence/machine.jsonl",
            "bolt-v3/decision evidence/machine.jsonl",
        ] {
            assert!(
                CanonicalRelativeEvidencePath::parse("machine_relative_path", raw).is_err(),
                "nonportable path must be rejected: {raw:?}"
            );
        }
    }

    #[test]
    fn accepts_the_portable_catalog_path_grammar() {
        let path = CanonicalRelativeEvidencePath::parse(
            "machine_relative_path",
            "bolt-v3/decision_evidence/current/machine.jsonl",
        )
        .expect("lowercase ASCII catalog path must be accepted");
        assert_eq!(
            path.as_str(),
            "bolt-v3/decision_evidence/current/machine.jsonl"
        );
    }

    #[test]
    fn rejects_fixed_catalog_occupants_as_stream_roots() {
        for raw in [
            crate::bolt_v3_operator_artifacts::LAUNCH_IDENTITY_FILE_NAME,
            ".bolt-v2-prestart-write-probe-123",
        ] {
            assert!(
                CanonicalRelativeEvidencePath::parse("machine_relative_path", raw).is_err(),
                "fixed catalog occupant must be reserved: {raw}"
            );
        }
    }
}
