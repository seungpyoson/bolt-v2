use super::*;

pub(super) fn validate_persistence_block(block: &PersistenceBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if !Path::new(&block.catalog_directory).is_absolute() {
        errors.push(format!(
            "persistence.catalog_directory must be an absolute path: `{}`",
            block.catalog_directory
        ));
    }
    if let Some(required_catalog_prefix) = block.required_catalog_prefix.as_deref()
        && !Path::new(required_catalog_prefix).is_absolute()
    {
        errors.push(format!(
            "{}.{} must be an absolute path: `{}`",
            stringify!(persistence),
            stringify!(required_catalog_prefix),
            required_catalog_prefix
        ));
    }
    if block.runtime_capture_start_poll_interval_ms == 0 {
        errors.push(
            "persistence.runtime_capture_start_poll_interval_ms must be a positive integer"
                .to_string(),
        );
    }
    if block.data_client_readiness_probe_poll_interval_ms == 0 {
        errors.push(format!(
            "{}.{} must be a positive integer",
            stringify!(persistence),
            stringify!(data_client_readiness_probe_poll_interval_ms)
        ));
    }
    if block
        .min_free_bytes
        .is_some_and(|min_free_bytes| min_free_bytes == 0)
    {
        errors.push(format!(
            "{}.{} must be a positive integer",
            stringify!(persistence),
            stringify!(min_free_bytes)
        ));
    }
    if block.streaming.flush_interval_ms == 0 {
        errors
            .push("persistence.streaming.flush_interval_ms must be a positive integer".to_string());
    }
    let evidence = &block.decision_evidence;
    let mut configured_paths = Vec::new();
    for (field, relative_path) in [
        (
            "machine_relative_path",
            evidence.machine_relative_path.as_str(),
        ),
        (
            "observation_relative_path",
            evidence.observation_relative_path.as_str(),
        ),
    ] {
        match CanonicalRelativeEvidencePath::parse(field, relative_path) {
            Ok(path) => configured_paths.push(path),
            Err(message) => errors.push(message),
        }
    }
    if evidence.retired_relative_paths.is_empty() {
        errors.push(
            "persistence.decision_evidence.retired_relative_paths must register at least one retired path"
                .to_string(),
        );
    }
    if evidence.reject_episode_max_count == 0 {
        errors.push(
            "persistence.decision_evidence.reject_episode_max_count must be a positive integer"
                .to_string(),
        );
    }
    for retired in &evidence.retired_relative_paths {
        match CanonicalRelativeEvidencePath::parse("retired_relative_paths", retired) {
            Ok(path) => configured_paths.push(path),
            Err(message) => errors.push(message),
        }
    }
    for (index, left) in configured_paths.iter().enumerate() {
        for right in &configured_paths[index + 1..] {
            if left == right {
                errors.push(format!(
                    "persistence.decision_evidence paths must be distinct: `{}`",
                    left.as_str()
                ));
            } else if left.is_ancestor_of(right) || right.is_ancestor_of(left) {
                errors.push(format!(
                    "persistence.decision_evidence paths must not be ancestors of one another: `{}` and `{}`",
                    left.as_str(),
                    right.as_str()
                ));
            }
        }
    }
    if let Err(message) =
        PositiveFiniteEvidenceReadCap::new(block.decision_evidence.recovery_evidence_max_bytes)
    {
        errors.push(format!(
            "persistence.decision_evidence.recovery_evidence_max_bytes {message}"
        ));
    }
    errors
}
