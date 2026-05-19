use crate::{
    bolt_v3_adapters::{BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::make_bolt_v3_live_node_builder,
    bolt_v3_secrets::{check_no_forbidden_credential_env_vars_with, resolve_bolt_v3_secrets_with},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3StartupCheckReport {
    pub facts: Vec<BoltV3StartupCheckFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3StartupCheckFact {
    pub stage: BoltV3StartupCheckStage,
    pub subject: BoltV3StartupCheckSubject,
    pub status: BoltV3StartupCheckStatus,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3StartupCheckStage {
    ForbiddenCredentialEnv,
    SecretResolution,
    AdapterMapping,
    LiveNodeBuilder,
    ClientRegistration,
    LiveNodeBuild,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3StartupCheckStatus {
    Satisfied,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3StartupCheckSubject {
    Root,
    Venue(String),
    BlockedByStage(BoltV3StartupCheckStage),
}

impl BoltV3StartupCheckReport {
    fn new() -> Self {
        Self { facts: Vec::new() }
    }

    fn push(
        &mut self,
        stage: BoltV3StartupCheckStage,
        subject: BoltV3StartupCheckSubject,
        status: BoltV3StartupCheckStatus,
        detail: impl Into<String>,
    ) {
        self.facts.push(BoltV3StartupCheckFact {
            stage,
            subject,
            status,
            detail: detail.into(),
        });
    }

    fn skip_after(
        &mut self,
        blocked_by: BoltV3StartupCheckStage,
        stages: &[BoltV3StartupCheckStage],
    ) {
        for stage in stages {
            self.push(
                *stage,
                BoltV3StartupCheckSubject::BlockedByStage(blocked_by),
                BoltV3StartupCheckStatus::Skipped,
                format!("{stage:?} blocked by {blocked_by:?}"),
            );
        }
    }
}

pub fn run_bolt_v3_startup_check_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> BoltV3StartupCheckReport
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let mut report = BoltV3StartupCheckReport::new();

    match check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set) {
        Ok(()) => report.push(
            BoltV3StartupCheckStage::ForbiddenCredentialEnv,
            BoltV3StartupCheckSubject::Root,
            BoltV3StartupCheckStatus::Satisfied,
            "no forbidden credential environment variables are set",
        ),
        Err(error) => {
            for finding in error.findings {
                report.push(
                    BoltV3StartupCheckStage::ForbiddenCredentialEnv,
                    BoltV3StartupCheckSubject::Venue(finding.venue_key.clone()),
                    BoltV3StartupCheckStatus::Failed,
                    finding.to_string(),
                );
            }
            report.skip_after(
                BoltV3StartupCheckStage::ForbiddenCredentialEnv,
                &[
                    BoltV3StartupCheckStage::SecretResolution,
                    BoltV3StartupCheckStage::AdapterMapping,
                    BoltV3StartupCheckStage::LiveNodeBuilder,
                    BoltV3StartupCheckStage::ClientRegistration,
                    BoltV3StartupCheckStage::LiveNodeBuild,
                ],
            );
            return report;
        }
    }

    let resolved = match resolve_bolt_v3_secrets_with(loaded, resolver) {
        Ok(resolved) => {
            if resolved.venues.is_empty() {
                report.push(
                    BoltV3StartupCheckStage::SecretResolution,
                    BoltV3StartupCheckSubject::Root,
                    BoltV3StartupCheckStatus::Satisfied,
                    "no venue secrets configured",
                );
            } else {
                for venue_key in resolved.venues.keys() {
                    report.push(
                        BoltV3StartupCheckStage::SecretResolution,
                        BoltV3StartupCheckSubject::Venue(venue_key.clone()),
                        BoltV3StartupCheckStatus::Satisfied,
                        format!("resolved secrets for venue `{venue_key}`"),
                    );
                }
            }
            resolved
        }
        Err(error) => {
            report.push(
                BoltV3StartupCheckStage::SecretResolution,
                BoltV3StartupCheckSubject::Venue(error.venue_key.clone()),
                BoltV3StartupCheckStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3StartupCheckStage::SecretResolution,
                &[
                    BoltV3StartupCheckStage::AdapterMapping,
                    BoltV3StartupCheckStage::LiveNodeBuilder,
                    BoltV3StartupCheckStage::ClientRegistration,
                    BoltV3StartupCheckStage::LiveNodeBuild,
                ],
            );
            return report;
        }
    };

    let adapters = match map_bolt_v3_adapters(loaded, &resolved) {
        Ok(adapters) => {
            if adapters.venues.is_empty() {
                report.push(
                    BoltV3StartupCheckStage::AdapterMapping,
                    BoltV3StartupCheckSubject::Root,
                    BoltV3StartupCheckStatus::Satisfied,
                    "no venue adapter configs mapped",
                );
            } else {
                for (venue_key, venue) in &adapters.venues {
                    report.push(
                        BoltV3StartupCheckStage::AdapterMapping,
                        BoltV3StartupCheckSubject::Venue(venue_key.clone()),
                        BoltV3StartupCheckStatus::Satisfied,
                        format!(
                            "mapped adapter configs for venue `{venue_key}`: data={} execution={}",
                            venue.data.is_some(),
                            venue.execution.is_some()
                        ),
                    );
                }
            }
            adapters
        }
        Err(error) => {
            report.push(
                BoltV3StartupCheckStage::AdapterMapping,
                adapter_mapping_error_subject(&error),
                BoltV3StartupCheckStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3StartupCheckStage::AdapterMapping,
                &[
                    BoltV3StartupCheckStage::LiveNodeBuilder,
                    BoltV3StartupCheckStage::ClientRegistration,
                    BoltV3StartupCheckStage::LiveNodeBuild,
                ],
            );
            return report;
        }
    };

    let builder = match make_bolt_v3_live_node_builder(loaded) {
        Ok(builder) => {
            report.push(
                BoltV3StartupCheckStage::LiveNodeBuilder,
                BoltV3StartupCheckSubject::Root,
                BoltV3StartupCheckStatus::Satisfied,
                "created NT LiveNodeBuilder from bolt-v3 config",
            );
            builder
        }
        Err(error) => {
            report.push(
                BoltV3StartupCheckStage::LiveNodeBuilder,
                BoltV3StartupCheckSubject::Root,
                BoltV3StartupCheckStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3StartupCheckStage::LiveNodeBuilder,
                &[
                    BoltV3StartupCheckStage::ClientRegistration,
                    BoltV3StartupCheckStage::LiveNodeBuild,
                ],
            );
            return report;
        }
    };

    let builder = match register_bolt_v3_clients(builder, adapters) {
        Ok((builder, summary)) => {
            push_registration_summary(&mut report, &summary);
            builder
        }
        Err(error) => {
            report.push(
                BoltV3StartupCheckStage::ClientRegistration,
                client_registration_error_subject(&error),
                BoltV3StartupCheckStatus::Failed,
                error.to_string(),
            );
            report.skip_after(
                BoltV3StartupCheckStage::ClientRegistration,
                &[BoltV3StartupCheckStage::LiveNodeBuild],
            );
            return report;
        }
    };

    match builder.build() {
        Ok(_node) => {
            report.push(
                BoltV3StartupCheckStage::LiveNodeBuild,
                BoltV3StartupCheckSubject::Root,
                BoltV3StartupCheckStatus::Satisfied,
                "built NT LiveNode without connecting clients",
            );
        }
        Err(error) => {
            report.push(
                BoltV3StartupCheckStage::LiveNodeBuild,
                BoltV3StartupCheckSubject::Root,
                BoltV3StartupCheckStatus::Failed,
                error.to_string(),
            );
        }
    }

    report
}

fn adapter_mapping_error_subject(error: &BoltV3AdapterMappingError) -> BoltV3StartupCheckSubject {
    let venue_key = match error {
        BoltV3AdapterMappingError::InstrumentFilter(_) => {
            return BoltV3StartupCheckSubject::Root;
        }
        BoltV3AdapterMappingError::SecretKindMismatch { venue_key, .. }
        | BoltV3AdapterMappingError::MissingResolvedSecrets { venue_key, .. }
        | BoltV3AdapterMappingError::SchemaParse { venue_key, .. }
        | BoltV3AdapterMappingError::NumericRange { venue_key, .. }
        | BoltV3AdapterMappingError::ValidationInvariant { venue_key, .. } => venue_key,
    };
    BoltV3StartupCheckSubject::Venue(venue_key.clone())
}

fn client_registration_error_subject(
    error: &BoltV3ClientRegistrationError,
) -> BoltV3StartupCheckSubject {
    let venue_key = match error {
        BoltV3ClientRegistrationError::AddDataClient { venue_key, .. }
        | BoltV3ClientRegistrationError::AddExecClient { venue_key, .. } => venue_key,
    };
    BoltV3StartupCheckSubject::Venue(venue_key.clone())
}

fn push_registration_summary(
    report: &mut BoltV3StartupCheckReport,
    summary: &BoltV3RegistrationSummary,
) {
    if summary.venues.is_empty() {
        report.push(
            BoltV3StartupCheckStage::ClientRegistration,
            BoltV3StartupCheckSubject::Root,
            BoltV3StartupCheckStatus::Satisfied,
            "no NT clients registered because no venues are configured",
        );
        return;
    }

    for (venue_key, venue) in &summary.venues {
        report.push(
            BoltV3StartupCheckStage::ClientRegistration,
            BoltV3StartupCheckSubject::Venue(venue_key.clone()),
            BoltV3StartupCheckStatus::Satisfied,
            format!(
                "registered NT clients for venue `{venue_key}`: data={} execution={}",
                venue.data, venue.execution
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_registration_error_subject_is_venue_keyed() {
        let data_error = BoltV3ClientRegistrationError::AddDataClient {
            venue_key: "venue_a".to_string(),
            message: "data rejected".to_string(),
        };
        assert_eq!(
            client_registration_error_subject(&data_error),
            BoltV3StartupCheckSubject::Venue("venue_a".to_string())
        );

        let exec_error = BoltV3ClientRegistrationError::AddExecClient {
            venue_key: "venue_b".to_string(),
            message: "execution rejected".to_string(),
        };
        assert_eq!(
            client_registration_error_subject(&exec_error),
            BoltV3StartupCheckSubject::Venue("venue_b".to_string())
        );
    }
}
