//! IAM policy generation for per-kind Artifact Index producers.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::artifact_index::ArtifactKind;

pub const ARTIFACT_INDEX_PRODUCER_IAM_PROVISIONING_PLAN_ROLE: &str =
    "artifact-index-producer-iam-provisioning-plan.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexProducerIamPolicy {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Statement")]
    pub statements: Vec<ArtifactIndexProducerIamStatement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexProducerIamStatement {
    #[serde(rename = "Sid")]
    pub sid: String,
    #[serde(rename = "Effect")]
    pub effect: String,
    #[serde(rename = "Action")]
    pub actions: Vec<String>,
    #[serde(rename = "Resource")]
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexProducerIamProvisioningPlanSpec {
    pub artifact_root: String,
    pub artifact_kind: ArtifactKind,
    #[serde(default)]
    pub proof_artifact_roots: Vec<String>,
    pub ssm_parameter_prefix: String,
    #[serde(default)]
    pub denied_artifact_kinds: Vec<ArtifactKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexProducerIamProvisioningPlan {
    pub artifact_kind: ArtifactKind,
    pub ssm_parameter_paths: ArtifactIndexProducerSsmParameterPaths,
    pub policy: ArtifactIndexProducerIamPolicy,
    pub proof_denied_artifact_kinds: Vec<ArtifactKind>,
    pub expected_denied_write_attempts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexProducerSsmParameterPaths {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

pub fn artifact_index_producer_iam_provisioning_plan(
    spec: ArtifactIndexProducerIamProvisioningPlanSpec,
) -> Result<ArtifactIndexProducerIamProvisioningPlan, ArtifactIndexIamPolicyError> {
    validate_denied_kinds(spec.artifact_kind, &spec.denied_artifact_kinds)?;
    let ssm_prefix = validate_ssm_parameter_prefix(&spec.ssm_parameter_prefix)?;
    let proof_roots: Vec<&str> = spec
        .proof_artifact_roots
        .iter()
        .map(String::as_str)
        .collect();
    let policy =
        artifact_index_producer_iam_policy(&spec.artifact_root, spec.artifact_kind, &proof_roots)?;
    let kind_path = spec.artifact_kind.as_str();
    Ok(ArtifactIndexProducerIamProvisioningPlan {
        artifact_kind: spec.artifact_kind,
        ssm_parameter_paths: ArtifactIndexProducerSsmParameterPaths {
            access_key_id: format!("{ssm_prefix}/{kind_path}/access-key-id"),
            secret_access_key: format!("{ssm_prefix}/{kind_path}/secret-access-key"),
            session_token: None,
        },
        policy,
        expected_denied_write_attempts: spec.denied_artifact_kinds.len()
            * crate::artifact_index_commit_proof::ArtifactIndexIamProbePathKind::ALL.len(),
        proof_denied_artifact_kinds: spec.denied_artifact_kinds,
    })
}

pub fn artifact_index_producer_iam_policy(
    artifact_root: &str,
    artifact_kind: ArtifactKind,
    proof_artifact_roots: &[&str],
) -> Result<ArtifactIndexProducerIamPolicy, ArtifactIndexIamPolicyError> {
    let mut statements = vec![producer_statement(
        "AllowArtifactIndexCommitProductionRoot",
        artifact_root,
        artifact_kind,
    )?];
    for (index, proof_root) in proof_artifact_roots.iter().enumerate() {
        statements.push(producer_statement(
            &format!("AllowArtifactIndexCommitProofRoot{index}"),
            proof_root,
            artifact_kind,
        )?);
    }
    Ok(ArtifactIndexProducerIamPolicy {
        version: "2012-10-17".to_string(),
        statements,
    })
}

fn producer_statement(
    sid: &str,
    artifact_root: &str,
    artifact_kind: ArtifactKind,
) -> Result<ArtifactIndexProducerIamStatement, ArtifactIndexIamPolicyError> {
    let s3_root = S3ArtifactRoot::parse(artifact_root)?;
    let kind = artifact_kind.as_str();
    Ok(ArtifactIndexProducerIamStatement {
        sid: sid.to_string(),
        effect: "Allow".to_string(),
        actions: vec!["s3:GetObject".to_string(), "s3:PutObject".to_string()],
        resources: vec![
            s3_root.resource_arn(&format!("artifact-index/v1/events/kind={kind}/*")),
            s3_root.resource_arn(&format!("artifact-index/v1/snapshots/kind={kind}/*")),
            s3_root.resource_arn(&format!(
                "artifact-index/v1/pointers/kind={kind}/latest.json"
            )),
            s3_root.resource_arn("artifact-index/v1/audit/epochs/*"),
        ],
    })
}

fn validate_denied_kinds(
    artifact_kind: ArtifactKind,
    denied_artifact_kinds: &[ArtifactKind],
) -> Result<(), ArtifactIndexIamPolicyError> {
    if denied_artifact_kinds.contains(&artifact_kind) {
        Err(ArtifactIndexIamPolicyError::DeniedArtifactKindIncludesProducerKind { artifact_kind })
    } else {
        Ok(())
    }
}

fn validate_ssm_parameter_prefix(prefix: &str) -> Result<String, ArtifactIndexIamPolicyError> {
    let whitespace_trimmed = prefix.trim();
    if whitespace_trimmed != prefix {
        return Err(ArtifactIndexIamPolicyError::InvalidSsmParameterPrefix {
            prefix: prefix.to_string(),
            reason: "SSM parameter prefix contains leading or trailing whitespace",
        });
    }
    let trimmed = whitespace_trimmed.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(ArtifactIndexIamPolicyError::InvalidSsmParameterPrefix {
            prefix: prefix.to_string(),
            reason: "SSM parameter prefix is empty",
        });
    }
    if !trimmed.starts_with('/') {
        return Err(ArtifactIndexIamPolicyError::InvalidSsmParameterPrefix {
            prefix: prefix.to_string(),
            reason: "expected absolute SSM parameter prefix starting with /",
        });
    }
    if trimmed.contains(char::is_whitespace) {
        return Err(ArtifactIndexIamPolicyError::InvalidSsmParameterPrefix {
            prefix: prefix.to_string(),
            reason: "SSM parameter prefix contains whitespace",
        });
    }
    if trimmed.contains('*') {
        return Err(ArtifactIndexIamPolicyError::InvalidSsmParameterPrefix {
            prefix: prefix.to_string(),
            reason: "SSM parameter prefix cannot contain wildcard characters",
        });
    }
    if !trimmed.ends_with("/artifact-index/producers") {
        return Err(ArtifactIndexIamPolicyError::InvalidSsmParameterPrefix {
            prefix: prefix.to_string(),
            reason: "expected Artifact Index producer SSM parameter prefix ending with /artifact-index/producers",
        });
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct S3ArtifactRoot {
    bucket: String,
    prefix: String,
}

impl S3ArtifactRoot {
    fn parse(uri: &str) -> Result<Self, ArtifactIndexIamPolicyError> {
        let Some(rest) = uri.strip_prefix("s3://") else {
            return Err(ArtifactIndexIamPolicyError::UnsupportedArtifactRoot {
                artifact_root: uri.to_string(),
            });
        };
        let (bucket, prefix) = rest.split_once('/').ok_or_else(|| {
            ArtifactIndexIamPolicyError::UnsupportedArtifactRoot {
                artifact_root: uri.to_string(),
            }
        })?;
        validate_non_empty("bucket", bucket, uri)?;
        validate_non_empty("prefix", prefix, uri)?;
        Ok(Self {
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
        })
    }

    fn resource_arn(&self, relative_path: &str) -> String {
        format!(
            "arn:aws:s3:::{}/{}/{}",
            self.bucket,
            self.prefix,
            relative_path.trim_start_matches('/')
        )
    }
}

fn validate_non_empty(
    field: &'static str,
    value: &str,
    artifact_root: &str,
) -> Result<(), ArtifactIndexIamPolicyError> {
    if value.trim().is_empty() {
        Err(ArtifactIndexIamPolicyError::EmptyField {
            field,
            artifact_root: artifact_root.to_string(),
        })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactIndexIamPolicyError {
    UnsupportedArtifactRoot {
        artifact_root: String,
    },
    EmptyField {
        field: &'static str,
        artifact_root: String,
    },
    InvalidSsmParameterPrefix {
        prefix: String,
        reason: &'static str,
    },
    DeniedArtifactKindIncludesProducerKind {
        artifact_kind: ArtifactKind,
    },
}

impl fmt::Display for ArtifactIndexIamPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedArtifactRoot { artifact_root } => {
                write!(
                    f,
                    "artifact_root must be an s3 URI with bucket and prefix: {artifact_root}"
                )
            }
            Self::EmptyField {
                field,
                artifact_root,
            } => {
                write!(f, "{field} is empty in artifact_root {artifact_root}")
            }
            Self::InvalidSsmParameterPrefix { prefix, reason } => {
                write!(f, "{reason}: {prefix}")
            }
            Self::DeniedArtifactKindIncludesProducerKind { artifact_kind } => {
                write!(
                    f,
                    "denied_artifact_kinds cannot include producer artifact_kind {}",
                    artifact_kind.as_str()
                )
            }
        }
    }
}

impl Error for ArtifactIndexIamPolicyError {}
