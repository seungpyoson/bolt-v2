//! Deploy target binding and the ops-launch `TargetVerify` stage.
//!
//! A deploy target is the specific cloud host this bolt-v3 launch is allowed
//! to run on, expressed as an optional region / availability-zone /
//! instance-id binding in tracked `deploy.toml`. The `TargetVerify` stage
//! proves the running host matches that binding before secrets or any runtime
//! side effects, so a misconfigured launch cannot resolve credentials on the
//! wrong box.
//!
//! Diagnostic vs fail-closed contract (single source of truth):
//!   - No `deploy.toml`, no `[target]`, or all of region / availability_zone /
//!     instance_id absent  =>  [`TargetVerifyOutcome::NoTargetConfigured`].
//!     The host is never observed. Status may report this diagnostic outcome,
//!     but the launch caller fails before secrets or Start.
//!   - Otherwise the configured fields are compared against live host facts
//!     read over IMDSv2. Any mismatch  =>
//!     [`TargetVerifyOutcome::Mismatched`]. An unreadable host  =>  `Err`.
//!     Both are fail-closed: the caller stops the launch.
//!   - `name_tag` is informational only (basic IMDSv2 metadata does not expose
//!     instance tags), so it never gates the outcome in this slice.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

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

/// Fetch budget for the IMDSv2 handshake. This is a deliberate fixed fail-fast
/// ceiling on the link-local IMDSv2 token + metadata handshake: the service is
/// reachable over the link-local address with no network hops, so a short fixed
/// preflight bound is the right shape and the launch lane fails closed quickly
/// when host facts are unreachable instead of stalling startup. The value is
/// NOT asserted to be operator-irrelevant; if operations ever observe
/// transient-slowness false-fails, promoting it to a configured value is
/// deferred to a follow-up rather than presumed unnecessary here.
const IMDS_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Parsed `deploy.toml`. The `[target]` table is optional so status can report
/// an absent or empty binding as a first-class diagnostic state; launch still
/// requires an observable binding.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployTargetConfig {
    pub target: Option<TargetBinding>,
}

/// Optional deploy target binding. Every field is optional; a field left unset
/// is simply not enforced by `TargetVerify`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBinding {
    pub region: Option<String>,
    pub availability_zone: Option<String>,
    pub instance_id: Option<String>,
    pub name_tag: Option<String>,
}

impl TargetBinding {
    /// Observable identity fields arm IMDS observation and comparison. Without
    /// one, verification returns `NoTargetConfigured`, which `ops launch`
    /// rejects. `name_tag` alone is diagnostic-only and is not compared.
    fn has_gating_field(&self) -> bool {
        [
            self.region.as_deref(),
            self.availability_zone.as_deref(),
            self.instance_id.as_deref(),
        ]
        .into_iter()
        .any(|value| configured_identity_value(value).is_some())
    }
}

fn configured_identity_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// Host facts observed from the running instance. Each field is optional
/// because a given metadata path may be unavailable on some hosts; an absent
/// observed value for a configured field is treated as a mismatch (fail
/// closed) rather than a silent pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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

/// Result of [`verify_deploy_target`]: the verification outcome plus the host
/// facts that were observed to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployTargetVerification {
    pub outcome: TargetVerifyOutcome,
    /// The host facts observed during verification. `Some` only when a gating
    /// binding was present (and therefore the host was observed); `None` for
    /// `NoTargetConfigured` (no IMDS call — keeps the verifier offline-safe).
    pub observed_host_facts: Option<ObservedHostFacts>,
}

/// Errors raised while loading or verifying the deploy target.
#[derive(Debug)]
pub enum DeployTargetError {
    /// `deploy.toml` exists but could not be read.
    Read {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
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
            Self::Read { path, source } => {
                write!(
                    f,
                    "failed to read deploy target config {}: {source}",
                    path.display()
                )
            }
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
            Self::Read { source, .. } => Some(source.as_ref()),
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
            return Ok(DeployTargetConfig { target: None });
        }
        Err(error) => {
            return Err(DeployTargetError::Read {
                path: path.clone(),
                source: Box::new(error),
            });
        }
    };
    toml::from_str(&contents).map_err(|source| DeployTargetError::Parse {
        path: path.display().to_string(),
        source,
    })
}

/// Compare the configured deploy target against the running host.
///
/// Observes the host only when a gating binding is present. Without one it
/// returns the diagnostic-only `NoTargetConfigured` outcome for the caller to
/// reject or render as status.
pub fn verify_deploy_target(
    config: &DeployTargetConfig,
    source: &dyn HostFactsSource,
) -> Result<DeployTargetVerification, DeployTargetError> {
    let Some(binding) = config
        .target
        .as_ref()
        .filter(|binding| binding.has_gating_field())
    else {
        return Ok(DeployTargetVerification {
            outcome: TargetVerifyOutcome::NoTargetConfigured,
            observed_host_facts: None,
        });
    };

    let observed = source.observe()?;

    let mut mismatches = Vec::new();
    compare_field(
        stringify!(region),
        configured_identity_value(binding.region.as_deref()),
        observed.region.as_deref(),
        &mut mismatches,
    );
    compare_field(
        stringify!(availability_zone),
        configured_identity_value(binding.availability_zone.as_deref()),
        observed.availability_zone.as_deref(),
        &mut mismatches,
    );
    compare_field(
        stringify!(instance_id),
        configured_identity_value(binding.instance_id.as_deref()),
        observed.instance_id.as_deref(),
        &mut mismatches,
    );

    let outcome = if mismatches.is_empty() {
        TargetVerifyOutcome::Matched
    } else {
        TargetVerifyOutcome::Mismatched(mismatches)
    };
    Ok(DeployTargetVerification {
        outcome,
        observed_host_facts: Some(observed),
    })
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

// The bolt-v3 legacy-default fence forbids a `Default` impl on the production
// surface, so the no-argument `new` is sanctioned with an explicit allow rather
// than satisfying `clippy::new_without_default` by adding a forbidden `Default`.
#[allow(clippy::new_without_default)]
impl Imdsv2HostFactsSource {
    pub fn new() -> Self {
        Self
    }

    /// Reject `observe` when called from inside an active Tokio runtime,
    /// mirroring `secrets::SsmResolverSession::ensure_not_inside_active_tokio_runtime`.
    /// `observe` builds and `block_on`s a current-thread runtime; Tokio panics
    /// if `block_on` runs inside another runtime's task. Host-facts observation
    /// must run on the synchronous startup boundary (the same one SSM secret
    /// resolution uses), before any NT runtime is built, so a same-thread
    /// misuse converts to a structured `DeployTargetError::Observe` instead of
    /// a runtime panic.
    fn ensure_not_inside_active_tokio_runtime() -> Result<(), DeployTargetError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(DeployTargetError::Observe(
                "Imdsv2HostFactsSource invoked from inside an active Tokio \
                 runtime; deploy-target host-facts observation must run on the \
                 synchronous startup boundary, before any NT runtime is built"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

impl HostFactsSource for Imdsv2HostFactsSource {
    fn observe(&self) -> Result<ObservedHostFacts, DeployTargetError> {
        Self::ensure_not_inside_active_tokio_runtime()?;
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
            let leaf_source = AwsImdsMetadataLeafSource { client };

            observe_imdsv2_host_facts(&leaf_source).await
        })
    }
}

trait ImdsMetadataLeafSource {
    async fn fetch_metadata(&self, path: &'static str)
    -> Result<Option<String>, DeployTargetError>;
}

struct AwsImdsMetadataLeafSource {
    client: aws_config::imds::Client,
}

impl ImdsMetadataLeafSource for AwsImdsMetadataLeafSource {
    async fn fetch_metadata(
        &self,
        path: &'static str,
    ) -> Result<Option<String>, DeployTargetError> {
        fetch_metadata(&self.client, path).await
    }
}

async fn observe_imdsv2_host_facts(
    source: &impl ImdsMetadataLeafSource,
) -> Result<ObservedHostFacts, DeployTargetError> {
    let (instance_id, availability_zone, region) = tokio::try_join!(
        source.fetch_metadata(IMDS_INSTANCE_ID_PATH),
        source.fetch_metadata(IMDS_AVAILABILITY_ZONE_PATH),
        source.fetch_metadata(IMDS_REGION_PATH),
    )?;

    Ok(ObservedHostFacts {
        region,
        availability_zone,
        instance_id,
    })
}

/// Fetch one IMDS metadata leaf. On success returns the trimmed value as
/// `Some`; any failure — including a missing/not-found path — maps to a
/// fail-closed `DeployTargetError::Observe`, because an unobservable host
/// fact means we cannot prove the host is the configured target, so the
/// launch must stop.
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

        let verification = verify_deploy_target(&config, &source)
            .expect("absent target must short-circuit before observing");

        assert_eq!(
            verification.outcome,
            TargetVerifyOutcome::NoTargetConfigured
        );
        assert_eq!(verification.observed_host_facts, None);
    }

    #[test]
    fn name_tag_only_binding_is_not_gating() {
        let config = DeployTargetConfig {
            target: Some(TargetBinding {
                name_tag: Some("informational-only".to_string()),
                region: None,
                availability_zone: None,
                instance_id: None,
            }),
        };
        let source = FakeHostFactsSource::erroring();

        let verification = verify_deploy_target(&config, &source)
            .expect("a name_tag-only binding must not arm verification");

        assert_eq!(
            verification.outcome,
            TargetVerifyOutcome::NoTargetConfigured
        );
        assert_eq!(verification.observed_host_facts, None);
    }

    #[test]
    fn blank_observable_fields_are_not_gating() {
        let config = DeployTargetConfig {
            target: Some(TargetBinding {
                region: Some(String::new()),
                availability_zone: Some("   ".to_string()),
                instance_id: None,
                name_tag: None,
            }),
        };
        let source = FakeHostFactsSource::erroring();

        let verification = verify_deploy_target(&config, &source)
            .expect("blank observable fields must not arm host-facts observation");

        assert_eq!(
            verification.outcome,
            TargetVerifyOutcome::NoTargetConfigured
        );
        assert_eq!(verification.observed_host_facts, None);
    }

    #[test]
    fn blank_observable_fields_are_not_compared_when_another_field_gates() {
        let config = DeployTargetConfig {
            target: Some(TargetBinding {
                region: Some(String::new()),
                availability_zone: Some("   ".to_string()),
                instance_id: Some("instance-target".to_string()),
                name_tag: None,
            }),
        };
        let observed = facts("region-other", "region-other-zone-b", "instance-target");
        let source = FakeHostFactsSource::facts(observed.clone());

        let verification =
            verify_deploy_target(&config, &source).expect("non-blank instance_id must verify");

        assert_eq!(verification.outcome, TargetVerifyOutcome::Matched);
        assert_eq!(verification.observed_host_facts, Some(observed));
    }

    #[test]
    fn configured_and_matching_facts_yield_matched() {
        let config = DeployTargetConfig {
            target: Some(binding("region-x", "region-x-zone-a", "instance-target")),
        };
        let source =
            FakeHostFactsSource::facts(facts("region-x", "region-x-zone-a", "instance-target"));

        let verification =
            verify_deploy_target(&config, &source).expect("matching facts must verify");

        assert_eq!(verification.outcome, TargetVerifyOutcome::Matched);
        assert_eq!(
            verification.observed_host_facts,
            Some(facts("region-x", "region-x-zone-a", "instance-target"))
        );
    }

    #[test]
    fn configured_and_differing_instance_id_yields_mismatch() {
        let config = DeployTargetConfig {
            target: Some(binding("region-x", "region-x-zone-a", "instance-target")),
        };
        let source =
            FakeHostFactsSource::facts(facts("region-x", "region-x-zone-a", "instance-other"));

        let verification = verify_deploy_target(&config, &source)
            .expect("a differing instance id must be a clean Mismatched, not an error");

        assert_eq!(
            verification.outcome,
            TargetVerifyOutcome::Mismatched(vec![FieldMismatch {
                field: "instance_id",
                configured: "instance-target".to_string(),
                observed: Some("instance-other".to_string()),
            }])
        );
        assert_eq!(
            verification.observed_host_facts,
            Some(facts("region-x", "region-x-zone-a", "instance-other"))
        );
    }

    #[test]
    fn configured_field_absent_from_observed_facts_is_a_mismatch() {
        let config = DeployTargetConfig {
            target: Some(TargetBinding {
                instance_id: Some("instance-target".to_string()),
                region: None,
                availability_zone: None,
                name_tag: None,
            }),
        };
        let source = FakeHostFactsSource::facts(ObservedHostFacts {
            region: None,
            availability_zone: None,
            instance_id: None,
        });

        let verification = verify_deploy_target(&config, &source)
            .expect("a missing observed value is a mismatch, not an error");

        assert_eq!(
            verification.outcome,
            TargetVerifyOutcome::Mismatched(vec![FieldMismatch {
                field: "instance_id",
                configured: "instance-target".to_string(),
                observed: None,
            }])
        );
        assert_eq!(
            verification.observed_host_facts,
            Some(ObservedHostFacts {
                region: None,
                availability_zone: None,
                instance_id: None,
            })
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
    fn imdsv2_host_facts_source_observe_inside_active_tokio_runtime_returns_observe_error() {
        // Mirrors secrets::SsmResolverSession's nested-runtime guard tests:
        // observe() builds and block_on's a current-thread runtime, which Tokio
        // panics on if called from inside another runtime. The guard converts
        // that misuse into a structured DeployTargetError::Observe so a launch
        // never aborts via a Tokio panic. Build the source before the outer
        // runtime, mirroring the SSM ordering note.
        let source = Imdsv2HostFactsSource::new();
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("outer current-thread runtime must build for this test");
        let result = outer.block_on(async { source.observe() });
        let err = result.expect_err(
            "observe must return Err instead of panicking when called from \
             inside an active Tokio runtime",
        );
        assert!(
            matches!(err, DeployTargetError::Observe(_)),
            "nested-runtime misuse must surface as DeployTargetError::Observe; got: {err:?}"
        );
        assert!(
            err.to_string().contains("active Tokio runtime"),
            "guard error must name the nested-runtime cause; got: {err}"
        );
    }

    struct FakeImdsMetadataLeafSource {
        calls: std::cell::RefCell<Vec<&'static str>>,
    }

    impl FakeImdsMetadataLeafSource {
        fn new() -> Self {
            Self {
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl ImdsMetadataLeafSource for FakeImdsMetadataLeafSource {
        async fn fetch_metadata(
            &self,
            path: &'static str,
        ) -> Result<Option<String>, DeployTargetError> {
            self.calls.borrow_mut().push(path);
            match path {
                IMDS_INSTANCE_ID_PATH => Ok(Some("instance-target".to_string())),
                IMDS_AVAILABILITY_ZONE_PATH => Ok(Some("region-x-zone-a".to_string())),
                IMDS_REGION_PATH => Ok(Some("region-x".to_string())),
                other => Err(DeployTargetError::Observe(format!(
                    "unexpected IMDS metadata path requested by test fake: {other}"
                ))),
            }
        }
    }

    #[test]
    fn imdsv2_leaf_seam_maps_metadata_leaves_to_observed_host_facts() {
        let source = FakeImdsMetadataLeafSource::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime must build for leaf-seam test");

        let facts = runtime
            .block_on(observe_imdsv2_host_facts(&source))
            .expect("fake IMDS leaves must map into observed host facts");

        assert_eq!(
            facts,
            ObservedHostFacts {
                region: Some("region-x".to_string()),
                availability_zone: Some("region-x-zone-a".to_string()),
                instance_id: Some("instance-target".to_string()),
            }
        );
        let mut calls = source.calls.borrow().clone();
        calls.sort_unstable();
        let mut expected = vec![
            IMDS_AVAILABILITY_ZONE_PATH,
            IMDS_INSTANCE_ID_PATH,
            IMDS_REGION_PATH,
        ];
        expected.sort_unstable();
        assert_eq!(calls, expected);
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

        let verification = verify_deploy_target(&config, &FakeHostFactsSource::erroring())
            .expect("an empty [target] table must degrade to no target configured");
        assert_eq!(
            verification.outcome,
            TargetVerifyOutcome::NoTargetConfigured
        );
        assert_eq!(verification.observed_host_facts, None);
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
