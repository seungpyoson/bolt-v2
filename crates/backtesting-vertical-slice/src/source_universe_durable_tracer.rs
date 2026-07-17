//! Registry-derived RA-001a durable execution proof.
//!
//! The registry, rather than a venue allowlist, defines the complete proof
//! set. Each entry embeds the exact canonical one-record batch report and pins
//! the committed execution pack, its launch TOML, and that report by byte
//! length and SHA-256.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    atomic_artifact_write::{atomic_file_create_or_verify_guarded, open_pinned_regular_file},
    canonical_trades::{SourceAdapterKind, TRADE_TABLE_FAMILY, require_registered_source_adapter},
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    operator::{
        DURABLE_COMPLETION_MANIFEST_FILE, RunSpec, VerifiedSourceBindingRegistry,
        validate_durable_run_spec_preflight,
    },
    operator_work_budget::{
        CooperativeDeadlineWriter, OperatorWorkBudgetGuard, OperatorWorkBudgetStage,
    },
    path_resolution::resolve_pack_control_path,
    pinned_regular_file::read_exact_pinned_file,
    retired_backfill_evidence::ensure_active_backfill_runtime_path,
    source_universe_batch_execution::{
        SourceUniverseBatchExecutionRecordProvenance, SourceUniverseBatchExecutionReport,
        SourceUniverseBatchExecutionReportStatus, execution_record_digest,
        validate_source_universe_batch_execution_report,
    },
    source_universe_batch_launch::{
        CommittedSourceUniverseExecutionPack, discover_committed_source_universe_execution_packs,
    },
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackRecord,
        validate_execution_pack_semantics,
    },
};

pub const SOURCE_UNIVERSE_DURABLE_TRACER_RECEIPT_SET_SCHEMA_VERSION: &str =
    "source-universe-durable-tracer-receipt-set.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseDurableTracerReportInput {
    pub pack_id: String,
    pub report_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseDurableTracerArtifactPin {
    pub bytes: u64,
    pub sha256: String,
}

impl SourceUniverseDurableTracerArtifactPin {
    fn validate(&self, label: &str) -> Result<()> {
        ensure!(self.bytes > 0, "{label} byte length must be positive");
        ensure!(
            is_lowercase_sha256_hex(&self.sha256),
            "{label} SHA-256 must be lowercase hex"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseDurableTracerReceipt {
    pub pack_id: String,
    pub execution_pack: SourceUniverseDurableTracerArtifactPin,
    pub launch: SourceUniverseDurableTracerArtifactPin,
    pub batch_report_artifact: SourceUniverseDurableTracerArtifactPin,
    pub batch_report: SourceUniverseBatchExecutionReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseDurableTracerReceiptSet {
    pub schema_version: String,
    pub source_revision: String,
    pub worker_executable_sha256: String,
    pub receipts: Vec<SourceUniverseDurableTracerReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseDurableTracerReceiptSetArtifact {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceUniverseDurableTracerAggregateLimits {
    pub max_registry_packs: u64,
    pub max_total_selected_object_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceUniverseDurableTracerAggregateEnvelope {
    pub registry_packs: u64,
    pub total_selected_object_bytes: u64,
}

struct PinnedArtifact {
    bytes: Vec<u8>,
    pin: SourceUniverseDurableTracerArtifactPin,
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    file_identity: (u64, u64),
}

fn read_pinned_artifact(path: &Path, label: &str, max_bytes: u64) -> Result<PinnedArtifact> {
    ensure!(
        max_bytes > 0,
        "{label} maximum byte length must be positive"
    );
    let (mut file, identity) = open_pinned_regular_file(path)
        .with_context(|| format!("pin {label} {}", path.display()))?;
    ensure!(
        identity.byte_len > 0,
        "{label} {} must not be empty",
        path.display()
    );
    ensure!(
        identity.byte_len <= max_bytes,
        "{label} {} byte length {} exceeds configured maximum {max_bytes}",
        path.display(),
        identity.byte_len
    );
    let bytes = read_exact_pinned_file(&mut file, path, identity.byte_len)
        .with_context(|| format!("read pinned {label} {}", path.display()))?;
    identity
        .revalidate(path, &file)
        .with_context(|| format!("revalidate pinned {label} {}", path.display()))?;
    Ok(PinnedArtifact {
        pin: SourceUniverseDurableTracerArtifactPin {
            bytes: identity.byte_len,
            sha256: sha256_hex(&bytes),
        },
        bytes,
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        file_identity: (identity.device, identity.inode),
    })
}

fn parse_canonical_batch_report(
    path: &Path,
    artifact: &PinnedArtifact,
) -> Result<SourceUniverseBatchExecutionReport> {
    let report: SourceUniverseBatchExecutionReport = serde_json::from_slice(&artifact.bytes)
        .with_context(|| format!("parse batch report {}", path.display()))?;
    let canonical = crate::reference_artifact::canonical_json_bytes(&report)
        .with_context(|| format!("canonicalize batch report {}", path.display()))?;
    ensure!(
        canonical == artifact.bytes,
        "batch report {} bytes are not canonical",
        path.display()
    );
    Ok(report)
}

fn parse_execution_pack(
    committed: &CommittedSourceUniverseExecutionPack,
    artifact: &PinnedArtifact,
) -> Result<SourceUniverseExecutionPack> {
    let pack: SourceUniverseExecutionPack = serde_json::from_slice(&artifact.bytes)
        .with_context(|| format!("parse execution pack {}", committed.summary_path.display()))?;
    validate_execution_pack_semantics(&pack).with_context(|| {
        format!(
            "validate execution pack {}",
            committed.summary_path.display()
        )
    })?;
    ensure!(
        pack.pack_id == committed.pack_id,
        "committed execution-pack identity changed for {}",
        committed.pack_id
    );
    ensure!(
        artifact.pin.bytes == committed.launch_spec.execution_pack.bytes
            && artifact.pin.sha256 == committed.launch_spec.execution_pack.sha256,
        "committed execution-pack bytes or SHA-256 disagree with its launch pin for {}",
        committed.pack_id
    );
    Ok(pack)
}

fn launch_selected_record<'a>(
    committed: &CommittedSourceUniverseExecutionPack,
    pack: &'a SourceUniverseExecutionPack,
) -> Result<&'a SourceUniverseExecutionPackRecord> {
    ensure!(
        committed.launch_spec.record_limit == Some(1),
        "RA-001a launch for {} must select exactly one record",
        pack.pack_id
    );
    pack.records
        .iter()
        .find(|record| {
            committed
                .launch_spec
                .start_sequence
                .is_none_or(|start| record.sequence >= start)
        })
        .with_context(|| {
            format!(
                "RA-001a launch for {} selects no materialized record from start_sequence {:?}",
                pack.pack_id, committed.launch_spec.start_sequence
            )
        })
}

fn validate_selected_run_spec_scope(
    committed: &CommittedSourceUniverseExecutionPack,
    pack: &SourceUniverseExecutionPack,
    record: &SourceUniverseExecutionPackRecord,
) -> Result<()> {
    ensure!(
        pack.table_family == TRADE_TABLE_FAMILY,
        "RA-001a committed pack {} table_family must be {}",
        pack.pack_id,
        TRADE_TABLE_FAMILY
    );
    let pack_base_dir = committed
        .summary_path
        .parent()
        .context("committed execution-pack summary has no parent")?;

    ensure_active_backfill_runtime_path(&record.run_spec_path)?;
    let run_spec_path = resolve_pack_control_path(pack_base_dir, &record.run_spec_path)?;
    let run_spec_artifact = read_pinned_artifact(
        &run_spec_path,
        "selected run spec",
        committed
            .launch_spec
            .bootstrap_limits
            .max_control_artifact_bytes,
    )?;
    ensure!(
        run_spec_artifact.pin.bytes == record.run_spec_bytes
            && run_spec_artifact.pin.sha256 == record.run_spec_sha256,
        "selected run-spec byte/SHA-256 pin mismatch for {} sequence {}",
        pack.pack_id,
        record.sequence
    );
    let run_spec: RunSpec = toml::from_slice(&run_spec_artifact.bytes)
        .with_context(|| format!("parse selected run spec {}", run_spec_path.display()))?;
    ensure!(
        run_spec.source_bindings_path == record.source_bindings_path,
        "selected run spec source-bindings path disagrees with committed pack {} sequence {}",
        pack.pack_id,
        record.sequence
    );

    ensure_active_backfill_runtime_path(&record.source_bindings_path)?;
    let source_bindings_path =
        resolve_pack_control_path(pack_base_dir, &record.source_bindings_path)?;
    let source_bindings_artifact = read_pinned_artifact(
        &source_bindings_path,
        "selected source-bindings registry",
        committed
            .launch_spec
            .bootstrap_limits
            .max_control_artifact_bytes,
    )?;
    ensure!(
        source_bindings_artifact.pin.bytes == record.source_bindings_bytes
            && source_bindings_artifact.pin.sha256 == record.source_bindings_sha256,
        "selected source-bindings byte/SHA-256 pin mismatch for {} sequence {}",
        pack.pack_id,
        record.sequence
    );
    let registry = VerifiedSourceBindingRegistry::from_frozen_pack_bytes(
        &run_spec,
        source_bindings_path,
        Arc::from(source_bindings_artifact.bytes),
        &record.source_bindings_sha256,
    )?;
    validate_durable_run_spec_preflight(&run_spec, &registry).with_context(|| {
        format!(
            "validate durable selected run spec for {} sequence {}",
            pack.pack_id, record.sequence
        )
    })?;
    let adapter = require_registered_source_adapter(
        &run_spec.converter.identity,
        &run_spec.converter.version,
    )?;
    ensure!(
        adapter.kind == SourceAdapterKind::CsvNativeTrades
            && adapter.table_family == TRADE_TABLE_FAMILY,
        "RA-001a selected adapter for {} sequence {} must be CSV-native trades",
        pack.pack_id,
        record.sequence
    );
    Ok(())
}

/// Preflight the complete registry before any source process starts. Breadth
/// remains registry-derived, while the aggregate count and selected network
/// bytes must fit explicit operator-owned cost limits.
pub fn validate_source_universe_durable_tracer_aggregate_limits(
    committed: &[CommittedSourceUniverseExecutionPack],
    limits: SourceUniverseDurableTracerAggregateLimits,
) -> Result<SourceUniverseDurableTracerAggregateEnvelope> {
    ensure!(
        limits.max_registry_packs > 0,
        "RA-001a max_registry_packs must be positive"
    );
    ensure!(
        limits.max_total_selected_object_bytes > 0,
        "RA-001a max_total_selected_object_bytes must be positive"
    );
    let registry_packs = u64::try_from(committed.len())
        .context("RA-001a committed registry count does not fit u64")?;
    ensure!(
        registry_packs > 0 && registry_packs <= limits.max_registry_packs,
        "RA-001a registry pack count {registry_packs} exceeds max_registry_packs {}",
        limits.max_registry_packs
    );

    let mut total_selected_object_bytes = 0_u64;
    for pack in committed {
        let execution_pack = read_pinned_artifact(
            &pack.summary_path,
            "execution pack",
            pack.launch_spec.execution_pack.bytes,
        )?;
        let parsed = parse_execution_pack(pack, &execution_pack)?;
        let selected = launch_selected_record(pack, &parsed)?;
        validate_selected_run_spec_scope(pack, &parsed, selected)?;
        total_selected_object_bytes = total_selected_object_bytes
            .checked_add(selected.selected_object_bytes)
            .context("RA-001a total selected-object byte count overflow")?;
        ensure!(
            total_selected_object_bytes <= limits.max_total_selected_object_bytes,
            "RA-001a total selected-object bytes {total_selected_object_bytes} exceed max_total_selected_object_bytes {}",
            limits.max_total_selected_object_bytes
        );
    }

    Ok(SourceUniverseDurableTracerAggregateEnvelope {
        registry_packs,
        total_selected_object_bytes,
    })
}

fn expected_pack_ids(committed: &[CommittedSourceUniverseExecutionPack]) -> BTreeSet<String> {
    committed.iter().map(|pack| pack.pack_id.clone()).collect()
}

fn index_report_inputs<'a>(
    inputs: &'a [SourceUniverseDurableTracerReportInput],
) -> Result<BTreeMap<&'a str, &'a Path>> {
    let mut indexed = BTreeMap::new();
    for input in inputs {
        ensure!(
            !input.pack_id.trim().is_empty(),
            "batch report pack_id must not be empty"
        );
        ensure!(
            input.report_path.is_absolute(),
            "batch report path must be absolute: {}",
            input.report_path.display()
        );
        ensure!(
            indexed
                .insert(input.pack_id.as_str(), input.report_path.as_path())
                .is_none(),
            "duplicate batch report pack identity {}",
            input.pack_id
        );
    }
    Ok(indexed)
}

fn validate_exact_pack_set(
    expected: &BTreeSet<String>,
    actual: &BTreeSet<String>,
    label: &str,
) -> Result<()> {
    let missing = expected.difference(actual).cloned().collect::<Vec<_>>();
    let extra = actual.difference(expected).cloned().collect::<Vec<_>>();
    ensure!(
        missing.is_empty(),
        "{label} is missing committed pack identities: {}",
        missing.join(", ")
    );
    ensure!(
        extra.is_empty(),
        "{label} has extra unregistered pack identities: {}",
        extra.join(", ")
    );
    Ok(())
}

fn validate_source_revision(source_revision: &str) -> Result<()> {
    ensure!(
        matches!(source_revision.len(), 40 | 64)
            && source_revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "source revision must be 40 or 64 lowercase hexadecimal characters"
    );
    Ok(())
}

fn resolve_git_executable() -> Result<PathBuf> {
    let path = env::var_os("PATH").context("PATH is required to resolve the Git executable")?;
    for directory in env::split_paths(&path) {
        ensure!(
            directory.is_absolute(),
            "PATH contains a relative entry; refusing ambiguous Git executable resolution"
        );
        let candidate = directory.join("git");
        if !candidate.is_file() {
            continue;
        }
        let resolved = candidate
            .canonicalize()
            .with_context(|| format!("canonicalize Git executable {}", candidate.display()))?;
        ensure!(
            resolved.is_file(),
            "resolved Git executable {} is not one regular file",
            resolved.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = resolved
                .metadata()
                .with_context(|| format!("read Git executable metadata {}", resolved.display()))?
                .permissions()
                .mode();
            ensure!(
                mode & 0o111 != 0,
                "resolved Git executable {} is not executable",
                resolved.display()
            );
        }
        return Ok(resolved);
    }
    anyhow::bail!("Git executable was not found on the absolute PATH")
}

fn git_checkout_command(git_executable: &Path, repo_root: &Path) -> Command {
    let mut command = Command::new(git_executable);
    command
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn run_git_stdout_bounded(
    git_executable: &Path,
    repo_root: &Path,
    args: &[&str],
    max_stdout_bytes: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let mut command = git_checkout_command(git_executable, repo_root);
    command.args(args).stdout(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("start git {label} for {}", repo_root.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .context("git stdout pipe was not available")?;
    let mut bytes = Vec::with_capacity(max_stdout_bytes.saturating_add(1));
    stdout
        .by_ref()
        .take(max_stdout_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read git {label} output for {}", repo_root.display()))?;
    if bytes.len() > max_stdout_bytes {
        let _ = child.kill();
        let _ = child.wait();
        ensure!(
            false,
            "git {label} output exceeded the fail-closed byte bound"
        );
    }
    let status = child
        .wait()
        .with_context(|| format!("wait for git {label} in {}", repo_root.display()))?;
    ensure!(
        status.success(),
        "git {label} failed for {} with {status}",
        repo_root.display()
    );
    Ok(bytes)
}

fn ensure_git_index_has_no_hidden_paths(git_executable: &Path, repo_root: &Path) -> Result<()> {
    let mut command = git_checkout_command(git_executable, repo_root);
    command
        .args(["ls-files", "-v", "-z"])
        .stdout(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("start git ls-files for {}", repo_root.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .context("git ls-files stdout pipe was not available")?;
    let mut buffer = [0_u8; 8 * 1024];
    let mut record_offset = 0_usize;
    let mut failure = None;
    'stream: loop {
        let bytes_read = stdout
            .read(&mut buffer)
            .with_context(|| format!("read git ls-files for {}", repo_root.display()))?;
        if bytes_read == 0 {
            break;
        }
        for byte in &buffer[..bytes_read] {
            if *byte == 0 {
                if record_offset < 3 {
                    failure = Some("git ls-files returned a malformed tracked-path record");
                    break 'stream;
                }
                record_offset = 0;
                continue;
            }
            if record_offset == 0 {
                if *byte == b'S' || byte.is_ascii_lowercase() {
                    failure = Some(
                        "RA-001a checkout index flags can hide tracked changes; clear assume-unchanged and skip-worktree flags",
                    );
                    break 'stream;
                }
                if !byte.is_ascii_uppercase() {
                    failure = Some("git ls-files returned an unknown tracked-path status tag");
                    break 'stream;
                }
            } else if record_offset == 1 && *byte != b' ' {
                failure = Some("git ls-files returned a malformed tracked-path separator");
                break 'stream;
            }
            record_offset = record_offset.saturating_add(1);
        }
    }
    if failure.is_none() && record_offset != 0 {
        failure = Some("git ls-files returned an unterminated tracked-path record");
    }
    if let Some(message) = failure {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!(message);
    }
    let status = child
        .wait()
        .with_context(|| format!("wait for git ls-files in {}", repo_root.display()))?;
    ensure!(
        status.success(),
        "git ls-files failed for {} with {status}",
        repo_root.display()
    );
    Ok(())
}

fn ensure_git_checkout_has_no_changes(git_executable: &Path, repo_root: &Path) -> Result<()> {
    let mut command = git_checkout_command(git_executable, repo_root);
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
        .stdout(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("start git status for {}", repo_root.display()))?;
    let mut first_byte = [0_u8; 1];
    let dirty = child
        .stdout
        .take()
        .context("git status stdout pipe was not available")?
        .read(&mut first_byte)
        .with_context(|| format!("read git status for {}", repo_root.display()))?
        > 0;
    if dirty {
        let _ = child.kill();
        let _ = child.wait();
        ensure!(
            false,
            "RA-001a checkout contains tracked or untracked changes"
        );
    }
    let status = child
        .wait()
        .with_context(|| format!("wait for git status in {}", repo_root.display()))?;
    ensure!(
        status.success(),
        "git status failed for {} with {status}",
        repo_root.display()
    );
    Ok(())
}

fn is_allowed_ignored_runtime_output(path: &[u8]) -> bool {
    const ALLOWED_ROOTS: [&[u8]; 4] = [
        b"target/",
        b".nextest-archive/",
        b".rust-verification/",
        b"scripts/__pycache__/",
    ];
    ALLOWED_ROOTS
        .iter()
        .any(|root| path == *root || path.starts_with(root))
}

fn ensure_git_ignored_entries_are_runtime_outputs(
    git_executable: &Path,
    repo_root: &Path,
) -> Result<()> {
    const MAX_IGNORED_ENTRY_BYTES: usize = 4096;
    const MAX_IGNORED_ENTRIES: u64 = 128;

    let mut command = git_checkout_command(git_executable, repo_root);
    command
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
            "-z",
        ])
        .stdout(Stdio::piped());
    let mut child = command.spawn().with_context(|| {
        format!(
            "start git ignored-path inventory for {}",
            repo_root.display()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .context("git ignored-path inventory stdout pipe was not available")?;
    let mut reader = BufReader::new(stdout);
    let mut entry = Vec::new();
    let mut entries = 0_u64;
    loop {
        entry.clear();
        let bytes_read = match reader.read_until(0, &mut entry) {
            Ok(bytes_read) => bytes_read,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).with_context(|| {
                    format!(
                        "read git ignored-path inventory for {}",
                        repo_root.display()
                    )
                });
            }
        };
        if bytes_read == 0 {
            break;
        }
        if entry.last() != Some(&0) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("git ignored-path inventory returned an unterminated entry");
        }
        entry.pop();
        entries = entries
            .checked_add(1)
            .context("git ignored-path inventory count overflow")?;
        if entry.is_empty()
            || entry.len() > MAX_IGNORED_ENTRY_BYTES
            || entries > MAX_IGNORED_ENTRIES
            || !is_allowed_ignored_runtime_output(&entry)
        {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "RA-001a checkout contains an ignored non-runtime-output path or exceeds the ignored-path inventory bound"
            );
        }
    }
    let status = child.wait().with_context(|| {
        format!(
            "wait for git ignored-path inventory in {}",
            repo_root.display()
        )
    })?;
    ensure!(
        status.success(),
        "git ignored-path inventory failed for {} with {status}",
        repo_root.display()
    );
    Ok(())
}

fn parse_single_git_line(bytes: Vec<u8>, label: &str) -> Result<String> {
    let output = String::from_utf8(bytes).with_context(|| format!("git {label} was not UTF-8"))?;
    let line = output.strip_suffix('\n').unwrap_or(output.as_str());
    let line = line.strip_suffix('\r').unwrap_or(line);
    ensure!(
        !line.is_empty() && !line.bytes().any(|byte| matches!(byte, b'\r' | b'\n')),
        "git {label} must return exactly one non-empty line"
    );
    Ok(line.to_string())
}

/// Bind an RA-001a proof to one exact, clean repository checkout.
///
/// Git is invoked directly without a shell. Repository-redirection variables
/// are removed, the requested root must be the exact Git top level, `HEAD`
/// must equal the caller's revision before and after the cleanliness check,
/// tracked and untracked changes fail closed, and ignored entries are accepted
/// only under the named generated-output roots required by the proof lane.
pub fn verify_source_universe_durable_tracer_checkout(
    repo_root: &Path,
    expected_source_revision: &str,
) -> Result<()> {
    validate_source_revision(expected_source_revision)?;
    ensure!(
        repo_root.is_absolute(),
        "RA-001a repository root must be absolute"
    );
    let canonical_root = repo_root.canonicalize().with_context(|| {
        format!(
            "canonicalize RA-001a repository root {}",
            repo_root.display()
        )
    })?;
    ensure!(
        canonical_root.is_dir(),
        "RA-001a repository root must be one directory"
    );
    let canonical_root_text = canonical_root
        .to_str()
        .context("RA-001a repository root must be UTF-8")?;
    let git_executable = resolve_git_executable()?;
    let top_level = parse_single_git_line(
        run_git_stdout_bounded(
            &git_executable,
            &canonical_root,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
            canonical_root_text.len().saturating_add(2),
            "repository-root resolution",
        )?,
        "repository-root resolution",
    )?;
    let resolved_top_level = Path::new(&top_level)
        .canonicalize()
        .with_context(|| format!("canonicalize Git top level {top_level}"))?;
    ensure!(
        resolved_top_level == canonical_root,
        "RA-001a repository root {} is not the exact Git top level {}",
        canonical_root.display(),
        resolved_top_level.display()
    );

    let read_head = || -> Result<String> {
        parse_single_git_line(
            run_git_stdout_bounded(
                &git_executable,
                &canonical_root,
                &["rev-parse", "--verify", "HEAD^{commit}"],
                65,
                "HEAD resolution",
            )?,
            "HEAD resolution",
        )
    };
    let head_before_status = read_head()?;
    validate_source_revision(&head_before_status)?;
    ensure!(
        head_before_status == expected_source_revision,
        "checkout HEAD {head_before_status} does not match expected source revision {expected_source_revision}"
    );
    ensure_git_index_has_no_hidden_paths(&git_executable, &canonical_root)?;
    ensure_git_checkout_has_no_changes(&git_executable, &canonical_root)?;
    ensure_git_ignored_entries_are_runtime_outputs(&git_executable, &canonical_root)?;
    let head_after_status = read_head()?;
    ensure!(
        head_after_status == expected_source_revision,
        "checkout HEAD {head_after_status} does not match expected source revision {expected_source_revision} after cleanliness verification"
    );
    Ok(())
}

/// Consume exactly one canonical completed report for every committed pack.
pub fn build_source_universe_durable_tracer_receipt_set(
    repo_root: &Path,
    source_revision: &str,
    expected_worker_executable_sha256: &str,
    report_inputs: &[SourceUniverseDurableTracerReportInput],
) -> Result<SourceUniverseDurableTracerReceiptSet> {
    validate_source_revision(source_revision)?;
    ensure!(
        is_lowercase_sha256_hex(expected_worker_executable_sha256),
        "expected worker executable SHA-256 must be lowercase hex"
    );
    let committed = discover_committed_source_universe_execution_packs(repo_root)?;
    let expected_ids = expected_pack_ids(&committed);
    let indexed_inputs = index_report_inputs(report_inputs)?;
    let actual_ids = indexed_inputs
        .keys()
        .map(|pack_id| (*pack_id).to_string())
        .collect::<BTreeSet<_>>();
    validate_exact_pack_set(&expected_ids, &actual_ids, "batch report set")?;

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    let mut report_file_identities = BTreeSet::new();
    let mut receipts = Vec::with_capacity(committed.len());
    for pack in committed {
        let report_path = indexed_inputs
            .get(pack.pack_id.as_str())
            .expect("exact report set contains every committed pack");
        let execution_pack = read_pinned_artifact(
            &pack.summary_path,
            "execution pack",
            pack.launch_spec.execution_pack.bytes,
        )?;
        parse_execution_pack(&pack, &execution_pack)?;
        let launch = read_pinned_artifact(
            &pack.launch_path,
            "batch launch spec",
            pack.launch_spec.bootstrap_limits.max_launch_artifact_bytes,
        )?;
        let report_artifact = read_pinned_artifact(
            report_path,
            "batch report",
            pack.launch_spec.bootstrap_limits.max_control_artifact_bytes,
        )?;
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        ensure!(
            report_file_identities.insert(report_artifact.file_identity),
            "duplicate batch report file identity for committed pack {}",
            pack.pack_id
        );
        let batch_report = parse_canonical_batch_report(report_path, &report_artifact)?;
        receipts.push(SourceUniverseDurableTracerReceipt {
            pack_id: pack.pack_id,
            execution_pack: execution_pack.pin,
            launch: launch.pin,
            batch_report_artifact: report_artifact.pin,
            batch_report,
        });
    }
    receipts.sort_by(|left, right| left.pack_id.cmp(&right.pack_id));
    let receipt_set = SourceUniverseDurableTracerReceiptSet {
        schema_version: SOURCE_UNIVERSE_DURABLE_TRACER_RECEIPT_SET_SCHEMA_VERSION.to_string(),
        source_revision: source_revision.to_string(),
        worker_executable_sha256: expected_worker_executable_sha256.to_string(),
        receipts,
    };
    validate_source_universe_durable_tracer_receipt_set(
        repo_root,
        source_revision,
        expected_worker_executable_sha256,
        &receipt_set,
    )?;
    Ok(receipt_set)
}

fn validate_report_against_pack(
    expected_worker_executable_sha256: &str,
    committed: &CommittedSourceUniverseExecutionPack,
    pack: &SourceUniverseExecutionPack,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<()> {
    ensure!(
        report.status == SourceUniverseBatchExecutionReportStatus::Completed
            && report.selected_record_count == 1
            && report.completed_record_count == 1
            && report.failed_record_count == 0
            && report.records.len() == 1
            && report.failures.is_empty(),
        "RA-001a batch report for {} must contain exactly one completed record and zero failures",
        pack.pack_id
    );
    ensure!(
        report.pack_id == pack.pack_id
            && report.batch_id == committed.launch_spec.batch_id
            && report.universe_id == pack.universe_id
            && report.venue == pack.venue,
        "RA-001a batch report identity disagrees with committed pack {}",
        pack.pack_id
    );
    let expected = launch_selected_record(committed, pack)?;
    validate_selected_run_spec_scope(committed, pack, expected)?;
    let actual = &report.records[0];
    validate_report_record_exact_fields(expected, actual)?;
    ensure!(
        actual.execution_record_sha256 == execution_record_digest(pack, expected.sequence)?,
        "RA-001a report execution-record SHA-256 disagrees with committed pack {}",
        pack.pack_id
    );
    ensure!(
        actual.canonical_rows > 0 && actual.nt_catalog_rows > 0,
        "RA-001a report for {} must contain positive canonical and NT rows",
        pack.pack_id
    );
    ensure!(
        actual.completion_provenance
            == SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated,
        "RA-001a report for {} must have executed_process_isolated completion provenance",
        pack.pack_id
    );
    ensure!(
        actual.attempt_worker_sha256 == expected_worker_executable_sha256,
        "RA-001a report attempt worker executable SHA-256 disagrees with the exact-current expected digest for {}",
        pack.pack_id
    );
    let durable_completion = actual
        .durable_completion
        .as_ref()
        .context("RA-001a completed record is missing its durable locator")?;
    durable_completion.validate()?;
    let expected_completion_uri = format!(
        "{}/{}",
        expected.output_prefix.trim_end_matches('/'),
        DURABLE_COMPLETION_MANIFEST_FILE
    );
    ensure!(
        durable_completion.object.uri == expected_completion_uri,
        "RA-001a durable locator URI disagrees with the committed output prefix for {}",
        pack.pack_id
    );
    Ok(())
}

fn validate_report_record_exact_fields(
    expected: &SourceUniverseExecutionPackRecord,
    actual: &crate::source_universe_batch_execution::SourceUniverseBatchExecutionRecord,
) -> Result<()> {
    ensure!(
        actual.sequence == expected.sequence
            && actual.operator_run_id == expected.operator_run_id
            && actual.source_binding == expected.source_binding
            && actual.category == expected.category
            && actual.symbol == expected.symbol
            && actual.archive_date == expected.archive_date
            && actual.selected_object_sha256 == expected.selected_object_sha256
            && actual.selected_object_bytes == expected.selected_object_bytes
            && actual.run_spec_sha256 == expected.run_spec_sha256
            && actual.accepted_tranche_sha256 == expected.accepted_tranche_sha256
            && actual.execution_plan_sha256 == expected.execution_plan_sha256
            && actual.source_bindings_sha256 == expected.source_bindings_sha256,
        "RA-001a completed report record does not exactly match its committed execution-pack record"
    );
    Ok(())
}

/// Revalidate a portable receipt set against the current complete registry.
pub fn validate_source_universe_durable_tracer_receipt_set(
    repo_root: &Path,
    expected_source_revision: &str,
    expected_worker_executable_sha256: &str,
    receipt_set: &SourceUniverseDurableTracerReceiptSet,
) -> Result<()> {
    validate_source_revision(expected_source_revision)?;
    ensure!(
        is_lowercase_sha256_hex(expected_worker_executable_sha256),
        "expected worker executable SHA-256 must be lowercase hex"
    );
    ensure!(
        receipt_set.schema_version == SOURCE_UNIVERSE_DURABLE_TRACER_RECEIPT_SET_SCHEMA_VERSION,
        "durable tracer receipt-set schema_version mismatch"
    );
    ensure!(
        receipt_set.source_revision == expected_source_revision,
        "durable tracer receipt-set source revision mismatch"
    );
    ensure!(
        receipt_set.worker_executable_sha256 == expected_worker_executable_sha256,
        "durable tracer receipt-set worker executable SHA-256 mismatch"
    );
    ensure!(
        receipt_set
            .receipts
            .windows(2)
            .all(|pair| pair[0].pack_id < pair[1].pack_id),
        "durable tracer receipts must have strict pack_id order without duplicates"
    );

    let committed = discover_committed_source_universe_execution_packs(repo_root)?;
    let expected_ids = expected_pack_ids(&committed);
    let actual_ids = receipt_set
        .receipts
        .iter()
        .map(|receipt| receipt.pack_id.clone())
        .collect::<BTreeSet<_>>();
    validate_exact_pack_set(&expected_ids, &actual_ids, "durable tracer receipt set")?;
    let receipts_by_pack = receipt_set
        .receipts
        .iter()
        .map(|receipt| (receipt.pack_id.as_str(), receipt))
        .collect::<BTreeMap<_, _>>();

    for pack in committed {
        let receipt = receipts_by_pack
            .get(pack.pack_id.as_str())
            .expect("exact receipt set contains every committed pack");
        receipt.execution_pack.validate("execution-pack artifact")?;
        receipt.launch.validate("launch artifact")?;
        receipt
            .batch_report_artifact
            .validate("batch-report artifact")?;

        let execution_pack_artifact = read_pinned_artifact(
            &pack.summary_path,
            "execution pack",
            pack.launch_spec.execution_pack.bytes,
        )?;
        ensure!(
            receipt.execution_pack == execution_pack_artifact.pin,
            "execution-pack byte/SHA-256 pin mismatch for {}",
            pack.pack_id
        );
        let execution_pack = parse_execution_pack(&pack, &execution_pack_artifact)?;
        let launch_artifact = read_pinned_artifact(
            &pack.launch_path,
            "batch launch spec",
            pack.launch_spec.bootstrap_limits.max_launch_artifact_bytes,
        )?;
        ensure!(
            receipt.launch == launch_artifact.pin,
            "launch byte/SHA-256 pin mismatch for {}",
            pack.pack_id
        );

        validate_source_universe_batch_execution_report(&receipt.batch_report)?;
        validate_report_against_pack(
            expected_worker_executable_sha256,
            &pack,
            &execution_pack,
            &receipt.batch_report,
        )?;
        let canonical_report =
            crate::reference_artifact::canonical_json_bytes(&receipt.batch_report)
                .context("canonicalize embedded batch report")?;
        let canonical_report_bytes = u64::try_from(canonical_report.len())
            .context("canonical batch report byte length exceeds u64")?;
        ensure!(
            receipt.batch_report_artifact.bytes == canonical_report_bytes
                && receipt.batch_report_artifact.sha256 == sha256_hex(&canonical_report),
            "batch-report byte/SHA-256 pin mismatch for {}",
            pack.pack_id
        );
    }
    Ok(())
}

/// Parse canonical JSON and validate every embedded and registry-derived pin.
pub fn parse_and_validate_source_universe_durable_tracer_receipt_set(
    repo_root: &Path,
    expected_source_revision: &str,
    expected_worker_executable_sha256: &str,
    bytes: &[u8],
) -> Result<SourceUniverseDurableTracerReceiptSet> {
    let receipt_set: SourceUniverseDurableTracerReceiptSet =
        serde_json::from_slice(bytes).context("parse durable tracer receipt set")?;
    let canonical = crate::reference_artifact::canonical_json_bytes(&receipt_set)
        .context("canonicalize durable tracer receipt set")?;
    ensure!(
        canonical == bytes,
        "durable tracer receipt-set bytes are not canonical"
    );
    validate_source_universe_durable_tracer_receipt_set(
        repo_root,
        expected_source_revision,
        expected_worker_executable_sha256,
        &receipt_set,
    )?;
    Ok(receipt_set)
}

/// Publish one immutable canonical local receipt set.
pub fn write_source_universe_durable_tracer_receipt_set(
    path: &Path,
    repo_root: &Path,
    expected_source_revision: &str,
    expected_worker_executable_sha256: &str,
    receipt_set: &SourceUniverseDurableTracerReceiptSet,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<SourceUniverseDurableTracerReceiptSetArtifact> {
    ensure!(
        path.is_absolute(),
        "durable tracer receipt-set path must be absolute: {}",
        path.display()
    );
    validate_source_universe_durable_tracer_receipt_set(
        repo_root,
        expected_source_revision,
        expected_worker_executable_sha256,
        receipt_set,
    )?;
    let bytes = crate::reference_artifact::canonical_json_bytes(receipt_set)
        .context("serialize canonical durable tracer receipt set")?;
    let byte_len = u64::try_from(bytes.len()).context("receipt-set byte length exceeds u64")?;
    work_budget.verify_decoded_bytes(byte_len, OperatorWorkBudgetStage::Publish)?;
    atomic_file_create_or_verify_guarded(
        path,
        work_budget,
        OperatorWorkBudgetStage::Publish,
        |file| {
            let mut writer =
                CooperativeDeadlineWriter::new(file, work_budget, OperatorWorkBudgetStage::Publish);
            writer
                .write_all(&bytes)
                .context("write durable tracer receipt set")?;
            writer.flush().context("flush durable tracer receipt set")?;
            Ok(())
        },
    )
    .with_context(|| format!("publish durable tracer receipt set {}", path.display()))?;
    Ok(SourceUniverseDurableTracerReceiptSetArtifact {
        path: path.to_path_buf(),
        bytes: byte_len,
        sha256: sha256_hex(&bytes),
    })
}

/// Reopen one just-published receipt through its exact byte/hash pin.
pub fn read_and_validate_source_universe_durable_tracer_receipt_set(
    repo_root: &Path,
    expected_source_revision: &str,
    expected_worker_executable_sha256: &str,
    artifact: &SourceUniverseDurableTracerReceiptSetArtifact,
) -> Result<SourceUniverseDurableTracerReceiptSet> {
    ensure!(
        artifact.path.is_absolute(),
        "durable tracer receipt-set path must be absolute: {}",
        artifact.path.display()
    );
    ensure!(
        artifact.bytes > 0 && is_lowercase_sha256_hex(&artifact.sha256),
        "durable tracer receipt-set artifact pin is invalid"
    );
    let pinned =
        read_pinned_artifact(&artifact.path, "durable tracer receipt set", artifact.bytes)?;
    ensure!(
        pinned.pin.bytes == artifact.bytes && pinned.pin.sha256 == artifact.sha256,
        "durable tracer receipt-set byte/SHA-256 pin mismatch"
    );
    parse_and_validate_source_universe_durable_tracer_receipt_set(
        repo_root,
        expected_source_revision,
        expected_worker_executable_sha256,
        &pinned.bytes,
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use tempfile::TempDir;

    use crate::{
        hashing::sha256_hex,
        operator::{
            DURABLE_COMPLETION_MANIFEST_FILE, DurableCompletionLocator,
            DurableObjectVersionIdentity,
        },
        operator_work_budget::OperatorWorkBudgetGuard,
        source_universe_batch_execution::{
            SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION,
            SourceUniverseBatchExecutionCompletionResolution, SourceUniverseBatchExecutionRecord,
            SourceUniverseBatchExecutionRecordProvenance, SourceUniverseBatchExecutionReport,
            SourceUniverseBatchExecutionReportStatus, execution_record_digest,
        },
        source_universe_batch_launch::discover_committed_source_universe_execution_packs,
        source_universe_execution_pack::SourceUniverseExecutionPack,
    };

    use super::{
        SourceUniverseDurableTracerAggregateLimits, SourceUniverseDurableTracerReportInput,
        build_source_universe_durable_tracer_receipt_set,
        parse_and_validate_source_universe_durable_tracer_receipt_set,
        read_and_validate_source_universe_durable_tracer_receipt_set,
        validate_source_universe_durable_tracer_aggregate_limits,
        validate_source_universe_durable_tracer_receipt_set,
        verify_source_universe_durable_tracer_checkout,
        write_source_universe_durable_tracer_receipt_set,
    };

    const EXPECTED_WORKER_SHA256: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const EXPECTED_SOURCE_REVISION: &str = "1111111111111111111111111111111111111111";
    const CATALOG_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const COMPLETION_SHA256: &str =
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crate is nested below repository root")
            .to_path_buf()
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("start git fixture command");
        assert!(
            output.status.success(),
            "git fixture command {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn initialized_git_repo() -> (TempDir, String) {
        let temp = TempDir::new().expect("create git fixture");
        run_git(temp.path(), &["init", "--quiet"]);
        run_git(temp.path(), &["config", "user.name", "RA-001a Test"]);
        run_git(
            temp.path(),
            &["config", "user.email", "ra001a-test@example.invalid"],
        );
        fs::write(temp.path().join("tracked.txt"), b"committed\n").expect("write tracked fixture");
        run_git(temp.path(), &["add", "--", "tracked.txt"]);
        run_git(
            temp.path(),
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ],
        );
        let head = String::from_utf8(run_git(temp.path(), &["rev-parse", "HEAD"]).stdout)
            .expect("git fixture HEAD is UTF-8")
            .trim()
            .to_string();
        (temp, head)
    }

    #[test]
    fn checkout_verifier_accepts_exact_clean_revision() {
        let (temp, head) = initialized_git_repo();

        verify_source_universe_durable_tracer_checkout(temp.path(), &head)
            .expect("accept exact clean checkout");
    }

    #[test]
    fn checkout_verifier_rejects_revision_mismatch() {
        let (temp, _) = initialized_git_repo();

        let error = verify_source_universe_durable_tracer_checkout(
            temp.path(),
            "2222222222222222222222222222222222222222",
        )
        .expect_err("reject caller revision that is not checkout HEAD");

        assert!(
            error
                .to_string()
                .contains("does not match expected source revision")
        );
    }

    #[test]
    fn checkout_verifier_rejects_tracked_changes() {
        let (temp, head) = initialized_git_repo();
        fs::write(temp.path().join("tracked.txt"), b"modified\n").expect("modify tracked fixture");

        let error = verify_source_universe_durable_tracer_checkout(temp.path(), &head)
            .expect_err("reject tracked checkout change");

        assert!(error.to_string().contains("tracked or untracked changes"));
    }

    #[test]
    fn checkout_verifier_rejects_untracked_changes() {
        let (temp, head) = initialized_git_repo();
        fs::write(temp.path().join("untracked.txt"), b"untracked\n")
            .expect("write untracked fixture");

        let error = verify_source_universe_durable_tracer_checkout(temp.path(), &head)
            .expect_err("reject untracked checkout change");

        assert!(error.to_string().contains("tracked or untracked changes"));
    }

    #[test]
    fn checkout_verifier_rejects_ignored_source_affecting_paths() {
        let (temp, head) = initialized_git_repo();
        fs::write(temp.path().join(".git/info/exclude"), b"build.rs\n")
            .expect("ignore synthetic build script");
        fs::write(temp.path().join("build.rs"), b"fn main() {}\n")
            .expect("write ignored build script");

        let error = verify_source_universe_durable_tracer_checkout(temp.path(), &head)
            .expect_err("reject ignored source-affecting path");

        assert!(
            error
                .to_string()
                .contains("ignored non-runtime-output path")
        );
    }

    #[test]
    fn checkout_verifier_allows_only_named_ignored_runtime_output_roots() {
        let (temp, head) = initialized_git_repo();
        fs::write(
            temp.path().join(".git/info/exclude"),
            b"target/\n.nextest-archive/\nscripts/__pycache__/\n",
        )
        .expect("ignore generated runtime output roots");
        for relative in [
            "target/debug/object",
            ".nextest-archive/archive",
            "scripts/__pycache__/module.pyc",
        ] {
            let path = temp.path().join(relative);
            fs::create_dir_all(path.parent().expect("generated path parent"))
                .expect("create generated output parent");
            fs::write(path, b"generated").expect("write generated output");
        }

        verify_source_universe_durable_tracer_checkout(temp.path(), &head)
            .expect("accept only governed ignored runtime output roots");
    }

    #[test]
    fn checkout_verifier_rejects_index_flags_that_hide_tracked_changes() {
        let (temp, head) = initialized_git_repo();
        run_git(
            temp.path(),
            &["update-index", "--assume-unchanged", "tracked.txt"],
        );
        fs::write(temp.path().join("tracked.txt"), b"hidden modification\n")
            .expect("modify assume-unchanged fixture");

        let error = verify_source_universe_durable_tracer_checkout(temp.path(), &head)
            .expect_err("reject index flag that can hide a tracked checkout change");

        assert!(
            error
                .to_string()
                .contains("index flags can hide tracked changes")
        );
    }

    fn write_canonical_report(path: &std::path::Path, report: &SourceUniverseBatchExecutionReport) {
        let bytes = crate::reference_artifact::canonical_json_bytes(report)
            .expect("serialize canonical batch report");
        fs::write(path, bytes).expect("write batch report");
    }

    fn complete_registry_reports(
        temp: &TempDir,
    ) -> (
        Vec<SourceUniverseDurableTracerReportInput>,
        Vec<SourceUniverseBatchExecutionReport>,
    ) {
        let committed = discover_committed_source_universe_execution_packs(&repo_root())
            .expect("discover committed execution packs");
        let mut inputs = Vec::with_capacity(committed.len());
        let mut reports = Vec::with_capacity(committed.len());

        for pack in committed {
            let pack_bytes = fs::read(&pack.summary_path).expect("read committed execution pack");
            let summary: SourceUniverseExecutionPack =
                serde_json::from_slice(&pack_bytes).expect("parse committed execution pack");
            let record = summary
                .records
                .iter()
                .find(|record| {
                    pack.launch_spec
                        .start_sequence
                        .is_none_or(|start| record.sequence >= start)
                })
                .expect("RA-001a launch selects one materialized record");
            let execution_record_sha256 = execution_record_digest(&summary, record.sequence)
                .expect("derive committed record digest");
            let batch_record = SourceUniverseBatchExecutionRecord {
                sequence: record.sequence,
                operator_run_id: record.operator_run_id.clone(),
                source_binding: record.source_binding.clone(),
                category: record.category.clone(),
                symbol: record.symbol.clone(),
                archive_date: record.archive_date.clone(),
                selected_object_sha256: record.selected_object_sha256.clone(),
                run_spec_sha256: record.run_spec_sha256.clone(),
                accepted_tranche_sha256: record.accepted_tranche_sha256.clone(),
                execution_plan_sha256: record.execution_plan_sha256.clone(),
                execution_record_sha256,
                source_bindings_sha256: record.source_bindings_sha256.clone(),
                selected_object_bytes: record.selected_object_bytes,
                canonical_rows: 1,
                nt_catalog_rows: 1,
                catalog_hash: CATALOG_SHA256.to_string(),
                completion_provenance:
                    SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated,
                completion_resolution: SourceUniverseBatchExecutionCompletionResolution::Published,
                attempt_worker_sha256: EXPECTED_WORKER_SHA256.to_string(),
                terminal_publisher_worker_sha256: EXPECTED_WORKER_SHA256.to_string(),
                durable_completion: Some(DurableCompletionLocator {
                    object: DurableObjectVersionIdentity {
                        uri: format!(
                            "{}/{}",
                            record.output_prefix.trim_end_matches('/'),
                            DURABLE_COMPLETION_MANIFEST_FILE
                        ),
                        sha256: COMPLETION_SHA256.to_string(),
                        byte_len: 1,
                        version_id: format!("version-{}", record.sequence),
                        e_tag: Some(format!("etag-{}", record.sequence)),
                    },
                }),
            };
            let report = SourceUniverseBatchExecutionReport {
                schema_version: SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION.to_string(),
                batch_id: pack.launch_spec.batch_id.clone(),
                status: SourceUniverseBatchExecutionReportStatus::Completed,
                pack_id: pack.pack_id.clone(),
                universe_id: summary.universe_id,
                venue: summary.venue,
                selected_record_count: 1,
                completed_record_count: 1,
                failed_record_count: 0,
                total_canonical_rows: 1,
                total_nt_catalog_rows: 1,
                records: vec![batch_record],
                failures: Vec::new(),
            };
            let report_path = temp.path().join(format!("{}.json", pack.pack_id));
            write_canonical_report(&report_path, &report);
            inputs.push(SourceUniverseDurableTracerReportInput {
                pack_id: pack.pack_id,
                report_path,
            });
            reports.push(report);
        }
        (inputs, reports)
    }

    #[test]
    fn aggregate_limits_bound_registry_breadth_and_selected_source_bytes() {
        let committed = discover_committed_source_universe_execution_packs(&repo_root())
            .expect("discover committed execution packs");
        let selected_bytes = committed
            .iter()
            .map(|pack| {
                let bytes = fs::read(&pack.summary_path).expect("read committed execution pack");
                let summary: SourceUniverseExecutionPack =
                    serde_json::from_slice(&bytes).expect("parse committed execution pack");
                summary
                    .records
                    .iter()
                    .find(|record| {
                        pack.launch_spec
                            .start_sequence
                            .is_none_or(|start| record.sequence >= start)
                    })
                    .expect("RA-001a launch selects one materialized record")
                    .selected_object_bytes
            })
            .sum::<u64>();
        let exact_limits = SourceUniverseDurableTracerAggregateLimits {
            max_registry_packs: u64::try_from(committed.len()).expect("registry count fits u64"),
            max_total_selected_object_bytes: selected_bytes,
        };

        let envelope =
            validate_source_universe_durable_tracer_aggregate_limits(&committed, exact_limits)
                .expect("accept exact aggregate limits");
        assert_eq!(envelope.registry_packs, exact_limits.max_registry_packs);
        assert_eq!(envelope.total_selected_object_bytes, selected_bytes);

        let count_error = validate_source_universe_durable_tracer_aggregate_limits(
            &committed,
            SourceUniverseDurableTracerAggregateLimits {
                max_registry_packs: exact_limits.max_registry_packs - 1,
                ..exact_limits
            },
        )
        .expect_err("reject aggregate registry count above configured cap");
        assert!(count_error.to_string().contains("max_registry_packs"));

        let bytes_error = validate_source_universe_durable_tracer_aggregate_limits(
            &committed,
            SourceUniverseDurableTracerAggregateLimits {
                max_total_selected_object_bytes: selected_bytes - 1,
                ..exact_limits
            },
        )
        .expect_err("reject aggregate selected bytes above configured cap");
        assert!(
            bytes_error
                .to_string()
                .contains("max_total_selected_object_bytes")
        );
    }

    #[test]
    fn builds_one_canonical_receipt_for_every_registry_pack_without_a_venue_allowlist() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, _) = complete_registry_reports(&temp);

        let receipts = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect("build registry-complete durable tracer receipts");
        validate_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &receipts,
        )
        .expect("validate durable tracer receipts");

        assert_eq!(receipts.receipts.len(), inputs.len());
        assert_eq!(receipts.source_revision, EXPECTED_SOURCE_REVISION);
        assert_eq!(receipts.worker_executable_sha256, EXPECTED_WORKER_SHA256);
        assert!(
            receipts
                .receipts
                .windows(2)
                .all(|pair| pair[0].pack_id < pair[1].pack_id)
        );
        assert!(receipts.receipts.iter().all(|receipt| {
            receipt.batch_report.records[0].completion_provenance
                == SourceUniverseBatchExecutionRecordProvenance::ExecutedProcessIsolated
        }));
    }

    #[test]
    fn rejects_duplicate_missing_and_extra_report_pack_identities() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, _) = complete_registry_reports(&temp);

        let mut duplicate = inputs.clone();
        duplicate.push(inputs[0].clone());
        let duplicate_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &duplicate,
        )
        .expect_err("duplicate pack identity must fail");
        assert!(duplicate_error.to_string().contains("duplicate"));

        let missing = &inputs[1..];
        let missing_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            missing,
        )
        .expect_err("missing pack identity must fail");
        assert!(missing_error.to_string().contains("missing"));

        let mut extra = inputs.clone();
        extra.push(SourceUniverseDurableTracerReportInput {
            pack_id: "unregistered-pack".to_string(),
            report_path: inputs[0].report_path.clone(),
        });
        let extra_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &extra,
        )
        .expect_err("extra pack identity must fail");
        assert!(extra_error.to_string().contains("extra"));
    }

    #[test]
    fn rejects_attempt_worker_that_is_not_the_exact_current_worker() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, mut reports) = complete_registry_reports(&temp);
        reports[0].records[0].completion_resolution =
            SourceUniverseBatchExecutionCompletionResolution::Discovered;
        reports[0].records[0].attempt_worker_sha256 = "d".repeat(64);
        write_canonical_report(&inputs[0].report_path, &reports[0]);
        let digest_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect_err("worker digest disagreement must fail");
        assert!(
            digest_error
                .to_string()
                .contains("attempt worker executable SHA-256")
        );
    }

    #[test]
    fn accepts_discovered_terminal_from_an_older_valid_publisher_and_rejects_tampering() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, mut reports) = complete_registry_reports(&temp);
        reports[0].records[0].completion_resolution =
            SourceUniverseBatchExecutionCompletionResolution::Discovered;
        reports[0].records[0].terminal_publisher_worker_sha256 = "d".repeat(64);
        write_canonical_report(&inputs[0].report_path, &reports[0]);

        build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect("tracer validates but does not equate an immutable terminal publisher");

        reports[0].records[0].terminal_publisher_worker_sha256 = "not-a-sha256".to_string();
        write_canonical_report(&inputs[0].report_path, &reports[0]);
        let error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect_err("tampered terminal publisher must fail closed");
        assert!(format!("{error:#}").contains("terminal_publisher_worker_sha256"));
    }

    #[test]
    fn rejects_zero_rows_and_non_exact_durable_locator_identity() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, mut reports) = complete_registry_reports(&temp);
        reports[0].records[0].canonical_rows = 0;
        reports[0].total_canonical_rows = 0;
        write_canonical_report(&inputs[0].report_path, &reports[0]);
        let rows_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect_err("zero canonical rows must fail");
        assert!(
            rows_error
                .to_string()
                .contains("positive canonical and NT rows")
        );

        reports[0].records[0].canonical_rows = 1;
        reports[0].total_canonical_rows = 1;
        reports[0].records[0]
            .durable_completion
            .as_mut()
            .expect("durable completion")
            .object
            .version_id = "null".to_string();
        write_canonical_report(&inputs[0].report_path, &reports[0]);
        let locator_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect_err("null version cannot identify immutable durable output");
        assert!(format!("{locator_error:#}").contains("S3 null version"));

        let durable_object = &mut reports[0].records[0]
            .durable_completion
            .as_mut()
            .expect("durable completion")
            .object;
        durable_object.version_id = "restored-version".to_string();
        durable_object.sha256 = "not-a-sha256".to_string();
        write_canonical_report(&inputs[0].report_path, &reports[0]);
        let hash_error = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect_err("malformed durable object hash must fail");
        assert!(format!("{hash_error:#}").contains("SHA-256"));
    }

    #[test]
    fn validation_rejects_tampered_pack_launch_and_report_pins() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, _) = complete_registry_reports(&temp);
        let receipts = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect("build receipt set");

        let mutations: [fn(&mut super::SourceUniverseDurableTracerReceipt); 3] = [
            |receipt: &mut super::SourceUniverseDurableTracerReceipt| {
                receipt.execution_pack.sha256 = "d".repeat(64);
            },
            |receipt: &mut super::SourceUniverseDurableTracerReceipt| {
                receipt.launch.bytes += 1;
            },
            |receipt: &mut super::SourceUniverseDurableTracerReceipt| {
                receipt.batch_report_artifact.sha256 = "e".repeat(64);
            },
        ];
        for mutate in mutations {
            let mut tampered = receipts.clone();
            mutate(&mut tampered.receipts[0]);
            assert!(
                validate_source_universe_durable_tracer_receipt_set(
                    &repo_root(),
                    EXPECTED_SOURCE_REVISION,
                    EXPECTED_WORKER_SHA256,
                    &tampered,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn canonical_round_trip_rejects_noncanonical_or_duplicate_receipts() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, _) = complete_registry_reports(&temp);
        let receipts = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect("build receipt set");
        let canonical = crate::reference_artifact::canonical_json_bytes(&receipts)
            .expect("serialize canonical receipt set");
        assert_eq!(
            parse_and_validate_source_universe_durable_tracer_receipt_set(
                &repo_root(),
                EXPECTED_SOURCE_REVISION,
                EXPECTED_WORKER_SHA256,
                &canonical,
            )
            .expect("parse canonical receipt set"),
            receipts
        );

        let compact = serde_json::to_vec(&receipts).expect("serialize compact receipt set");
        let canonical_error = parse_and_validate_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &compact,
        )
        .expect_err("noncanonical receipt bytes must fail");
        assert!(canonical_error.to_string().contains("not canonical"));

        let mut duplicate = receipts.clone();
        duplicate.receipts.push(receipts.receipts[0].clone());
        let duplicate_error = validate_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &duplicate,
        )
        .expect_err("duplicate receipt identity must fail");
        assert!(duplicate_error.to_string().contains("strict pack_id order"));

        let revision_error = validate_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            "2222222222222222222222222222222222222222",
            EXPECTED_WORKER_SHA256,
            &receipts,
        )
        .expect_err("receipt source revision mismatch must fail");
        assert!(
            revision_error
                .to_string()
                .contains("source revision mismatch")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn create_only_writer_is_idempotent_only_for_the_same_canonical_receipt_bytes() {
        let temp = TempDir::new().expect("temporary report directory");
        let (inputs, _) = complete_registry_reports(&temp);
        let receipts = build_source_universe_durable_tracer_receipt_set(
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &inputs,
        )
        .expect("build receipt set");
        let output = temp.path().join("ra001a-receipt-set.json");
        let guard = OperatorWorkBudgetGuard::unbounded();

        let first = write_source_universe_durable_tracer_receipt_set(
            &output,
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &receipts,
            &guard,
        )
        .expect("create receipt set");
        let second = write_source_universe_durable_tracer_receipt_set(
            &output,
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &receipts,
            &guard,
        )
        .expect("verify identical receipt set");
        assert_eq!(first, second);
        assert_eq!(
            first.sha256,
            sha256_hex(&fs::read(&output).expect("read receipt set"))
        );
        assert_eq!(
            read_and_validate_source_universe_durable_tracer_receipt_set(
                &repo_root(),
                EXPECTED_SOURCE_REVISION,
                EXPECTED_WORKER_SHA256,
                &first,
            )
            .expect("reopen exact pinned receipt set"),
            receipts
        );

        let mut changed = receipts;
        changed.receipts[0].batch_report.records[0].catalog_hash = "d".repeat(64);
        let changed_report =
            crate::reference_artifact::canonical_json_bytes(&changed.receipts[0].batch_report)
                .expect("serialize changed report");
        changed.receipts[0].batch_report_artifact = super::SourceUniverseDurableTracerArtifactPin {
            bytes: u64::try_from(changed_report.len()).expect("report length fits u64"),
            sha256: sha256_hex(&changed_report),
        };
        let conflict = write_source_universe_durable_tracer_receipt_set(
            &output,
            &repo_root(),
            EXPECTED_SOURCE_REVISION,
            EXPECTED_WORKER_SHA256,
            &changed,
            &guard,
        )
        .expect_err("different receipt bytes must not replace immutable output");
        assert!(format!("{conflict:#}").contains("different bytes"));
    }
}
