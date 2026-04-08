use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail, ensure};
use arrow::{ipc::reader::StreamReader, record_batch::RecordBatch};
use nautilus_persistence::backend::{catalog::ParquetDataCatalog, custom::decode_batch_to_data};

const SUPPORTED_STREAM_CLASSES: &[&str] = &[
    "quotes",
    "trades",
    "order_book_deltas",
    "order_book_depths",
    "index_prices",
    "mark_prices",
    "instrument_closes",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamToLakeReport {
    pub instance_id: String,
    pub output_root: PathBuf,
    pub converted_classes: Vec<&'static str>,
}

pub fn supported_stream_classes() -> &'static [&'static str] {
    SUPPORTED_STREAM_CLASSES
}

pub fn convert_live_spool_to_parquet(
    catalog_path: &Path,
    instance_id: &str,
    output_root: &Path,
) -> Result<StreamToLakeReport> {
    ensure_local_path(catalog_path, "catalog_path")?;
    ensure_local_path(output_root, "output_root")?;
    ensure_disjoint_paths(catalog_path, output_root)?;
    ensure_empty_output_root(output_root)?;

    let source_instance_dir = catalog_path.join("live").join(instance_id);
    ensure!(
        source_instance_dir.is_dir(),
        "missing live spool instance directory: {}",
        source_instance_dir.display()
    );

    stage_live_instance(&source_instance_dir, &output_root.join("live").join(instance_id))?;

    let mut catalog = ParquetDataCatalog::new(output_root, None, None, None, None);
    for data_cls in SUPPORTED_STREAM_CLASSES {
        convert_staged_class_to_parquet(&mut catalog, output_root, instance_id, data_cls)?;
    }

    Ok(StreamToLakeReport {
        instance_id: instance_id.to_string(),
        output_root: output_root.to_path_buf(),
        converted_classes: SUPPORTED_STREAM_CLASSES.to_vec(),
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

fn stage_live_instance(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if path.is_dir() {
            if SUPPORTED_STREAM_CLASSES.contains(&file_name.as_ref()) {
                copy_tree_if_exists(&path, &destination.join(file_name.as_ref()))?;
            }
            continue;
        }

        // Older Task 3 spool output can be flat (`quotes_<ts>.feather` at the instance root).
        // Normalize those files into the per-class tree the batch converter expects.
        for data_cls in SUPPORTED_STREAM_CLASSES {
            if file_name.starts_with(&format!("{data_cls}_")) {
                copy_tree_if_exists(&path, &destination.join(data_cls).join(file_name.as_ref()))?;
                break;
            }
        }
    }

    for data_cls in SUPPORTED_STREAM_CLASSES {
        fs::create_dir_all(destination.join(data_cls))?;
    }

    Ok(())
}

fn convert_staged_class_to_parquet(
    catalog: &mut ParquetDataCatalog,
    output_root: &Path,
    instance_id: &str,
    data_cls: &'static str,
) -> Result<()> {
    let class_root = output_root.join("live").join(instance_id).join(data_cls);
    let files = collect_feather_files(&class_root)?;

    for file in files {
        let (mut metadata, batches) = read_feather_batches(&file)?;
        metadata
            .entry("type_name".to_string())
            .or_insert_with(|| type_name_for_data_class(data_cls).to_string());

        let mut data = Vec::new();
        for batch in batches {
            data.extend(decode_batch_to_data(&metadata, batch, false)?);
        }

        if !data.is_empty() {
            catalog.write_data_enum(&data, None, None, None)?;
        }
    }

    Ok(())
}

fn collect_feather_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }

    let metadata = fs::symlink_metadata(root)?;
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

fn read_feather_batches(path: &Path) -> Result<(HashMap<String, String>, Vec<RecordBatch>)> {
    let bytes = fs::read(path)?;
    let reader = StreamReader::try_new(Cursor::new(bytes), None)?;
    // Arrow stream metadata lives on the reader schema, not each RecordBatch schema.
    let metadata = reader.schema().metadata().clone();
    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch?);
    }

    Ok((metadata, batches))
}

fn copy_tree_if_exists(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if !destination.exists() {
            fs::copy(source, destination)?;
        }
        return Ok(());
    }

    fs::create_dir_all(destination)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        copy_tree_if_exists(&entry.path(), &destination.join(entry.file_name()))?;
    }

    Ok(())
}

fn type_name_for_data_class(data_cls: &str) -> &'static str {
    match data_cls {
        "quotes" => "QuoteTick",
        "trades" => "TradeTick",
        "order_book_deltas" => "OrderBookDelta",
        "order_book_depths" => "OrderBookDepth10",
        "index_prices" => "IndexPriceUpdate",
        "mark_prices" => "MarkPriceUpdate",
        "instrument_closes" => "InstrumentClose",
        other => panic!("unsupported reduced Task 4 data class: {other}"),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        Ok(fs::canonicalize(path)?)
    }
    else {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };

        let mut tail = Vec::<OsString>::new();
        let mut cursor = absolute.as_path();
        while !cursor.exists() {
            let name = cursor
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("unable to normalize path {}", absolute.display()))?;
            tail.push(name.to_os_string());
            cursor = cursor.parent().ok_or_else(|| {
                anyhow::anyhow!("unable to find existing ancestor for {}", absolute.display())
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
