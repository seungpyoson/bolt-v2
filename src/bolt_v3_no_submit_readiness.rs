use nautilus_live::node::LiveNode;

use crate::{
    bolt_v3_adapters::{BoltV3ClientMappingError, map_bolt_v3_clients},
    bolt_v3_client_registration::BoltV3RegistrationSummary,
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{
        BoltV3LiveNodeError, build_bolt_v3_client_only_live_node_from_adapters,
        build_bolt_v3_live_node_from_registered_builder, connect_bolt_v3_clients,
        disconnect_bolt_v3_clients, make_bolt_v3_client_registered_live_node_builder,
    },
    bolt_v3_secrets::{check_no_forbidden_credential_env_vars_with, resolve_bolt_v3_secrets_with},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3NoSubmitReadinessReport {
    pub facts: Vec<BoltV3NoSubmitReadinessFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3NoSubmitReadinessFact {
    pub stage: BoltV3NoSubmitReadinessStage,
    pub subject: BoltV3NoSubmitReadinessSubject,
    pub status: BoltV3NoSubmitReadinessStatus,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3NoSubmitReadinessStage {
    ForbiddenCredentialEnv,
    SecretResolution,
    AdapterMapping,
    LiveNodeBuilder,
    ClientRegistration,
    LiveNodeBuild,
    Connect,
    Disconnect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3NoSubmitReadinessStatus {
    Satisfied,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3NoSubmitReadinessSubject {
    Root,
    Client(String),
    BlockedByStage(BoltV3NoSubmitReadinessStage),
}

impl BoltV3NoSubmitReadinessReport {
    fn new() -> Self {
        Self { facts: Vec::new() }
    }

    fn push(
        &mut self,
        stage: BoltV3NoSubmitReadinessStage,
        subject: BoltV3NoSubmitReadinessSubject,
        status: BoltV3NoSubmitReadinessStatus,
        detail: impl Into<String>,
    ) {
        self.facts.push(BoltV3NoSubmitReadinessFact {
            stage,
            subject,
            status,
            detail: detail.into(),
        });
    }

    fn skip_after(
        &mut self,
        blocked_by: BoltV3NoSubmitReadinessStage,
        stages: &[BoltV3NoSubmitReadinessStage],
    ) {
        for stage in stages {
            self.push(
                *stage,
                BoltV3NoSubmitReadinessSubject::BlockedByStage(blocked_by),
                BoltV3NoSubmitReadinessStatus::Skipped,
                format!("{stage:?} blocked by {blocked_by:?}"),
            );
        }
    }
}

pub fn build_bolt_v3_no_submit_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(LiveNode, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_clients(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    build_bolt_v3_client_only_live_node_from_adapters(loaded, adapters)
}

pub async fn run_bolt_v3_no_submit_readiness_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> BoltV3NoSubmitReadinessReport
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let mut report = BoltV3NoSubmitReadinessReport::new();

    match check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set) {
        Ok(()) => report.push(
            BoltV3NoSubmitReadinessStage::ForbiddenCredentialEnv,
            BoltV3NoSubmitReadinessSubject::Root,
            BoltV3NoSubmitReadinessStatus::Satisfied,
            "no forbidden credential environment variables are set",
        ),
        Err(error) => {
            for finding in error.findings {
                report.push(
                    BoltV3NoSubmitReadinessStage::ForbiddenCredentialEnv,
                    BoltV3NoSubmitReadinessSubject::Client(finding.client_id_key.clone()),
                    BoltV3NoSubmitReadinessStatus::Failed,
                    finding.to_string(),
                );
            }
            report.skip_after(
                BoltV3NoSubmitReadinessStage::ForbiddenCredentialEnv,
                &[
                    BoltV3NoSubmitReadinessStage::SecretResolution,
                    BoltV3NoSubmitReadinessStage::AdapterMapping,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                    BoltV3NoSubmitReadinessStage::ClientRegistration,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
    }

    let resolved = match resolve_bolt_v3_secrets_with(loaded, resolver) {
        Ok(resolved) => {
            if resolved.clients.is_empty() {
                report.push(
                    BoltV3NoSubmitReadinessStage::SecretResolution,
                    BoltV3NoSubmitReadinessSubject::Root,
                    BoltV3NoSubmitReadinessStatus::Satisfied,
                    "no client secrets configured",
                );
            } else {
                for client_id_key in resolved.clients.keys() {
                    report.push(
                        BoltV3NoSubmitReadinessStage::SecretResolution,
                        BoltV3NoSubmitReadinessSubject::Client(client_id_key.clone()),
                        BoltV3NoSubmitReadinessStatus::Satisfied,
                        format!("resolved secrets for client_id `{client_id_key}`"),
                    );
                }
            }
            resolved
        }
        Err(error) => {
            report.push(
                BoltV3NoSubmitReadinessStage::SecretResolution,
                BoltV3NoSubmitReadinessSubject::Client(error.client_id_key.clone()),
                BoltV3NoSubmitReadinessStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3NoSubmitReadinessStage::SecretResolution,
                &[
                    BoltV3NoSubmitReadinessStage::AdapterMapping,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                    BoltV3NoSubmitReadinessStage::ClientRegistration,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
    };

    let adapters = match map_bolt_v3_clients(loaded, &resolved) {
        Ok(adapters) => {
            if adapters.clients.is_empty() {
                report.push(
                    BoltV3NoSubmitReadinessStage::AdapterMapping,
                    BoltV3NoSubmitReadinessSubject::Root,
                    BoltV3NoSubmitReadinessStatus::Satisfied,
                    "no client configs mapped",
                );
            } else {
                for (client_id_key, client_id) in &adapters.clients {
                    report.push(
                        BoltV3NoSubmitReadinessStage::AdapterMapping,
                        BoltV3NoSubmitReadinessSubject::Client(client_id_key.clone()),
                        BoltV3NoSubmitReadinessStatus::Satisfied,
                        format!(
                            "mapped client configs for client_id `{client_id_key}`: data={} execution={}",
                            client_id.data.is_some(),
                            client_id.execution.is_some()
                        ),
                    );
                }
            }
            adapters
        }
        Err(error) => {
            report.push(
                BoltV3NoSubmitReadinessStage::AdapterMapping,
                adapter_mapping_error_subject(&error),
                BoltV3NoSubmitReadinessStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3NoSubmitReadinessStage::AdapterMapping,
                &[
                    BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                    BoltV3NoSubmitReadinessStage::ClientRegistration,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
    };

    let (builder, _summary) = match make_bolt_v3_client_registered_live_node_builder(
        loaded, adapters,
    ) {
        Ok((builder, summary)) => {
            report.push(
                BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                BoltV3NoSubmitReadinessSubject::Root,
                BoltV3NoSubmitReadinessStatus::Satisfied,
                "created NT LiveNodeBuilder from bolt-v3 config",
            );
            push_registration_summary(&mut report, &summary);
            (builder, summary)
        }
        Err(BoltV3LiveNodeError::BuilderConstruction(error)) => {
            report.push(
                BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                BoltV3NoSubmitReadinessSubject::Root,
                BoltV3NoSubmitReadinessStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                &[
                    BoltV3NoSubmitReadinessStage::ClientRegistration,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
        Err(BoltV3LiveNodeError::ClientRegistration(error)) => {
            report.push(
                BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                BoltV3NoSubmitReadinessSubject::Root,
                BoltV3NoSubmitReadinessStatus::Satisfied,
                "created NT LiveNodeBuilder from bolt-v3 config",
            );
            report.push(
                BoltV3NoSubmitReadinessStage::ClientRegistration,
                BoltV3NoSubmitReadinessSubject::Client(match &error {
                    crate::bolt_v3_client_registration::BoltV3ClientRegistrationError::AddDataClient {
                        client_id_key,
                        ..
                    }
                    | crate::bolt_v3_client_registration::BoltV3ClientRegistrationError::AddExecClient {
                        client_id_key,
                        ..
                    } => client_id_key.clone(),
                }),
                BoltV3NoSubmitReadinessStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3NoSubmitReadinessStage::ClientRegistration,
                &[
                    BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
        Err(error) => {
            report.push(
                BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                BoltV3NoSubmitReadinessSubject::Root,
                BoltV3NoSubmitReadinessStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
                &[
                    BoltV3NoSubmitReadinessStage::ClientRegistration,
                    BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
    };

    let mut node = match build_bolt_v3_live_node_from_registered_builder(builder) {
        Ok(node) => {
            report.push(
                BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                BoltV3NoSubmitReadinessSubject::Root,
                BoltV3NoSubmitReadinessStatus::Satisfied,
                "built NT LiveNode from configured clients",
            );
            node
        }
        Err(error) => {
            report.push(
                BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                BoltV3NoSubmitReadinessSubject::Root,
                BoltV3NoSubmitReadinessStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3NoSubmitReadinessStage::LiveNodeBuild,
                &[
                    BoltV3NoSubmitReadinessStage::Connect,
                    BoltV3NoSubmitReadinessStage::Disconnect,
                ],
            );
            return report;
        }
    };

    match connect_bolt_v3_clients(&mut node, loaded).await {
        Ok(()) => report.push(
            BoltV3NoSubmitReadinessStage::Connect,
            BoltV3NoSubmitReadinessSubject::Root,
            BoltV3NoSubmitReadinessStatus::Satisfied,
            "connected configured NT clients through controlled-connect",
        ),
        Err(error) => report.push(
            BoltV3NoSubmitReadinessStage::Connect,
            BoltV3NoSubmitReadinessSubject::Root,
            BoltV3NoSubmitReadinessStatus::Failed,
            error.to_string(),
        ),
    }

    match disconnect_bolt_v3_clients(&mut node, loaded).await {
        Ok(()) => report.push(
            BoltV3NoSubmitReadinessStage::Disconnect,
            BoltV3NoSubmitReadinessSubject::Root,
            BoltV3NoSubmitReadinessStatus::Satisfied,
            "disconnected configured NT clients through controlled-disconnect",
        ),
        Err(error) => report.push(
            BoltV3NoSubmitReadinessStage::Disconnect,
            BoltV3NoSubmitReadinessSubject::Root,
            BoltV3NoSubmitReadinessStatus::Failed,
            error.to_string(),
        ),
    }

    report
}

fn adapter_mapping_error_subject(
    error: &BoltV3ClientMappingError,
) -> BoltV3NoSubmitReadinessSubject {
    let client_id_key = match error {
        BoltV3ClientMappingError::SecretVenueMismatch { client_id_key, .. }
        | BoltV3ClientMappingError::MissingResolvedSecrets { client_id_key, .. }
        | BoltV3ClientMappingError::SchemaParse { client_id_key, .. }
        | BoltV3ClientMappingError::NumericRange { client_id_key, .. }
        | BoltV3ClientMappingError::ValidationInvariant { client_id_key, .. } => client_id_key,
    };
    BoltV3NoSubmitReadinessSubject::Client(client_id_key.clone())
}

fn push_registration_summary(
    report: &mut BoltV3NoSubmitReadinessReport,
    summary: &BoltV3RegistrationSummary,
) {
    if summary.clients.is_empty() {
        report.push(
            BoltV3NoSubmitReadinessStage::ClientRegistration,
            BoltV3NoSubmitReadinessSubject::Root,
            BoltV3NoSubmitReadinessStatus::Satisfied,
            "no NT clients registered because no clients are configured",
        );
        return;
    }

    for (client_id_key, client_id) in &summary.clients {
        report.push(
            BoltV3NoSubmitReadinessStage::ClientRegistration,
            BoltV3NoSubmitReadinessSubject::Client(client_id_key.clone()),
            BoltV3NoSubmitReadinessStatus::Satisfied,
            format!(
                "registered NT clients for client_id `{client_id_key}`: data={} execution={}",
                client_id.data, client_id.execution
            ),
        );
    }
}
