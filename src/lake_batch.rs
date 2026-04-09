use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    fs::File,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail, ensure};
use arrow::ipc::reader::StreamReader;
use nautilus_persistence::backend::{catalog::ParquetDataCatalog, custom::decode_batch_to_data};

use crate::venue_contract::{Capability, ClassReport, CompletenessReport, Policy, VenueContract};

const SUPPORTED_STREAM_CLASSES: &[&str] = &[
    "quotes",
    "trades",
    "order_book_deltas",
    "order_book_depths",
    "index_prices",
    "mark_prices",
    "instrument_closes",
];

#[derive(Debug, Clone, PartialEq)]
pub struct StreamToLakeReport {
    pub instance_id: String,
    pub output_root: PathBuf,
    pub converted_classes: Vec<&'static str>,
    pub completeness: Option<CompletenessReport>,
}

pub fn supported_stream_classes() -> &'static [&'static str] {
    SUPPORTED_STREAM_CLASSES
}

pub fn convert_live_spool_to_parquet(
    catalog_path: &Path,
    instance_id: &str,
    output_root: &Path,
    contract: Option<&VenueContract>,
) -> Result<StreamToLakeReport> {
    ensure_local_path(catalog_path, "catalog_path")?;
    ensure_local_path(output_root, "output_root")?;
    ensure_instance_id_segment(instance_id)?;
    ensure_disjoint_paths(catalog_path, output_root)?;
    ensure_empty_output_root(output_root)?;

    let source_instance_dir = catalog_path.join("live").join(instance_id);
    ensure!(
        source_instance_dir.is_dir(),
        "missing live spool instance directory: {}",
        source_instance_dir.display()
    );

    let class_files = discover_source_files(&source_instance_dir)?;

    fs::create_dir_all(output_root)?;
    let catalog = ParquetDataCatalog::new(output_root, None, None, None, None);
    let mut converted_classes = Vec::new();
    for data_cls in SUPPORTED_STREAM_CLASSES {
        if let Some(files) = class_files.get(data_cls)
            && convert_class_to_parquet(&catalog, files, data_cls)?
        {
            converted_classes.push(*data_cls);
        }
    }

    let completeness = if let Some(contract) = contract {
        let report = build_completeness_report(
            contract,
            instance_id,
            &source_instance_dir,
            &class_files,
            &converted_classes,
        )?;
        let json = serde_json::to_string_pretty(&report)?;
        fs::write(output_root.join("completeness_report.json"), &json)?;
        if report.outcome == "fail" {
            bail!("contract validation failed:\n{json}");
        }
        Some(report)
    } else {
        ensure!(
            !converted_classes.is_empty(),
            "no supported reduced task 4 data found"
        );
        None
    };

    Ok(StreamToLakeReport {
        instance_id: instance_id.to_string(),
        output_root: output_root.to_path_buf(),
        converted_classes,
        completeness,
    })
}

/// Build a completeness report without bailing — the caller decides
/// whether a "fail" outcome is fatal.
fn build_completeness_report(
    contract: &VenueContract,
    instance_id: &str,
    source_instance_dir: &Path,
    class_files: &BTreeMap<&'static str, Vec<PathBuf>>,
    converted_classes: &[&'static str],
) -> Result<CompletenessReport> {
    let mut classes = BTreeMap::new();
    let mut has_failure = false;

    // ── Unknown-class discovery ──
    const SPOOL_INFRASTRUCTURE_DIRS: &[&str] = &["instruments", "status"];

    let known_classes: std::collections::HashSet<&str> = contract
        .streams
        .keys()
        .map(String::as_str)
        .chain(supported_stream_classes().iter().copied())
        .chain(SPOOL_INFRASTRUCTURE_DIRS.iter().copied())
        .collect();

    if source_instance_dir.is_dir() {
        for entry in std::fs::read_dir(source_instance_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let name = dir_name.to_string_lossy();
            if !known_classes.contains(name.as_ref()) {
                has_failure = true;
                classes.insert(
                    name.to_string(),
                    ClassReport {
                        capability: "unknown".to_string(),
                        policy: None,
                        spool_present: true,
                        rows_converted: None,
                        files_converted: None,
                        status: "fail_unknown".to_string(),
                        reason: Some("directory not in contract or known classes".into()),
                    },
                );
            }
        }
    }

    // ── Per-stream classification ──
    for (name, stream) in &contract.streams {
        let spool_present = class_files
            .get(name.as_str())
            .is_some_and(|f| !f.is_empty());
        let was_converted = converted_classes.contains(&name.as_str());
        let effective_policy = contract.effective_policy(name).unwrap_or(Policy::Disabled);

        let (status, reason) = match (&stream.capability, spool_present, was_converted) {
            (Capability::Unsupported, false, _) => ("pass_unsupported", None),
            (Capability::Unsupported, true, _) => {
                has_failure = true;
                ("fail_contract_violation", None)
            }
            (Capability::Supported | Capability::Conditional, true, true) => ("pass", None),
            (Capability::Supported | Capability::Conditional, true, false) => {
                match effective_policy {
                    Policy::Required => {
                        has_failure = true;
                        (
                            "spool_present_conversion_empty",
                            Some("required class has spool data but zero converted rows".into()),
                        )
                    }
                    _ => ("spool_present_conversion_empty", None),
                }
            }
            (Capability::Supported | Capability::Conditional, false, _) => match effective_policy {
                Policy::Required => {
                    has_failure = true;
                    ("fail_required_absent", Some("no_spool_data".to_string()))
                }
                Policy::Optional => ("warn_optional_absent", None),
                Policy::Disabled => ("pass_disabled", None),
            },
        };

        let cap_str = match stream.capability {
            Capability::Supported => "supported",
            Capability::Unsupported => "unsupported",
            Capability::Conditional => "conditional",
        };
        let policy_str = match effective_policy {
            Policy::Required => Some("required".to_string()),
            Policy::Optional => Some("optional".to_string()),
            Policy::Disabled => None,
        };

        classes.insert(
            name.clone(),
            ClassReport {
                capability: cap_str.to_string(),
                policy: policy_str,
                spool_present,
                rows_converted: None,
                files_converted: None,
                status: status.to_string(),
                reason,
            },
        );
    }

    Ok(CompletenessReport {
        schema_version: 1,
        venue: contract.venue.clone(),
        contract_version: contract.schema_version,
        instance_id: instance_id.to_string(),
        outcome: if has_failure { "fail" } else { "pass" }.to_string(),
        classes,
    })
}

fn ensure_local_path(path: &Path, label: &str) -> Result<()> {
    if path.to_string_lossy().contains("://") {
        bail!(
            "Task 4 reduced currently supports only local {label}, got `{}`",
            path.display()
        );
    }

    Ok(())
}

fn ensure_instance_id_segment(instance_id: &str) -> Result<()> {
    let mut components = Path::new(instance_id).components();
    let component = components.next();
    ensure!(
        matches!(component, Some(std::path::Component::Normal(_))) && components.next().is_none(),
        "instance_id must be a single path segment"
    );

    Ok(())
}

fn ensure_disjoint_paths(catalog_path: &Path, output_root: &Path) -> Result<()> {
    let catalog_path = absolute_path(catalog_path)?;
    let output_root = absolute_path(output_root)?;

    ensure!(
        !paths_overlap(&catalog_path, &output_root),
        "output_root must not overlap catalog_path"
    );

    Ok(())
}

fn ensure_empty_output_root(output_root: &Path) -> Result<()> {
    if !output_root.exists() {
        return Ok(());
    }

    ensure!(
        fs::read_dir(output_root)?.next().is_none(),
        "output_root must be empty before conversion"
    );

    Ok(())
}

/// Scan a live spool instance directory and build a logical map of
/// class name → source feather file paths.  Handles both per-class
/// subdirectories and legacy Task 3 flat spool layouts.  Symlinks
/// are silently skipped.
fn discover_source_files(
    source_instance_dir: &Path,
) -> Result<BTreeMap<&'static str, Vec<PathBuf>>> {
    let mut class_files: BTreeMap<&'static str, Vec<PathBuf>> = BTreeMap::new();

    for entry in fs::read_dir(source_instance_dir)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.is_symlink() {
            continue;
        }

        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        if meta.is_dir() {
            for data_cls in SUPPORTED_STREAM_CLASSES {
                if *data_cls == file_name_str.as_ref() {
                    let files = collect_feather_files(&entry.path())?;
                    class_files.entry(data_cls).or_default().extend(files);
                    break;
                }
            }
        } else if meta.is_file() {
            // Legacy Task 3 flat spool: `quotes_<ts>.feather` at instance root.
            if entry.path().extension().and_then(|e| e.to_str()) != Some("feather") {
                continue;
            }
            for data_cls in SUPPORTED_STREAM_CLASSES {
                if file_name_str.starts_with(&format!("{data_cls}_")) {
                    class_files.entry(data_cls).or_default().push(entry.path());
                    break;
                }
            }
        }
    }

    for files in class_files.values_mut() {
        files.sort();
    }

    Ok(class_files)
}

fn convert_class_to_parquet(
    catalog: &ParquetDataCatalog,
    files: &[PathBuf],
    data_cls: &'static str,
) -> Result<bool> {
    let type_name = type_name_for_data_class(data_cls)?;
    let mut converted_any = false;

    for file in files {
        let mut reader = open_feather_reader(file)?;
        let mut metadata = reader.schema().metadata().clone();
        metadata
            .entry("type_name".to_string())
            .or_insert_with(|| type_name.to_string());

        let mut file_data = Vec::new();
        for batch in &mut reader {
            let batch = batch?;
            file_data.extend(decode_batch_to_data(&metadata, batch, false)?);
        }
        if !file_data.is_empty() {
            catalog.write_data_enum(&file_data, None, None, Some(true))?;
            converted_any = true;
        }
    }

    Ok(converted_any)
}

fn collect_feather_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }

    let metadata = fs::symlink_metadata(root)?;
    if metadata.is_symlink() {
        return Ok(files);
    }
    if metadata.is_file() {
        if root.extension().and_then(|ext| ext.to_str()) == Some("feather") {
            files.push(root.to_path_buf());
        }
        return Ok(files);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        files.extend(collect_feather_files(&entry.path())?);
    }
    files.sort();

    Ok(files)
}

fn open_feather_reader(path: &Path) -> Result<StreamReader<File>> {
    Ok(StreamReader::try_new(File::open(path)?, None)?)
}

fn type_name_for_data_class(data_cls: &str) -> Result<&'static str> {
    match data_cls {
        "quotes" => Ok("QuoteTick"),
        "trades" => Ok("TradeTick"),
        "order_book_deltas" => Ok("OrderBookDelta"),
        "order_book_depths" => Ok("OrderBookDepth10"),
        "index_prices" => Ok("IndexPriceUpdate"),
        "mark_prices" => Ok("MarkPriceUpdate"),
        "instrument_closes" => Ok("InstrumentClose"),
        other => bail!("unsupported reduced task 4 data class: {other}"),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        Ok(fs::canonicalize(path)?)
    } else {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };

        let mut tail = Vec::<OsString>::new();
        let mut cursor = absolute.as_path();
        while !cursor.exists() {
            let name = cursor.file_name().ok_or_else(|| {
                anyhow::anyhow!("unable to normalize path {}", absolute.display())
            })?;
            tail.push(name.to_os_string());
            cursor = cursor.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "unable to find existing ancestor for {}",
                    absolute.display()
                )
            })?;
        }

        let mut resolved = fs::canonicalize(cursor)?;
        for component in tail.iter().rev() {
            resolved.push(component);
        }
        Ok(resolved)
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}
