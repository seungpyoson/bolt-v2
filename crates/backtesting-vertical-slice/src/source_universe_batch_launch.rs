use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use crate::{
    atomic_artifact_write::open_pinned_regular_file,
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    path_resolution::{resolve_pack_control_path, validate_portable_path_component},
    pinned_regular_file::{PinnedRegularFileIdentity, read_exact_pinned_file},
    source_universe_batch_execution::{
        SourceUniverseBatchBootstrapLimits, SourceUniverseBatchResourceLimits,
        validate_process_isolated_batch_selection,
    },
    source_universe_execution_pack::{
        SOURCE_UNIVERSE_EXECUTION_PACK_FILE, SourceUniverseExecutionPack,
        SourceUniverseExecutionPackSpec, SourceUniverseExecutionPackStatus,
        validate_execution_pack_semantics,
    },
    source_universe_local_storage::SourceUniverseLocalStoragePolicy,
};

pub const SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION: &str =
    "source-universe-batch-launch-spec.v4";
pub const COMMITTED_SOURCE_UNIVERSE_EXECUTION_PACK_ROOT: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs";
pub const SOURCE_UNIVERSE_EXECUTION_PACK_GENERATOR_SPEC_FILE: &str =
    "source-universe-execution-pack.toml";
pub const SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_FILE: &str = "source-universe-batch-launch.toml";
const SOURCE_UNIVERSE_EXECUTION_PACK_OUTPUT_DIR: &str = "execution-pack";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryIdentitySnapshot {
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    device: u64,
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    inode: u64,
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    modified_seconds: i64,
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    modified_nanoseconds: i64,
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    changed_seconds: i64,
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    changed_nanoseconds: i64,
}

impl DirectoryIdentitySnapshot {
    fn capture(path: &Path, metadata: &fs::Metadata) -> Result<Self> {
        ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "committed registry directory {} must be a non-symlink directory",
            path.display()
        );
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        {
            anyhow::bail!(
                "committed registry directory capabilities are unsupported on this platform for {}",
                path.display()
            );
        }
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        {
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
            })
        }
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn matches(&self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_dir()
            && !metadata.file_type().is_symlink()
            && self.device == metadata.dev()
            && self.inode == metadata.ino()
            && self.modified_seconds == metadata.mtime()
            && self.modified_nanoseconds == metadata.mtime_nsec()
            && self.changed_seconds == metadata.ctime()
            && self.changed_nanoseconds == metadata.ctime_nsec()
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    fn matches(&self, _metadata: &fs::Metadata) -> bool {
        false
    }
}

#[derive(Debug)]
struct PinnedDirectoryLease {
    canonical_path: PathBuf,
    file: fs::File,
    metadata: fs::Metadata,
    identity: DirectoryIdentitySnapshot,
}

impl PinnedDirectoryLease {
    fn open(path: &Path, expected_identity: Option<&DirectoryIdentitySnapshot>) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("lstat committed registry directory {}", path.display()))?;
        let identity = DirectoryIdentitySnapshot::capture(path, &metadata)?;
        if let Some(expected_identity) = expected_identity {
            ensure!(
                identity == *expected_identity,
                "committed registry directory {} does not match its enumerated identity",
                path.display()
            );
        }
        let canonical_path = path.canonicalize().with_context(|| {
            format!(
                "canonicalize committed registry directory {}",
                path.display()
            )
        })?;
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        {
            let _ = canonical_path;
            anyhow::bail!(
                "committed registry directory capabilities are unsupported on this platform for {}",
                path.display()
            );
        }
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        {
            let file = {
                let mut options = fs::OpenOptions::new();
                options.read(true).custom_flags(
                    libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                );
                options.open(path).with_context(|| {
                    format!("open committed registry directory {}", path.display())
                })?
            };
            let handle_metadata = file.metadata().with_context(|| {
                format!("fstat committed registry directory {}", path.display())
            })?;
            ensure!(
                identity.matches(&handle_metadata),
                "committed registry directory {} changed identity while opening",
                path.display()
            );
            let lease = Self {
                canonical_path,
                file,
                metadata,
                identity,
            };
            lease.revalidate()?;
            Ok(lease)
        }
    }

    fn revalidate(&self) -> Result<()> {
        let path_metadata = fs::symlink_metadata(&self.canonical_path).with_context(|| {
            format!(
                "re-lstat committed registry directory {}",
                self.canonical_path.display()
            )
        })?;
        let handle_metadata = self.file.metadata().with_context(|| {
            format!(
                "re-fstat committed registry directory {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            self.identity.matches(&path_metadata) && self.identity.matches(&handle_metadata),
            "committed registry directory identity changed: {}",
            self.canonical_path.display()
        );
        let canonical_now = self.canonical_path.canonicalize().with_context(|| {
            format!(
                "re-canonicalize committed registry directory {}",
                self.canonical_path.display()
            )
        })?;
        ensure!(
            canonical_now == self.canonical_path,
            "committed registry directory canonical path changed: {}",
            self.canonical_path.display()
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommittedRegistryScopeSnapshot {
    name: String,
    path: PathBuf,
    identity: DirectoryIdentitySnapshot,
}

#[derive(Debug)]
struct CommittedPackReadLease {
    scope: PinnedDirectoryLease,
    output: PinnedDirectoryLease,
    generator_path: PathBuf,
    generator_file: fs::File,
    generator_identity: PinnedRegularFileIdentity,
    launch_path: PathBuf,
    launch_file: fs::File,
    launch_identity: PinnedRegularFileIdentity,
    summary_path: PathBuf,
    summary_file: fs::File,
    summary_identity: PinnedRegularFileIdentity,
}

impl CommittedPackReadLease {
    fn revalidate(&self) -> Result<()> {
        self.scope.revalidate()?;
        self.output.revalidate()?;
        self.generator_identity
            .revalidate(&self.generator_path, &self.generator_file)?;
        self.launch_identity
            .revalidate(&self.launch_path, &self.launch_file)?;
        self.summary_identity
            .revalidate(&self.summary_path, &self.summary_file)?;
        Ok(())
    }
}

fn inventory_committed_registry_scopes(
    registry: &PinnedDirectoryLease,
) -> Result<Vec<CommittedRegistryScopeSnapshot>> {
    registry.revalidate()?;
    let mut scopes = fs::read_dir(&registry.canonical_path)
        .with_context(|| {
            format!(
                "read committed source-universe execution-pack registry {}",
                registry.canonical_path.display()
            )
        })?
        .map(|entry| {
            let entry = entry.with_context(|| {
                format!(
                    "read entry in registry {}",
                    registry.canonical_path.display()
                )
            })?;
            let name = entry.file_name().into_string().map_err(|_| {
                anyhow::anyhow!("committed execution-pack scope name must be UTF-8")
            })?;
            validate_portable_path_component("committed_execution_pack_scope", &name)?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).with_context(|| {
                format!("lstat committed execution-pack scope {}", path.display())
            })?;
            let identity = DirectoryIdentitySnapshot::capture(&path, &metadata)?;
            let canonical_path = path.canonicalize().with_context(|| {
                format!(
                    "canonicalize committed execution-pack scope {}",
                    path.display()
                )
            })?;
            ensure!(
                canonical_path.starts_with(&registry.canonical_path),
                "committed execution-pack scope {} resolves outside registry root {}",
                canonical_path.display(),
                registry.canonical_path.display()
            );
            Ok(CommittedRegistryScopeSnapshot {
                name,
                path,
                identity,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    scopes.sort_by(|left, right| left.name.cmp(&right.name));
    ensure!(
        !scopes.is_empty(),
        "committed source-universe execution-pack registry {} must not be empty",
        registry.canonical_path.display()
    );
    registry.revalidate()?;
    Ok(scopes)
}

#[derive(Debug)]
pub struct CommittedSourceUniverseExecutionPack {
    pub scope_dir: PathBuf,
    pub generator_spec_path: PathBuf,
    pub launch_path: PathBuf,
    pub launch_bytes: u64,
    pub launch_sha256: String,
    pub summary_path: PathBuf,
    pub pack_id: String,
    pub launch_spec: SourceUniverseBatchLaunchSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchLaunchArtifactSpec {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchLaunchSpec {
    pub schema_version: String,
    pub batch_id: String,
    pub execution_pack: SourceUniverseBatchLaunchArtifactSpec,
    pub output_dir: PathBuf,
    pub start_sequence: Option<u64>,
    pub record_limit: Option<u64>,
    pub continue_on_error: bool,
    pub fetch_timeout_seconds: u64,
    pub worker_termination_grace_seconds: u64,
    pub max_concurrent_records: u64,
    pub transport: SourceUniverseBatchTransportSpec,
    pub object_cache_dir: PathBuf,
    pub allow_partial: bool,
    pub bootstrap_limits: SourceUniverseBatchBootstrapLimits,
    pub resource_limits: SourceUniverseBatchResourceLimits,
    pub local_storage: SourceUniverseLocalStoragePolicy,
}

#[derive(Debug)]
pub struct PinnedSourceUniverseBatchLaunchSpec {
    pub canonical_path: PathBuf,
    pub spec: SourceUniverseBatchLaunchSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceUniverseBatchTransportSpec {
    StagedS3,
    Https { http_user_agent: String },
}

impl SourceUniverseBatchLaunchSpec {
    /// Read exactly the launch artifact admitted by the parent tracer.
    ///
    /// Length and SHA-256 are checked before parsing, so a pathname replacement
    /// cannot make the child execute a different output root, execution pack,
    /// or resource envelope from the one admitted before process fanout.
    pub fn from_sha256_pinned_toml_file(
        path: &Path,
        expected_bytes: u64,
        expected_sha256: &str,
    ) -> Result<PinnedSourceUniverseBatchLaunchSpec> {
        ensure!(
            expected_bytes > 0,
            "expected batch launch spec byte length must be positive"
        );
        ensure!(
            is_lowercase_sha256_hex(expected_sha256),
            "expected batch launch spec SHA-256 must be lowercase hex"
        );
        let declared_parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent_lease = PinnedDirectoryLease::open(declared_parent, None)
            .context("pin batch launch spec parent directory")?;
        let canonical_path = require_regular_contained_file(
            &parent_lease.canonical_path,
            path,
            "batch launch spec",
        )?;
        ensure!(
            canonical_path.parent() == Some(parent_lease.canonical_path.as_path()),
            "batch launch spec {} must be one direct child of its pinned parent {}",
            canonical_path.display(),
            parent_lease.canonical_path.display()
        );
        parent_lease.revalidate()?;
        let (mut file, identity) =
            open_pinned_regular_file(&canonical_path).with_context(|| {
                format!("open pinned batch launch spec {}", canonical_path.display())
            })?;
        identity
            .revalidate_expected_parent(&parent_lease.canonical_path, &parent_lease.metadata)?;
        ensure!(
            identity.byte_len == expected_bytes,
            "batch launch spec {} byte length mismatch: expected {}, got {}",
            canonical_path.display(),
            expected_bytes,
            identity.byte_len
        );
        let bytes = read_exact_pinned_file(&mut file, &canonical_path, expected_bytes)?;
        identity.revalidate(&canonical_path, &file)?;
        identity
            .revalidate_expected_parent(&parent_lease.canonical_path, &parent_lease.metadata)?;
        parent_lease.revalidate()?;
        let actual_sha256 = sha256_hex(&bytes);
        ensure!(
            actual_sha256 == expected_sha256,
            "batch launch spec {} SHA-256 mismatch: expected {}, got {}",
            canonical_path.display(),
            expected_sha256,
            actual_sha256
        );
        let spec = Self::from_toml_bytes(&canonical_path, &bytes)?;
        identity.revalidate(&canonical_path, &file)?;
        identity
            .revalidate_expected_parent(&parent_lease.canonical_path, &parent_lease.metadata)?;
        parent_lease.revalidate()?;
        Ok(PinnedSourceUniverseBatchLaunchSpec {
            canonical_path,
            spec,
        })
    }

    fn from_toml_bytes(path: &Path, bytes: &[u8]) -> Result<Self> {
        let spec: Self = toml::from_slice(bytes)
            .with_context(|| format!("parse batch launch spec {}", path.display()))?;
        ensure!(
            spec.schema_version == SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
            "batch launch spec schema_version mismatch: expected {}, got {}",
            SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
            spec.schema_version
        );
        ensure!(
            spec.fetch_timeout_seconds > 0,
            "batch launch spec fetch_timeout_seconds must be positive"
        );
        ensure!(
            spec.worker_termination_grace_seconds > 0,
            "batch launch spec worker_termination_grace_seconds must be positive"
        );
        spec.transport.validate()?;
        validate_process_isolated_batch_selection(
            spec.record_limit,
            Some(spec.max_concurrent_records),
        )?;
        spec.bootstrap_limits.validate()?;
        spec.resource_limits.validate()?;
        spec.local_storage.validate()?;
        Ok(spec)
    }
}

impl SourceUniverseBatchTransportSpec {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::StagedS3 => Ok(()),
            Self::Https { http_user_agent } => validate_http_user_agent(http_user_agent),
        }
    }
}

/// Discover the complete, committed execution-pack registry below `repo_root`.
///
/// Discovery is intentionally validation, not launch: callers still execute
/// exactly one explicitly selected TOML launch spec. Every immediate registry
/// child must be one self-contained, immutable pack entry; an invalid child
/// makes the whole registry unusable.
pub fn discover_committed_source_universe_execution_packs(
    repo_root: &Path,
) -> Result<Vec<CommittedSourceUniverseExecutionPack>> {
    let repo_root_metadata = fs::symlink_metadata(repo_root)
        .with_context(|| format!("stat repository root {}", repo_root.display()))?;
    ensure!(
        repo_root_metadata.is_dir() && !repo_root_metadata.file_type().is_symlink(),
        "repository root {} must be a non-symlink directory",
        repo_root.display()
    );
    let canonical_repo_root = repo_root
        .canonicalize()
        .with_context(|| format!("canonicalize repository root {}", repo_root.display()))?;

    let registry_root = repo_root.join(COMMITTED_SOURCE_UNIVERSE_EXECUTION_PACK_ROOT);
    let registry_lease = PinnedDirectoryLease::open(&registry_root, None)?;
    ensure!(
        registry_lease
            .canonical_path
            .starts_with(&canonical_repo_root),
        "committed source-universe execution-pack registry {} resolves outside repository root {}",
        registry_lease.canonical_path.display(),
        canonical_repo_root.display()
    );
    let initial_scopes = inventory_committed_registry_scopes(&registry_lease)?;

    let mut pack_ids = BTreeSet::new();
    let mut summary_paths = BTreeSet::new();
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    let mut summary_file_identities = BTreeSet::new();
    let mut committed_packs = Vec::with_capacity(initial_scopes.len());
    let mut read_leases = Vec::with_capacity(initial_scopes.len());

    for scope_snapshot in &initial_scopes {
        registry_lease.revalidate()?;
        let scope_lease =
            PinnedDirectoryLease::open(&scope_snapshot.path, Some(&scope_snapshot.identity))?;
        let canonical_scope_dir = scope_lease.canonical_path.clone();
        ensure!(
            canonical_scope_dir.starts_with(&registry_lease.canonical_path),
            "committed execution-pack scope {} resolves outside registry root {}",
            canonical_scope_dir.display(),
            registry_lease.canonical_path.display()
        );

        let generator_spec_path = require_regular_contained_file(
            &canonical_scope_dir,
            &canonical_scope_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_GENERATOR_SPEC_FILE),
            "execution-pack generator spec",
        )?;
        let launch_path = require_regular_contained_file(
            &canonical_scope_dir,
            &canonical_scope_dir.join(SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_FILE),
            "batch launch spec",
        )?;
        let expected_output_dir =
            canonical_scope_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_OUTPUT_DIR);
        let output_lease = PinnedDirectoryLease::open(&expected_output_dir, None)?;
        let canonical_output_dir = output_lease.canonical_path.clone();
        ensure!(
            canonical_output_dir.starts_with(&canonical_scope_dir),
            "committed execution-pack output {} resolves outside scope {}",
            canonical_output_dir.display(),
            canonical_scope_dir.display()
        );
        let summary_path = require_regular_contained_file(
            &canonical_scope_dir,
            &canonical_output_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE),
            "execution-pack summary",
        )?;

        let (generator_bytes, generator_file, generator_identity) = read_pinned_regular_file(
            &generator_spec_path,
            "execution-pack generator spec",
            (&canonical_scope_dir, &scope_lease.metadata),
        )?;
        let generator_spec: SourceUniverseExecutionPackSpec = toml::from_slice(&generator_bytes)
            .with_context(|| {
                format!(
                    "parse execution-pack generator spec {}",
                    generator_spec_path.display()
                )
            })?;
        ensure!(
            !generator_spec.pack_id.trim().is_empty(),
            "execution-pack generator spec {} pack_id must not be empty",
            generator_spec_path.display()
        );
        if let Some(record_limit) = generator_spec.record_limit {
            ensure!(
                record_limit > 0,
                "execution-pack generator spec {} record_limit must be positive when set",
                generator_spec_path.display()
            );
        }
        let expected_output_identity = canonical_output_dir
            .strip_prefix(&canonical_repo_root)
            .expect("contained committed output has a repository-relative identity");
        ensure!(
            generator_spec.output_dir == expected_output_identity,
            "execution-pack generator spec {} output_dir must be exactly {}, got {}",
            generator_spec_path.display(),
            expected_output_identity.display(),
            generator_spec.output_dir.display()
        );

        let (launch_bytes, launch_file, launch_identity) = read_pinned_regular_file(
            &launch_path,
            "batch launch spec",
            (&canonical_scope_dir, &scope_lease.metadata),
        )?;
        let launch_sha256 = sha256_hex(&launch_bytes);
        let launch_spec =
            SourceUniverseBatchLaunchSpec::from_toml_bytes(&launch_path, &launch_bytes)?;
        let expected_summary_identity = Path::new(SOURCE_UNIVERSE_EXECUTION_PACK_OUTPUT_DIR)
            .join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE);
        ensure!(
            launch_spec.execution_pack.path == expected_summary_identity,
            "batch launch spec {} execution_pack.path must be exactly {}, got {}",
            launch_path.display(),
            expected_summary_identity.display(),
            launch_spec.execution_pack.path.display()
        );
        let resolved_launch_summary =
            resolve_pack_control_path(&canonical_scope_dir, &launch_spec.execution_pack.path)?;
        ensure!(
            resolved_launch_summary == summary_path,
            "batch launch spec {} resolves execution-pack summary to {}, expected {}",
            launch_path.display(),
            resolved_launch_summary.display(),
            summary_path.display()
        );

        let (mut summary_file, summary_identity) = open_pinned_regular_file(&summary_path)
            .with_context(|| format!("pin execution-pack summary {}", summary_path.display()))?;
        summary_identity
            .revalidate_expected_parent(&canonical_output_dir, &output_lease.metadata)?;
        ensure!(
            launch_spec.execution_pack.bytes == summary_identity.byte_len,
            "batch launch spec {} execution-pack byte length mismatch: expected {}, got {}",
            launch_path.display(),
            launch_spec.execution_pack.bytes,
            summary_identity.byte_len
        );
        let summary_bytes = read_exact_pinned_file(
            &mut summary_file,
            &summary_path,
            launch_spec.execution_pack.bytes,
        )?;
        summary_identity.revalidate(&summary_path, &summary_file)?;
        let summary_sha256 = sha256_hex(&summary_bytes);
        ensure!(
            launch_spec.execution_pack.sha256 == summary_sha256,
            "batch launch spec {} execution-pack SHA-256 mismatch: expected {}, got {}",
            launch_path.display(),
            launch_spec.execution_pack.sha256,
            summary_sha256
        );
        let summary: SourceUniverseExecutionPack = serde_json::from_slice(&summary_bytes)
            .with_context(|| format!("parse execution-pack summary {}", summary_path.display()))?;
        validate_execution_pack_semantics(&summary).with_context(|| {
            format!("validate execution-pack summary {}", summary_path.display())
        })?;
        ensure!(
            matches!(
                summary.status,
                SourceUniverseExecutionPackStatus::Ready
                    | SourceUniverseExecutionPackStatus::PartiallyReady
            ),
            "committed execution-pack summary {} must be ready or partially_ready",
            summary_path.display()
        );
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        ensure!(
            summary_file_identities.insert((summary_identity.device, summary_identity.inode)),
            "duplicate committed execution-pack summary file identity at {}",
            summary_path.display()
        );
        ensure!(
            generator_spec.pack_id == summary.pack_id,
            "committed execution-pack pack_id mismatch: generator {} declares {}, summary {} declares {}",
            generator_spec_path.display(),
            generator_spec.pack_id,
            summary_path.display(),
            summary.pack_id
        );
        ensure!(
            pack_ids.insert(summary.pack_id.clone()),
            "duplicate committed execution-pack pack_id {}",
            summary.pack_id
        );
        ensure!(
            summary_paths.insert(summary_path.clone()),
            "duplicate committed execution-pack summary path {}",
            summary_path.display()
        );
        committed_packs.push(CommittedSourceUniverseExecutionPack {
            scope_dir: canonical_scope_dir,
            generator_spec_path: generator_spec_path.clone(),
            launch_path: launch_path.clone(),
            launch_bytes: launch_identity.byte_len,
            launch_sha256,
            summary_path: summary_path.clone(),
            pack_id: summary.pack_id,
            launch_spec,
        });
        read_leases.push(CommittedPackReadLease {
            scope: scope_lease,
            output: output_lease,
            generator_path: generator_spec_path,
            generator_file,
            generator_identity,
            launch_path,
            launch_file,
            launch_identity,
            summary_path,
            summary_file,
            summary_identity,
        });
    }

    registry_lease.revalidate()?;
    for lease in &read_leases {
        lease.revalidate()?;
    }
    let final_scopes = inventory_committed_registry_scopes(&registry_lease)?;
    ensure!(
        final_scopes == initial_scopes,
        "committed source-universe execution-pack registry membership changed during discovery"
    );
    for lease in &read_leases {
        lease.revalidate()?;
    }
    registry_lease.revalidate()?;

    Ok(committed_packs)
}

fn require_regular_contained_file(
    authoritative_root: &Path,
    path: &Path,
    label: &str,
) -> Result<PathBuf> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("stat {label} {}", path.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{label} {} must be a non-symlink regular file",
        path.display()
    );
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("canonicalize {label} {}", path.display()))?;
    ensure!(
        canonical_path.starts_with(authoritative_root),
        "{label} {} resolves outside authoritative root {}",
        canonical_path.display(),
        authoritative_root.display()
    );
    Ok(canonical_path)
}

fn read_pinned_regular_file(
    path: &Path,
    label: &str,
    expected_parent: (&Path, &fs::Metadata),
) -> Result<(Vec<u8>, fs::File, PinnedRegularFileIdentity)> {
    let (mut file, identity) = open_pinned_regular_file(path)
        .with_context(|| format!("pin {label} {}", path.display()))?;
    identity.revalidate_expected_parent(expected_parent.0, expected_parent.1)?;
    let bytes = read_exact_pinned_file(&mut file, path, identity.byte_len)
        .with_context(|| format!("read pinned {label} {}", path.display()))?;
    identity
        .revalidate(path, &file)
        .with_context(|| format!("revalidate pinned {label} {}", path.display()))?;
    Ok((bytes, file, identity))
}

fn validate_http_user_agent(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "batch launch spec http_user_agent must not be empty"
    );
    reqwest::header::HeaderValue::from_bytes(value.as_bytes())
        .context("batch launch spec http_user_agent must be a valid HTTP HeaderValue")?;
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests {
    use std::{fs, path::Path};

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{
        COMMITTED_SOURCE_UNIVERSE_EXECUTION_PACK_ROOT, PinnedDirectoryLease,
        SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_FILE, SOURCE_UNIVERSE_EXECUTION_PACK_GENERATOR_SPEC_FILE,
        discover_committed_source_universe_execution_packs, inventory_committed_registry_scopes,
    };
    use crate::{
        hashing::sha256_hex, source_universe_execution_pack::SOURCE_UNIVERSE_EXECUTION_PACK_FILE,
    };

    struct SyntheticCommittedPack {
        generator_spec_path: std::path::PathBuf,
        launch_path: std::path::PathBuf,
        output_dir: std::path::PathBuf,
        summary_path: std::path::PathBuf,
    }

    fn registry_root(repo_root: &Path) -> std::path::PathBuf {
        repo_root.join(COMMITTED_SOURCE_UNIVERSE_EXECUTION_PACK_ROOT)
    }

    fn summary_value(pack_id: &str) -> Value {
        json!({
            "schema_version": "source-universe-execution-pack.v4",
            "pack_id": pack_id,
            "status": "ready",
            "work_order_id": "work-order",
            "input_id": "operator-input",
            "gate_id": "object-gate",
            "conversion_run_plan_id": "conversion-plan",
            "universe_id": "test-universe",
            "venue": "test-venue",
            "source": "test-source",
            "family": "test-family",
            "table_family": "trades",
            "planned_object_count": 1,
            "executable_record_count": 1,
            "withheld_record_count": 0,
            "selected_record_count": 1,
            "materialized_record_count": 1,
            "skipped_executable_record_count": 0,
            "executable_source_bytes": 1,
            "materialized_source_bytes": 1,
            "artifact_refs": [{
                "role": "source_bindings",
                "path": "config/source-bindings.toml",
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
            }],
            "records": [{
                "sequence": 0,
                "work_item_id": "work-item",
                "operator_run_id": "operator-run",
                "source_binding": "source-binding",
                "category": "spot",
                "symbol": "SYMBOL",
                "archive_date": "2026-07-01",
                "source_uri": "s3://bucket/object",
                "source_url": "https://example.invalid/object",
                "selected_object_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "selected_object_bytes": 1,
                "source_proof_id": "source-proof",
                "source_proof_version": 1,
                "accepted_tranche_id": "accepted-tranche",
                "output_prefix": "s3://bucket/output",
                "source_bindings_path": "config/source-bindings.toml",
                "source_bindings_bytes": 1,
                "source_bindings_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "run_spec_path": "controls/run-spec.toml",
                "run_spec_bytes": 1,
                "run_spec_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "accepted_tranche_path": "controls/accepted-tranche.json",
                "accepted_tranche_bytes": 1,
                "accepted_tranche_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "execution_plan_path": "controls/execution-plan.json",
                "execution_plan_bytes": 1,
                "execution_plan_sha256": "0000000000000000000000000000000000000000000000000000000000000000"
            }],
            "blocking_reasons": []
        })
    }

    fn write_generator_spec(path: &Path, pack_id: &str, output_dir: &Path) {
        fs::write(
            path,
            format!(
                r#"pack_id = "{pack_id}"
source_universe_conversion_work_order_path = "controls/work-order.json"
run_spec_template_path = "controls/run-spec.toml"
output_dir = "{}"
overwrite_existing_artifacts = false
record_limit = 1

[venue_policy]
starting_balance_amount = "1"
spot = "CASH"
crypto_perpetual = "MARGIN"
crypto_future = "MARGIN"
"#,
                output_dir.display()
            ),
        )
        .expect("write generator spec");
    }

    fn write_launch_spec(
        path: &Path,
        summary_identity: &Path,
        summary_bytes: &[u8],
        declared_bytes: u64,
        declared_sha256: &str,
    ) {
        fs::write(
            path,
            format!(
                r#"schema_version = "source-universe-batch-launch-spec.v4"
batch_id = "synthetic-one-record-tracer"
output_dir = "target/source-universe-batch-output/synthetic-one-record-tracer"
record_limit = 1
continue_on_error = false
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "target/source-universe-workspace/cache"
allow_partial = false

[transport]
kind = "staged_s3"

[execution_pack]
path = "{}"
bytes = {declared_bytes}
sha256 = "{declared_sha256}"

[bootstrap_limits]
max_launch_artifact_bytes = 65536
max_control_artifact_bytes = 65536
max_retained_control_input_bytes = 262144

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 1

[local_storage]
workspace_root = "target/source-universe-workspace"
owner_lock_path = "target/source-universe-workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                summary_identity.display()
            ),
        )
        .expect("write launch spec");
        assert!(!summary_bytes.is_empty());
    }

    fn write_committed_pack(
        repo_root: &Path,
        scope_name: &str,
        pack_id: &str,
    ) -> SyntheticCommittedPack {
        let scope_dir = registry_root(repo_root).join(scope_name);
        let output_dir = scope_dir.join("execution-pack");
        fs::create_dir_all(&output_dir).expect("create synthetic committed-pack directories");
        let generator_spec_path =
            scope_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_GENERATOR_SPEC_FILE);
        let launch_path = scope_dir.join(SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_FILE);
        let summary_path = output_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE);
        let repo_relative_output_dir = Path::new(COMMITTED_SOURCE_UNIVERSE_EXECUTION_PACK_ROOT)
            .join(scope_name)
            .join("execution-pack");
        write_generator_spec(&generator_spec_path, pack_id, &repo_relative_output_dir);
        let summary_bytes = serde_json::to_vec_pretty(&summary_value(pack_id))
            .expect("serialize synthetic execution pack");
        fs::write(&summary_path, &summary_bytes).expect("write execution-pack summary");
        write_launch_spec(
            &launch_path,
            Path::new("execution-pack")
                .join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE)
                .as_path(),
            &summary_bytes,
            u64::try_from(summary_bytes.len()).expect("summary length fits u64"),
            &sha256_hex(&summary_bytes),
        );
        SyntheticCommittedPack {
            generator_spec_path,
            launch_path,
            output_dir,
            summary_path,
        }
    }

    fn refresh_launch_pin(pack: &SyntheticCommittedPack) {
        let summary_bytes = fs::read(&pack.summary_path).expect("read synthetic summary");
        write_launch_spec(
            &pack.launch_path,
            Path::new("execution-pack")
                .join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE)
                .as_path(),
            &summary_bytes,
            u64::try_from(summary_bytes.len()).expect("summary length fits u64"),
            &sha256_hex(&summary_bytes),
        );
    }

    fn error_text(repo_root: &Path) -> String {
        format!(
            "{:#}",
            discover_committed_source_universe_execution_packs(repo_root)
                .expect_err("registry must fail closed")
        )
    }

    #[test]
    fn discovers_every_immediate_scope_in_sorted_order() {
        let repo = TempDir::new().expect("temporary repository");
        write_committed_pack(repo.path(), "zeta-scope", "pack-zeta");
        write_committed_pack(repo.path(), "alpha-scope", "pack-alpha");

        let packs = discover_committed_source_universe_execution_packs(repo.path())
            .expect("discover committed packs");

        assert_eq!(
            packs
                .iter()
                .map(|pack| pack.pack_id.as_str())
                .collect::<Vec<_>>(),
            ["pack-alpha", "pack-zeta"]
        );
        for pack in packs {
            assert!(
                pack.scope_dir
                    .starts_with(repo.path().canonicalize().unwrap())
            );
            assert!(pack.generator_spec_path.is_file());
            assert!(pack.launch_path.is_file());
            assert!(pack.summary_path.is_file());
            assert_eq!(pack.launch_spec.record_limit, Some(1));
            assert_eq!(pack.launch_spec.max_concurrent_records, 1);
            assert!(
                pack.launch_spec
                    .resource_limits
                    .worker_reserved_overhead_bytes
                    > 0
            );
        }
    }

    #[test]
    fn rejects_missing_or_empty_registry() {
        let missing_repo = TempDir::new().expect("temporary repository");
        assert!(error_text(missing_repo.path()).contains("stat committed"));

        let empty_repo = TempDir::new().expect("temporary repository");
        fs::create_dir_all(registry_root(empty_repo.path())).expect("create empty registry");
        assert!(error_text(empty_repo.path()).contains("must not be empty"));
    }

    #[test]
    fn rejects_non_directory_registry_child() {
        let repo = TempDir::new().expect("temporary repository");
        write_committed_pack(repo.path(), "valid-scope", "valid-pack");
        fs::write(
            registry_root(repo.path()).join("loose-entry"),
            b"not a scope",
        )
        .expect("write loose registry entry");

        assert!(error_text(repo.path()).contains("must be a non-symlink directory"));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn pinned_registry_rejects_same_name_directory_replacement() {
        let repo = TempDir::new().expect("temporary repository");
        write_committed_pack(repo.path(), "stable-scope", "stable-pack");
        let registry_path = registry_root(repo.path());
        let registry = PinnedDirectoryLease::open(&registry_path, None).expect("pin registry");
        let initial = inventory_committed_registry_scopes(&registry).expect("inventory registry");
        let displaced = repo.path().join("displaced-registry");
        fs::rename(&registry_path, &displaced).expect("displace registry");
        fs::create_dir(&registry_path).expect("create replacement registry");
        fs::create_dir(registry_path.join(&initial[0].name))
            .expect("create same-name replacement scope");

        let error = registry
            .revalidate()
            .expect_err("replacement registry must not inherit prior authorization");

        assert!(error.to_string().contains("directory identity changed"));
    }

    #[test]
    fn rejects_missing_or_non_regular_required_file() {
        let missing_repo = TempDir::new().expect("temporary repository");
        let missing = write_committed_pack(missing_repo.path(), "missing-scope", "missing-pack");
        fs::remove_file(&missing.launch_path).expect("remove launch spec");
        assert!(error_text(missing_repo.path()).contains("stat batch launch spec"));

        let directory_repo = TempDir::new().expect("temporary repository");
        let directory =
            write_committed_pack(directory_repo.path(), "directory-scope", "directory-pack");
        fs::remove_file(&directory.launch_path).expect("remove launch spec");
        fs::create_dir(&directory.launch_path).expect("replace launch with directory");
        assert!(error_text(directory_repo.path()).contains("non-symlink regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_scope_required_file_and_output() {
        use std::os::unix::fs::symlink;

        let scope_repo = TempDir::new().expect("temporary repository");
        fs::create_dir_all(registry_root(scope_repo.path())).expect("create registry");
        let external_scope = scope_repo.path().join("external-scope");
        fs::create_dir(&external_scope).expect("create external scope");
        symlink(
            &external_scope,
            registry_root(scope_repo.path()).join("scope-link"),
        )
        .expect("symlink scope");
        assert!(error_text(scope_repo.path()).contains("non-symlink directory"));

        let file_repo = TempDir::new().expect("temporary repository");
        let file_pack = write_committed_pack(file_repo.path(), "file-scope", "file-pack");
        let real_launch = file_pack.launch_path.with_extension("real");
        fs::rename(&file_pack.launch_path, &real_launch).expect("move launch spec");
        symlink(&real_launch, &file_pack.launch_path).expect("symlink launch spec");
        assert!(error_text(file_repo.path()).contains("non-symlink regular file"));

        let output_repo = TempDir::new().expect("temporary repository");
        let output_pack = write_committed_pack(output_repo.path(), "output-scope", "output-pack");
        let external_output = output_repo.path().join("external-output");
        fs::rename(&output_pack.output_dir, &external_output).expect("move output directory");
        symlink(&external_output, &output_pack.output_dir).expect("symlink output directory");
        assert!(error_text(output_repo.path()).contains("non-symlink directory"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_repository_root() {
        use std::os::unix::fs::symlink;

        let parent = TempDir::new().expect("temporary parent");
        let real_repo = parent.path().join("real-repo");
        fs::create_dir(&real_repo).expect("create real repository");
        let linked_repo = parent.path().join("linked-repo");
        symlink(&real_repo, &linked_repo).expect("symlink repository");

        assert!(error_text(&linked_repo).contains("repository root"));
    }

    #[test]
    fn rejects_malformed_generator_launch_and_summary_documents() {
        let generator_repo = TempDir::new().expect("temporary repository");
        let generator =
            write_committed_pack(generator_repo.path(), "generator-scope", "generator-pack");
        fs::write(&generator.generator_spec_path, b"not = [valid").expect("corrupt generator spec");
        assert!(error_text(generator_repo.path()).contains("parse execution-pack generator spec"));

        let launch_repo = TempDir::new().expect("temporary repository");
        let launch = write_committed_pack(launch_repo.path(), "launch-scope", "launch-pack");
        fs::write(&launch.launch_path, b"not = [valid").expect("corrupt launch spec");
        assert!(error_text(launch_repo.path()).contains("parse batch launch spec"));

        let summary_repo = TempDir::new().expect("temporary repository");
        let summary = write_committed_pack(summary_repo.path(), "summary-scope", "summary-pack");
        fs::write(&summary.summary_path, b"not-json").expect("corrupt summary");
        refresh_launch_pin(&summary);
        assert!(error_text(summary_repo.path()).contains("parse execution-pack summary"));
    }

    #[test]
    fn rejects_launch_size_hash_and_summary_path_mismatches() {
        let size_repo = TempDir::new().expect("temporary repository");
        let size = write_committed_pack(size_repo.path(), "size-scope", "size-pack");
        let size_bytes = fs::read(&size.summary_path).expect("read summary");
        write_launch_spec(
            &size.launch_path,
            Path::new("execution-pack")
                .join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE)
                .as_path(),
            &size_bytes,
            u64::try_from(size_bytes.len()).unwrap() + 1,
            &sha256_hex(&size_bytes),
        );
        assert!(error_text(size_repo.path()).contains("byte length mismatch"));

        let hash_repo = TempDir::new().expect("temporary repository");
        let hash = write_committed_pack(hash_repo.path(), "hash-scope", "hash-pack");
        let hash_bytes = fs::read(&hash.summary_path).expect("read summary");
        write_launch_spec(
            &hash.launch_path,
            Path::new("execution-pack")
                .join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE)
                .as_path(),
            &hash_bytes,
            u64::try_from(hash_bytes.len()).unwrap(),
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        assert!(error_text(hash_repo.path()).contains("SHA-256 mismatch"));

        let path_repo = TempDir::new().expect("temporary repository");
        let path = write_committed_pack(path_repo.path(), "path-scope", "path-pack");
        let path_bytes = fs::read(&path.summary_path).expect("read summary");
        write_launch_spec(
            &path.launch_path,
            Path::new("alternate/source-universe-execution-pack.json"),
            &path_bytes,
            u64::try_from(path_bytes.len()).unwrap(),
            &sha256_hex(&path_bytes),
        );
        assert!(error_text(path_repo.path()).contains("execution_pack.path must be exactly"));
    }

    #[test]
    fn rejects_generator_output_and_pack_id_mismatches() {
        let output_repo = TempDir::new().expect("temporary repository");
        let output = write_committed_pack(output_repo.path(), "output-scope", "output-pack");
        write_generator_spec(
            &output.generator_spec_path,
            "output-pack",
            Path::new("specs/unrelated-output"),
        );
        assert!(error_text(output_repo.path()).contains("output_dir must be exactly"));

        let id_repo = TempDir::new().expect("temporary repository");
        let id = write_committed_pack(id_repo.path(), "identity-scope", "summary-pack");
        let expected_output = Path::new(COMMITTED_SOURCE_UNIVERSE_EXECUTION_PACK_ROOT)
            .join("identity-scope")
            .join("execution-pack");
        write_generator_spec(&id.generator_spec_path, "generator-pack", &expected_output);
        assert!(error_text(id_repo.path()).contains("pack_id mismatch"));
    }

    #[test]
    fn rejects_semantically_invalid_or_blocked_summary() {
        let invalid_repo = TempDir::new().expect("temporary repository");
        let invalid = write_committed_pack(invalid_repo.path(), "invalid-scope", "invalid-pack");
        let mut invalid_value = summary_value("invalid-pack");
        invalid_value["artifact_refs"] = json!([]);
        fs::write(
            &invalid.summary_path,
            serde_json::to_vec_pretty(&invalid_value).unwrap(),
        )
        .expect("write invalid summary");
        refresh_launch_pin(&invalid);
        assert!(error_text(invalid_repo.path()).contains("source_bindings artifact ref"));

        let blocked_repo = TempDir::new().expect("temporary repository");
        let blocked = write_committed_pack(blocked_repo.path(), "blocked-scope", "blocked-pack");
        let mut blocked_value = summary_value("blocked-pack");
        blocked_value["status"] = json!("blocked");
        blocked_value["planned_object_count"] = json!(0);
        blocked_value["executable_record_count"] = json!(0);
        blocked_value["selected_record_count"] = json!(0);
        blocked_value["materialized_record_count"] = json!(0);
        blocked_value["executable_source_bytes"] = json!(0);
        blocked_value["materialized_source_bytes"] = json!(0);
        blocked_value["records"] = json!([]);
        blocked_value["blocking_reasons"] =
            json!(["no_source_universe_execution_records_materialized"]);
        fs::write(
            &blocked.summary_path,
            serde_json::to_vec_pretty(&blocked_value).unwrap(),
        )
        .expect("write blocked summary");
        refresh_launch_pin(&blocked);
        assert!(error_text(blocked_repo.path()).contains("ready or partially_ready"));
    }

    #[test]
    fn rejects_duplicate_pack_ids() {
        let repo = TempDir::new().expect("temporary repository");
        write_committed_pack(repo.path(), "alpha-scope", "shared-pack");
        write_committed_pack(repo.path(), "beta-scope", "shared-pack");

        assert!(error_text(repo.path()).contains("duplicate committed execution-pack pack_id"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_duplicate_summary_file_identity() {
        let repo = TempDir::new().expect("temporary repository");
        let first = write_committed_pack(repo.path(), "alpha-scope", "alpha-pack");
        let second = write_committed_pack(repo.path(), "beta-scope", "beta-pack");
        fs::remove_file(&second.summary_path).expect("remove second summary");
        fs::hard_link(&first.summary_path, &second.summary_path).expect("hard-link summary");
        refresh_launch_pin(&second);

        assert!(error_text(repo.path()).contains("duplicate committed execution-pack summary"));
    }
}
