//! IAM policy generation for per-kind Artifact Index producers.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::artifact_index::ArtifactKind;

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
        }
    }
}

impl Error for ArtifactIndexIamPolicyError {}
