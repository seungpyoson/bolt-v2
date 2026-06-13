use std::path::PathBuf;

use ahash::AHashMap;
use anyhow::{Context, Result};
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
