use std::{collections::BTreeSet, path::PathBuf};

use ahash::AHashMap;
use anyhow::{Context, Result, bail, ensure};
use arrow::record_batch::RecordBatch;
use nautilus_core::UnixNanos;
use nautilus_model::data::Data;
use nautilus_persistence::backend::{
    catalog::{CatalogPathPrefix, ParquetDataCatalog},
    session::DataBackendSession,
};
use nautilus_serialization::arrow::DecodeDataFromRecordBatch;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogQuerySpec {
    pub catalog_uri: String,
    pub storage_options: Option<AHashMap<String, String>>,
    pub instrument_ids: Option<Vec<String>>,
    pub start: Option<UnixNanos>,
    pub end: Option<UnixNanos>,
    pub where_clause: Option<String>,
    pub files: Option<Vec<String>>,
    pub optimize_file_loading: bool,
}

pub fn query_catalog_typed<T>(spec: &CatalogQuerySpec) -> Result<Vec<T>>
where
    T: DecodeDataFromRecordBatch + CatalogPathPrefix + TryFrom<Data>,
{
    let mut catalog = ParquetDataCatalog::from_uri(
        &spec.catalog_uri,
        spec.storage_options.clone(),
        None,
        None,
        None,
    )
    .context("create NautilusTrader catalog from URI")?;
    catalog
        .query_typed_data::<T>(
            spec.instrument_ids.clone(),
            spec.start,
            spec.end,
            spec.where_clause.as_deref(),
            spec.files.clone(),
            spec.optimize_file_loading,
        )
        .context("query typed data through NautilusTrader catalog")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlBatchQuerySpec {
    pub table_name: String,
    pub file_path: PathBuf,
    pub sql: Option<String>,
    pub chunk_size: usize,
}

pub fn query_sql_arrow_batches(spec: &SqlBatchQuerySpec) -> Result<Vec<RecordBatch>> {
    let file_path = spec.file_path.to_str().with_context(|| {
        format!(
            "catalog SQL file path is not UTF-8: {}",
            spec.file_path.display()
        )
    })?;
    let mut session = DataBackendSession::new(spec.chunk_size);
    session
        .collect_query_batches(&spec.table_name, file_path, spec.sql.as_deref())
        .context("query Arrow batches through NautilusTrader DataBackendSession")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotebookQueryEngine {
    pub engine_key: String,
    pub reads_nt_catalog_arrow: bool,
    pub read_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotebookErgonomics {
    pub read_only: bool,
    pub exposes_arrow_batches: bool,
    pub exposes_sql_examples: bool,
    pub mutation_actions_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CustomUiDecision {
    NotSelected,
    AllowedAfterProductGate {
        confirmed_requirement_refs: Vec<String>,
        rejected_product_refs: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotebookBiSurfaceSpec {
    pub artifact_root: String,
    pub nt_catalog_arrow_uri: String,
    pub query_engines: Vec<NotebookQueryEngine>,
    pub dashboard_product_refs: Vec<String>,
    pub notebook: NotebookErgonomics,
    pub custom_ui: CustomUiDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotebookBiSurface {
    pub artifact_root: String,
    pub nt_catalog_arrow_uri: String,
    pub query_engines: Vec<NotebookQueryEngine>,
    pub dashboard_product_refs: Vec<String>,
    pub notebook: NotebookErgonomics,
    pub custom_ui: CustomUiDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsSourceBinding {
    pub source_binding_key: String,
    pub venue_key: String,
    pub provider_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureJoinSpec {
    pub left_source_binding_key: String,
    pub right_source_binding_key: String,
    pub as_of_column: String,
    pub freshness_column: String,
}

pub fn build_notebook_bi_surface(spec: NotebookBiSurfaceSpec) -> Result<NotebookBiSurface> {
    let artifact_root = normalize_artifact_root(&spec.artifact_root)?;
    ensure_uri_under_artifact_root(
        "nt_catalog_arrow_uri",
        &artifact_root,
        &spec.nt_catalog_arrow_uri,
    )?;
    ensure!(
        spec.nt_catalog_arrow_uri.ends_with(".parquet"),
        "nt_catalog_arrow_uri must reference NT catalog Arrow/Parquet output"
    );
    ensure!(
        !spec.query_engines.is_empty(),
        "query_engines must include at least one off-the-shelf BI/query engine"
    );
    let mut engine_keys = BTreeSet::new();
    for engine in &spec.query_engines {
        ensure_non_empty("query_engines.engine_key", &engine.engine_key)?;
        ensure!(
            engine_keys.insert(engine.engine_key.clone()),
            "query_engines must not contain duplicate engine_key {:?}",
            engine.engine_key
        );
        ensure!(
            engine.reads_nt_catalog_arrow,
            "query engine {:?} must read NT catalog Arrow output",
            engine.engine_key
        );
        ensure!(
            engine.read_only,
            "query engine {:?} must be read-only",
            engine.engine_key
        );
    }
    ensure_non_empty_list("dashboard_product_refs", &spec.dashboard_product_refs)?;
    for product_ref in &spec.dashboard_product_refs {
        ensure_non_empty("dashboard_product_refs", product_ref)?;
    }
    validate_notebook_ergonomics(&spec.notebook)?;
    validate_custom_ui_decision(&spec.custom_ui)?;

    Ok(NotebookBiSurface {
        artifact_root,
        nt_catalog_arrow_uri: spec.nt_catalog_arrow_uri,
        query_engines: spec.query_engines,
        dashboard_product_refs: spec.dashboard_product_refs,
        notebook: spec.notebook,
        custom_ui: spec.custom_ui,
    })
}

pub fn validate_feature_join_bindings(
    bindings: &[AnalyticsSourceBinding],
    joins: &[FeatureJoinSpec],
) -> Result<()> {
    ensure_non_empty_list("analytics source bindings", bindings)?;
    ensure_non_empty_list("feature joins", joins)?;

    let mut source_binding_keys = BTreeSet::new();
    let mut venue_keys = BTreeSet::new();
    let mut provider_keys = BTreeSet::new();
    for binding in bindings {
        ensure_non_empty("source_binding_key", &binding.source_binding_key)?;
        ensure_non_empty("venue_key", &binding.venue_key)?;
        ensure_non_empty("provider_key", &binding.provider_key)?;
        ensure!(
            source_binding_keys.insert(binding.source_binding_key.clone()),
            "duplicate source_binding_key {:?}",
            binding.source_binding_key
        );
        venue_keys.insert(binding.venue_key.clone());
        provider_keys.insert(binding.provider_key.clone());
    }

    for join in joins {
        ensure_non_empty("left_source_binding_key", &join.left_source_binding_key)?;
        ensure_non_empty("right_source_binding_key", &join.right_source_binding_key)?;
        ensure_non_empty("as_of_column", &join.as_of_column)?;
        ensure_non_empty("freshness_column", &join.freshness_column)?;
        ensure_resolves_by_source_binding_key(
            "left_source_binding_key",
            &join.left_source_binding_key,
            &source_binding_keys,
            &venue_keys,
            &provider_keys,
        )?;
        ensure_resolves_by_source_binding_key(
            "right_source_binding_key",
            &join.right_source_binding_key,
            &source_binding_keys,
            &venue_keys,
            &provider_keys,
        )?;
    }

    Ok(())
}

fn validate_notebook_ergonomics(notebook: &NotebookErgonomics) -> Result<()> {
    ensure!(notebook.read_only, "notebook surface must be read-only");
    ensure!(
        notebook.exposes_arrow_batches,
        "notebook surface must expose Arrow-batch reads"
    );
    ensure!(
        notebook.exposes_sql_examples,
        "notebook surface must expose SQL example queries"
    );
    ensure!(
        !notebook.mutation_actions_enabled,
        "notebook surface must not enable mutation actions"
    );
    Ok(())
}

fn validate_custom_ui_decision(custom_ui: &CustomUiDecision) -> Result<()> {
    match custom_ui {
        CustomUiDecision::NotSelected => Ok(()),
        CustomUiDecision::AllowedAfterProductGate {
            confirmed_requirement_refs,
            rejected_product_refs,
        } => {
            ensure_non_empty_list("confirmed requirement refs", confirmed_requirement_refs)?;
            ensure_non_empty_list("rejected product refs", rejected_product_refs)?;
            for requirement_ref in confirmed_requirement_refs {
                ensure_non_empty("confirmed requirement refs", requirement_ref)?;
            }
            for product_ref in rejected_product_refs {
                ensure_non_empty("rejected product refs", product_ref)?;
            }
            Ok(())
        }
    }
}

fn ensure_resolves_by_source_binding_key(
    field: &'static str,
    value: &str,
    source_binding_keys: &BTreeSet<String>,
    venue_keys: &BTreeSet<String>,
    provider_keys: &BTreeSet<String>,
) -> Result<()> {
    if source_binding_keys.contains(value) {
        return Ok(());
    }
    if venue_keys.contains(value) || provider_keys.contains(value) {
        bail!("{field} must use source_binding_key, not venue/provider identity");
    }
    bail!("{field} {value:?} must reference a configured source_binding_key")
}

fn ensure_uri_under_artifact_root(
    field: &'static str,
    artifact_root: &str,
    uri: &str,
) -> Result<()> {
    ensure_non_empty(field, uri)?;
    ensure!(
        uri.starts_with(&format!("{artifact_root}/")),
        "{field} must live under artifact_root {artifact_root:?}"
    );
    Ok(())
}

fn normalize_artifact_root(artifact_root: &str) -> Result<String> {
    let normalized = artifact_root.trim_end_matches('/').to_string();
    ensure_non_empty("artifact_root", &normalized)?;
    ensure!(
        normalized.starts_with("s3://"),
        "artifact_root must be an s3:// URI"
    );
    Ok(normalized)
}

fn ensure_non_empty(field: &'static str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    Ok(())
}

fn ensure_non_empty_list<T>(field: &'static str, values: &[T]) -> Result<()> {
    ensure!(!values.is_empty(), "{field} must not be empty");
    Ok(())
}
