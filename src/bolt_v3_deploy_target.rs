//! Deploy target binding and the ops-launch `TargetVerify` stage.
//!
//! A deploy target is the specific cloud host this bolt-v3 launch is allowed
//! to run on, expressed as an optional region / availability-zone /
//! instance-id binding in tracked `deploy.toml`. The `TargetVerify` stage
//! proves the running host matches that binding before secrets or any runtime
//! side effects, so a misconfigured launch cannot resolve credentials on the
//! wrong box.
//!
//! Degrade vs fail-closed contract (single source of truth):
//!   - No `deploy.toml`, no `[target]`, or all of region / availability_zone /
//!     instance_id absent  =>  [`TargetVerifyOutcome::NoTargetConfigured`].
//!     The host is never observed; launch proceeds. This lets the lane work
//!     before any instance is provisioned.
//!   - Otherwise the configured fields are compared against live host facts
//!     read over IMDSv2. Any mismatch  =>
//!     [`TargetVerifyOutcome::Mismatched`]. An unreadable host  =>  `Err`.
//!     Both are fail-closed: the caller stops the launch.
//!   - `name_tag` is informational only (basic IMDSv2 metadata does not expose
//!     instance tags), so it never gates the outcome in this slice.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::bounded_config_read::{self, ConfigFileReadError};

/// File name, relative to the ops `config_root`, that carries the deploy
/// target binding. Sits alongside `live.toml` but is a separate load path:
/// it is NOT part of `BoltV3RootConfig`, the live-config bundle, or the
/// config-bundle checksum.
const DEPLOY_TARGET_FILE_NAME: &str = "deploy.toml";

/// Standard IMDSv2 metadata paths. The IMDS client performs the token +
/// metadata handshake internally; these are the leaf paths it reads.
const IMDS_INSTANCE_ID_PATH: &str = "/latest/meta-data/instance-id";
const IMDS_AVAILABILITY_ZONE_PATH: &str = "/latest/meta-data/placement/availability-zone";
const IMDS_REGION_PATH: &str = "/latest/meta-data/placement/region";

/// Short fetch budget for the IMDSv2 handshake. Kept small because the launch
/// lane must fail closed quickly when the host facts are unreachable rather
/// than stalling startup.
const IMDS_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Parsed `deploy.toml`. The `[target]` table is optional so an absent or
/// empty binding is a first-class "no target configured" state.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployTargetConfig {
    pub target: Option<TargetBinding>,
}

/// Optional deploy target binding. Every field is optional; a field left unset
/// is simply not enforced by `TargetVerify`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBinding {
    pub region: Option<String>,
    pub availability_zone: Option<String>,
    pub instance_id: Option<String>,
    pub name_tag: Option<String>,
}

impl TargetBinding {
    /// A binding gates launch only if at least one of the observable identity
    /// fields (region / availability_zone / instance_id) is set. `name_tag`
    /// alone is informational and does not arm verification.
    fn has_gating_field(&self) -> bool {
        self.region.is_some() || self.availability_zone.is_some() || self.instance_id.is_some()
    }
}

/// Host facts observed from the running instance. Each field is optional
/// because a given metadata path may be unavailable on some hosts; an absent
/// observed value for a configured field is treated as a mismatch (fail
/// closed) rather than a silent pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedHostFacts {
    pub region: Option<String>,
    pub availability_zone: Option<String>,
    pub instance_id: Option<String>,
}

/// One configured field that did not match the observed host fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMismatch {
    pub field: &'static str,
    pub configured: String,
    pub observed: Option<String>,
}

/// Outcome of comparing the configured binding against the running host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetVerifyOutcome {
    /// No gating binding configured; the host was not observed.
    NoTargetConfigured,
    /// Every configured field matched the observed host facts.
    Matched,
    /// At least one configured field did not match.
    Mismatched(Vec<FieldMismatch>),
}

/// Errors raised while loading or verifying the deploy target.
#[derive(Debug)]
pub enum DeployTargetError {
    /// `deploy.toml` exists but could not be read.
    Read(ConfigFileReadError),
    /// `deploy.toml` could not be parsed as a valid deploy target.
    Parse {
        path: String,
        source: toml::de::Error,
    },
    /// The host facts could not be observed (fail closed: we cannot prove the
    /// running host is the configured target).
    Observe(String),
}

impl std::fmt::Display for DeployTargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(source) => write!(f, "failed to read deploy target config: {source}"),
            Self::Parse { path, source } => {
                write!(f, "failed to parse deploy target config {path}: {source}")
            }
            Self::Observe(message) => {
                write!(f, "failed to observe deploy host facts: {message}")
            }
        }
    }
}

impl std::error::Error for DeployTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(source) => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Observe(_) => None,
        }
    }
}

/// Source of live host facts. Pluggable so the deploy actor (#880) and tests
/// can inject their own implementation; tests must use a fake source so no
/// network call happens.
pub trait HostFactsSource {
    fn observe(&self) -> Result<ObservedHostFacts, DeployTargetError>;
}

/// Load `deploy.toml` from `<config_root>/deploy.toml`.
///
/// Uses the same bounded reader as the live-config load path (single reader).
/// A missing file degrades to `Ok(DeployTargetConfig { target: None })`; a
/// present-but-unreadable or unparseable file fails loud.
pub fn load_deploy_target(config_root: &Path) -> Result<DeployTargetConfig, DeployTargetError> {
    let path = config_root.join(DEPLOY_TARGET_FILE_NAME);
    let contents = match bounded_config_read::read_to_string(&path) {
        Ok(contents) => contents,
        // A missing file is the documented "no target configured" degrade.
        Err(ConfigFileReadError::Open { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(DeployTargetConfig::default());
        }
        Err(error) => return Err(DeployTargetError::Read(error)),
    };
    toml::from_str(&contents).map_err(|source| DeployTargetError::Parse {
        path: path.display().to_string(),
        source,
    })
}

/// Compare the configured deploy target against the running host.
///
/// Observes the host only when a gating binding is present, so the verifier
/// works offline before any instance exists.
pub fn verify_deploy_target(
    config: &DeployTargetConfig,
    source: &dyn HostFactsSource,
) -> Result<TargetVerifyOutcome, DeployTargetError> {
    let Some(binding) = config
        .target
        .as_ref()
        .filter(|binding| binding.has_gating_field())
    else {
        return Ok(TargetVerifyOutcome::NoTargetConfigured);
    };

    let observed = source.observe()?;

    let mut mismatches = Vec::new();
    compare_field(
        "region",
        binding.region.as_deref(),
        observed.region.as_deref(),
        &mut mismatches,
    );
    compare_field(
        "availability_zone",
        binding.availability_zone.as_deref(),
        observed.availability_zone.as_deref(),
        &mut mismatches,
    );
    compare_field(
        "instance_id",
        binding.instance_id.as_deref(),
        observed.instance_id.as_deref(),
        &mut mismatches,
    );

    if mismatches.is_empty() {
        Ok(TargetVerifyOutcome::Matched)
    } else {
        Ok(TargetVerifyOutcome::Mismatched(mismatches))
    }
}

/// Record a mismatch when a configured field differs from (or is missing in)
/// the observed facts. Unconfigured fields are skipped.
fn compare_field(
    field: &'static str,
    configured: Option<&str>,
    observed: Option<&str>,
    mismatches: &mut Vec<FieldMismatch>,
) {
    let Some(configured) = configured else {
        return;
    };
    if observed != Some(configured) {
        mismatches.push(FieldMismatch {
            field,
            configured: configured.to_string(),
            observed: observed.map(str::to_string),
        });
    }
}

/// Production [`HostFactsSource`] backed by the EC2 Instance Metadata Service
/// (IMDSv2). Owns a current-thread Tokio runtime to bridge the AWS SDK's async
/// IMDS API from the synchronous startup boundary, mirroring
/// `secrets::SsmResolverSession`. The runtime is built per `observe` call (the
/// launch lane observes at most once), and the same no-nested-runtime
/// guarantee holds because `observe` runs on the synchronous startup thread.
pub struct Imdsv2HostFactsSource;

impl Imdsv2HostFactsSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Imdsv2HostFactsSource {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFactsSource for Imdsv2HostFactsSource {
    fn observe(&self) -> Result<ObservedHostFacts, DeployTargetError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                DeployTargetError::Observe(format!(
                    "failed to build Tokio runtime for IMDSv2 host facts: {error}"
                ))
            })?;

        runtime.block_on(async {
            let client = aws_config::imds::Client::builder()
                .connect_timeout(IMDS_FETCH_TIMEOUT)
                .read_timeout(IMDS_FETCH_TIMEOUT)
                .operation_timeout(IMDS_FETCH_TIMEOUT)
                .build();

            let instance_id = fetch_metadata(&client, IMDS_INSTANCE_ID_PATH).await?;
            let availability_zone = fetch_metadata(&client, IMDS_AVAILABILITY_ZONE_PATH).await?;
            let region = fetch_metadata(&client, IMDS_REGION_PATH).await?;

            Ok(ObservedHostFacts {
                region,
                availability_zone,
                instance_id,
            })
        })
    }
}

/// Fetch one IMDS metadata leaf, mapping a not-found to `None` and any other
/// failure to a fail-closed `DeployTargetError`.
async fn fetch_metadata(
    client: &aws_config::imds::Client,
    path: &'static str,
) -> Result<Option<String>, DeployTargetError> {
    match client.get(path).await {
        Ok(value) => Ok(Some(value.as_ref().trim().to_string())),
        Err(error) => Err(DeployTargetError::Observe(format!(
            "IMDSv2 metadata fetch failed for {path}: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake host-facts source for unit tests: never touches the network. It
    /// either returns canned facts or a fixed observe error.
    struct FakeHostFactsSource {
        result: Result<ObservedHostFacts, String>,
    }

    impl FakeHostFactsSource {
        fn facts(facts: ObservedHostFacts) -> Self {
            Self { result: Ok(facts) }
        }

        fn erroring() -> Self {
            Self {
                result: Err("fake source must not be observed in this case".to_string()),
            }
        }
    }

    impl HostFactsSource for FakeHostFactsSource {
        fn observe(&self) -> Result<ObservedHostFacts, DeployTargetError> {
            self.result.clone().map_err(DeployTargetError::Observe)
        }
    }

    fn binding(region: &str, az: &str, instance_id: &str) -> TargetBinding {
        TargetBinding {
            region: Some(region.to_string()),
            availability_zone: Some(az.to_string()),
            instance_id: Some(instance_id.to_string()),
            name_tag: None,
        }
    }

    fn facts(region: &str, az: &str, instance_id: &str) -> ObservedHostFacts {
        ObservedHostFacts {
            region: Some(region.to_string()),
            availability_zone: Some(az.to_string()),
            instance_id: Some(instance_id.to_string()),
        }
    }

    #[test]
    fn no_target_configured_does_not_observe_even_with_erroring_source() {
        let config = DeployTargetConfig { target: None };
        let source = FakeHostFactsSource::erroring();

        let outcome = verify_deploy_target(&config, &source)
            .expect("absent target must short-circuit before observing");

        assert_eq!(outcome, TargetVerifyOutcome::NoTargetConfigured);
    }

    #[test]
    fn name_tag_only_binding_is_not_gating() {
        let config = DeployTargetConfig {
            target: Some(TargetBinding {
                name_tag: Some("informational-only".to_string()),
                ..TargetBinding::default()
            }),
        };
        let source = FakeHostFactsSource::erroring();

        let outcome = verify_deploy_target(&config, &source)
            .expect("a name_tag-only binding must not arm verification");

        assert_eq!(outcome, TargetVerifyOutcome::NoTargetConfigured);
    }

    #[test]
    fn configured_and_matching_facts_yield_matched() {
        let config = DeployTargetConfig {
            target: Some(binding("region-x", "region-x-zone-a", "instance-target")),
        };
        let source =
            FakeHostFactsSource::facts(facts("region-x", "region-x-zone-a", "instance-target"));

        let outcome = verify_deploy_target(&config, &source).expect("matching facts must verify");

        assert_eq!(outcome, TargetVerifyOutcome::Matched);
    }

    #[test]
    fn configured_and_differing_instance_id_yields_mismatch() {
        let config = DeployTargetConfig {
            target: Some(binding("region-x", "region-x-zone-a", "instance-target")),
        };
        let source =
            FakeHostFactsSource::facts(facts("region-x", "region-x-zone-a", "instance-other"));

        let outcome = verify_deploy_target(&config, &source)
            .expect("a differing instance id must be a clean Mismatched, not an error");

        assert_eq!(
            outcome,
            TargetVerifyOutcome::Mismatched(vec![FieldMismatch {
                field: "instance_id",
                configured: "instance-target".to_string(),
                observed: Some("instance-other".to_string()),
            }])
        );
    }

    #[test]
    fn configured_field_absent_from_observed_facts_is_a_mismatch() {
        let config = DeployTargetConfig {
            target: Some(TargetBinding {
                instance_id: Some("instance-target".to_string()),
                ..TargetBinding::default()
            }),
        };
        let source = FakeHostFactsSource::facts(ObservedHostFacts::default());

        let outcome = verify_deploy_target(&config, &source)
            .expect("a missing observed value is a mismatch, not an error");

        assert_eq!(
            outcome,
            TargetVerifyOutcome::Mismatched(vec![FieldMismatch {
                field: "instance_id",
                configured: "instance-target".to_string(),
                observed: None,
            }])
        );
    }

    #[test]
    fn observe_error_with_configured_target_fails_closed() {
        let config = DeployTargetConfig {
            target: Some(binding("region-x", "region-x-zone-a", "instance-target")),
        };
        let source = FakeHostFactsSource::erroring();

        let error = verify_deploy_target(&config, &source)
            .expect_err("an unobservable host must fail closed when a target is configured");

        assert!(matches!(error, DeployTargetError::Observe(_)));
    }

    #[test]
    fn load_deploy_target_missing_file_degrades_to_none() {
        let temp = tempfile::tempdir().expect("tempdir should create");

        let config = load_deploy_target(temp.path())
            .expect("a missing deploy.toml must degrade to no target configured");

        assert!(config.target.is_none());
    }

    #[test]
    fn load_deploy_target_parses_valid_target() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(
            temp.path().join(DEPLOY_TARGET_FILE_NAME),
            "[target]\nregion = \"region-x\"\ninstance_id = \"instance-target\"\n",
        )
        .expect("deploy.toml fixture should write");

        let config = load_deploy_target(temp.path()).expect("a valid deploy.toml must parse");

        let binding = config.target.expect("target table must be present");
        assert_eq!(binding.region.as_deref(), Some("region-x"));
        assert_eq!(binding.instance_id.as_deref(), Some("instance-target"));
        assert!(binding.availability_zone.is_none());
        assert!(binding.name_tag.is_none());
    }

    #[test]
    fn load_deploy_target_empty_target_table_is_no_target() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(temp.path().join(DEPLOY_TARGET_FILE_NAME), "[target]\n")
            .expect("deploy.toml fixture should write");

        let config = load_deploy_target(temp.path())
            .expect("an empty [target] table must parse to a non-gating binding");

        let outcome = verify_deploy_target(&config, &FakeHostFactsSource::erroring())
            .expect("an empty [target] table must degrade to no target configured");
        assert_eq!(outcome, TargetVerifyOutcome::NoTargetConfigured);
    }

    #[test]
    fn load_deploy_target_rejects_unknown_field() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(
            temp.path().join(DEPLOY_TARGET_FILE_NAME),
            "[target]\nunexpected_field = \"x\"\n",
        )
        .expect("deploy.toml fixture should write");

        let error = load_deploy_target(temp.path())
            .expect_err("an unknown field under [target] must be rejected");

        assert!(matches!(error, DeployTargetError::Parse { .. }));
    }
}
