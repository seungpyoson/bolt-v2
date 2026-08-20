//! Canonical immutable intent for an Artifact Index latest-pointer mutation.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ARTIFACT_INDEX_AUDIT_INTENT_V1_SCHEMA_VERSION: &str = "artifact-index-audit-intent.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactIndexAuditKind {
    Raw,
    NtCatalog,
    SourceProofs,
    Backtests,
    ArtifactIndex,
    ResearchAnalytics,
}

impl ArtifactIndexAuditKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::NtCatalog => "nt-catalog",
            Self::SourceProofs => "source-proofs",
            Self::Backtests => "backtests",
            Self::ArtifactIndex => "artifact-index",
            Self::ResearchAnalytics => "research-analytics",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactIndexAuditPrecondition {
    IfNoneMatchAny,
    IfMatch {
        etag: Option<String>,
        version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Written create-only before the authoritative latest-pointer CAS. The
/// pointer determines whether this intent committed.
pub struct ArtifactIndexAuditIntentV1 {
    pub schema_version: String,
    pub audit_intent_id: String,
    pub artifact_kind: ArtifactIndexAuditKind,
    pub latest_pointer_uri: String,
    pub prior_snapshot_id: Option<String>,
    pub new_snapshot_id: String,
    pub new_snapshot_uri: String,
    pub new_snapshot_content_hash: String,
    pub writer_id: String,
    pub precondition: ArtifactIndexAuditPrecondition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIndexAuditIntentIdentity {
    pub artifact_kind: ArtifactIndexAuditKind,
    pub latest_pointer_uri: String,
    pub prior_snapshot_id: Option<String>,
    pub new_snapshot_id: String,
    pub new_snapshot_uri: String,
    pub new_snapshot_content_hash: String,
    pub writer_id: String,
    pub precondition: ArtifactIndexAuditPrecondition,
}

#[derive(Serialize)]
struct ArtifactIndexAuditIntentHashIdentity<'a> {
    schema_version: &'static str,
    artifact_kind: ArtifactIndexAuditKind,
    latest_pointer_uri: &'a str,
    prior_snapshot_id: Option<&'a str>,
    new_snapshot_id: &'a str,
    new_snapshot_uri: &'a str,
    new_snapshot_content_hash: &'a str,
    writer_id: &'a str,
    precondition: &'a ArtifactIndexAuditPrecondition,
}

impl ArtifactIndexAuditIntentV1 {
    /// # Errors
    ///
    /// Returns an error if the identity does not describe a canonical pointer
    /// mutation or cannot be serialized.
    pub fn new(
        identity: ArtifactIndexAuditIntentIdentity,
    ) -> Result<Self, ArtifactIndexAuditError> {
        let audit_intent_id = audit_intent_id(&identity)?;
        let intent = Self {
            schema_version: ARTIFACT_INDEX_AUDIT_INTENT_V1_SCHEMA_VERSION.to_string(),
            audit_intent_id,
            artifact_kind: identity.artifact_kind,
            latest_pointer_uri: identity.latest_pointer_uri,
            prior_snapshot_id: identity.prior_snapshot_id,
            new_snapshot_id: identity.new_snapshot_id,
            new_snapshot_uri: identity.new_snapshot_uri,
            new_snapshot_content_hash: identity.new_snapshot_content_hash,
            writer_id: identity.writer_id,
            precondition: identity.precondition,
        };
        intent.validate()?;
        Ok(intent)
    }

    /// # Errors
    ///
    /// Returns an error if the wire value is not a canonical v1 intent or its
    /// identifier does not bind the exact CAS tuple.
    pub fn validate(&self) -> Result<(), ArtifactIndexAuditError> {
        if self.schema_version != ARTIFACT_INDEX_AUDIT_INTENT_V1_SCHEMA_VERSION {
            return Err(ArtifactIndexAuditError::Invalid(
                "unsupported audit intent schema".to_string(),
            ));
        }
        validate_sha256("audit_intent_id", &self.audit_intent_id)?;
        validate_sha256("new_snapshot_content_hash", &self.new_snapshot_content_hash)?;
        validate_non_empty("latest_pointer_uri", &self.latest_pointer_uri)?;
        validate_non_empty("new_snapshot_id", &self.new_snapshot_id)?;
        validate_non_empty("new_snapshot_uri", &self.new_snapshot_uri)?;
        validate_non_empty("writer_id", &self.writer_id)?;
        if let Some(prior_snapshot_id) = &self.prior_snapshot_id {
            validate_non_empty("prior_snapshot_id", prior_snapshot_id)?;
        }
        match (&self.prior_snapshot_id, &self.precondition) {
            (None, ArtifactIndexAuditPrecondition::IfNoneMatchAny) => {}
            (Some(_), ArtifactIndexAuditPrecondition::IfMatch { etag, version }) => {
                if etag.is_none() && version.is_none() {
                    return Err(ArtifactIndexAuditError::Invalid(
                        "if-match precondition requires an ETag or version".to_string(),
                    ));
                }
                if let Some(etag) = etag {
                    validate_non_empty("precondition.etag", etag)?;
                }
                if let Some(version) = version {
                    validate_non_empty("precondition.version", version)?;
                }
            }
            _ => {
                return Err(ArtifactIndexAuditError::Invalid(
                    "prior snapshot and pointer precondition disagree".to_string(),
                ));
            }
        }
        let expected_id = audit_intent_id(&self.identity())?;
        if self.audit_intent_id != expected_id {
            return Err(ArtifactIndexAuditError::Invalid(
                "audit intent id does not content-address the exact CAS tuple".to_string(),
            ));
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when validation or serialization fails.
    pub fn bytes(&self) -> Result<Vec<u8>, ArtifactIndexAuditError> {
        self.validate()?;
        serde_json::to_vec(self)
            .map_err(|error| ArtifactIndexAuditError::Serialize(error.to_string()))
    }

    fn identity(&self) -> ArtifactIndexAuditIntentIdentity {
        ArtifactIndexAuditIntentIdentity {
            artifact_kind: self.artifact_kind,
            latest_pointer_uri: self.latest_pointer_uri.clone(),
            prior_snapshot_id: self.prior_snapshot_id.clone(),
            new_snapshot_id: self.new_snapshot_id.clone(),
            new_snapshot_uri: self.new_snapshot_uri.clone(),
            new_snapshot_content_hash: self.new_snapshot_content_hash.clone(),
            writer_id: self.writer_id.clone(),
            precondition: self.precondition.clone(),
        }
    }
}

fn audit_intent_id(
    identity: &ArtifactIndexAuditIntentIdentity,
) -> Result<String, ArtifactIndexAuditError> {
    let hash_identity = ArtifactIndexAuditIntentHashIdentity {
        schema_version: ARTIFACT_INDEX_AUDIT_INTENT_V1_SCHEMA_VERSION,
        artifact_kind: identity.artifact_kind,
        latest_pointer_uri: &identity.latest_pointer_uri,
        prior_snapshot_id: identity.prior_snapshot_id.as_deref(),
        new_snapshot_id: &identity.new_snapshot_id,
        new_snapshot_uri: &identity.new_snapshot_uri,
        new_snapshot_content_hash: &identity.new_snapshot_content_hash,
        writer_id: &identity.writer_id,
        precondition: &identity.precondition,
    };
    let bytes = serde_json::to_vec(&hash_identity)
        .map_err(|error| ArtifactIndexAuditError::Serialize(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), ArtifactIndexAuditError> {
    if value.trim().is_empty() {
        Err(ArtifactIndexAuditError::Invalid(format!(
            "{field} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), ArtifactIndexAuditError> {
    if value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ArtifactIndexAuditError::Invalid(format!(
            "{field} must be a lowercase sha256 digest"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactIndexAuditError {
    Invalid(String),
    Serialize(String),
}

impl fmt::Display for ArtifactIndexAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => {
                write!(formatter, "invalid Artifact Index audit intent: {message}")
            }
            Self::Serialize(message) => {
                write!(
                    formatter,
                    "serialize Artifact Index audit intent: {message}"
                )
            }
        }
    }
}

impl Error for ArtifactIndexAuditError {}
