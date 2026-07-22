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
    for (field, path) in [
        (
            "machine_relative_path",
            block.decision_evidence.machine_relative_path.as_str(),
        ),
        (
            "observation_relative_path",
            block.decision_evidence.observation_relative_path.as_str(),
        ),
    ] {
        if let Err(message) = validate_decision_evidence_relative_path(field, path) {
            errors.push(message);
        }
    }
    if block.decision_evidence.retired_relative_paths.is_empty() {
        errors.push(
            "persistence.decision_evidence.retired_relative_paths must name at least one retired stream"
                .to_string(),
        );
    }
    for path in &block.decision_evidence.retired_relative_paths {
        if let Err(message) =
            validate_decision_evidence_relative_path("retired_relative_paths", path)
        {
            errors.push(message);
        }
    }
    let machine_path = Path::new(block.decision_evidence.machine_relative_path.trim());
    let observation_path = Path::new(block.decision_evidence.observation_relative_path.trim());
    if machine_path == observation_path {
        errors.push(
            "persistence.decision_evidence machine_relative_path and observation_relative_path must resolve to different files"
                .to_string(),
        );
    }
    let retired_paths = block
        .decision_evidence
        .retired_relative_paths
        .iter()
        .map(|path| Path::new(path.trim()))
        .collect::<BTreeSet<_>>();
    if retired_paths.len() != block.decision_evidence.retired_relative_paths.len() {
        errors.push(
            "persistence.decision_evidence.retired_relative_paths must not contain duplicates"
                .to_string(),
        );
    }
    if retired_paths.contains(machine_path) || retired_paths.contains(observation_path) {
        errors.push(
            "persistence.decision_evidence retired paths must differ from active stream paths"
                .to_string(),
        );
    }
    if block
        .decision_evidence
        .recovery_evidence_max_bytes
        .is_some_and(|max_bytes| max_bytes == 0)
    {
        errors.push(
            "persistence.decision_evidence.recovery_evidence_max_bytes must be a positive integer"
                .to_string(),
        );
    }
    errors
}

pub(super) fn validate_capital_admission_recovery_evidence(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let enforced_submit_admission =
        crate::bolt_v3_settlement_runtime::capital_admission_runtime_feed_pool(root).is_some();
    if enforced_submit_admission
        && root
            .persistence
            .decision_evidence
            .recovery_evidence_max_bytes
            .is_none()
    {
        errors.push(
            "persistence.decision_evidence.recovery_evidence_max_bytes must be configured when risk.capital_pools enables submit admission enforcement"
                .to_string(),
        );
    }
    errors
}

pub(super) fn validate_settlement_sink_recovery_evidence(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let settlement_sink_configured =
        crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSinkBackends::from_root(root)
            .will_configure_runtime_sink();
    if settlement_sink_configured
        && root
            .persistence
            .decision_evidence
            .recovery_evidence_max_bytes
            .is_none()
    {
        errors.push(
            "persistence.decision_evidence.recovery_evidence_max_bytes must be configured when a settlement runtime sink is configured"
                .to_string(),
        );
    }
    errors
}
