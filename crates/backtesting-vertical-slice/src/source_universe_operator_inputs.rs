//! Source-universe operator input materialization.
//!
//! This pre-execution artifact bridges accepted source-universe object gates to
//! the existing single-object operator contract. It proves every planned object
//! has converter mapping and NT instrument metadata before payload bytes are
//! fetched.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hashing::sha256_hex;
use crate::path_resolution::{
    portable_artifact_path_for_spec, resolve_existing_path, resolve_output_dir,
};
use crate::reference_artifact::ReferenceArtifactPin;
use crate::{
    canonical_trades::{CsvTradeMappingConfig, RawPayloadContainer},
    catalog_projection::{
        CatalogInstrumentSpec, CatalogInstrumentSpecSource, CryptoFutureInstrumentKind,
        CryptoFutureInstrumentSpec, CryptoPerpetualInstrumentKind, CryptoPerpetualInstrumentSpec,
        SpotInstrumentSpec,
    },
    source_universe_conversion_run_plan::{
        SourceUniverseConversionRunPlan, SourceUniverseConversionRunPlanStatus,
    },
    source_universe_object_gates::{
        SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateStatus,
    },
};

pub const SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION: &str =
    "source-universe-operator-inputs.v1";
pub const SOURCE_UNIVERSE_OPERATOR_INPUTS_FILE: &str = "source-universe-operator-inputs.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseOperatorInputsSpec {
    pub input_id: String,
    pub source_universe_object_gates_path: PathBuf,
    pub source_universe_conversion_run_plan_path: PathBuf,
    pub source_universe_conversion_plan_path: PathBuf,
    pub instrument_metadata_snapshot_path: PathBuf,
    pub output_dir: PathBuf,
    pub operator_run_id_prefix: String,
    pub nt_venue: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub raw_payload_container: RawPayloadContainer,
    pub max_decoded_bytes: u64,
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
    pub default_spot_max_notional: String,
    pub default_derivative_max_notional: String,
    pub default_derivative_multiplier: String,
    pub default_maker_fee: String,
    pub default_taker_fee: String,
    #[serde(default)]
    pub overwrite_existing_artifacts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseOperatorInputsStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseOperatorInputRecordStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseOperatorInstrumentSpecRecord {
    pub instrument_key: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub nt_instrument_id: String,
    pub metadata_source_uri: String,
    pub instrument_spec: CatalogInstrumentSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseOperatorConverterMapping {
    pub source_binding: String,
    pub category: String,
    pub schema_columns: Vec<String>,
    pub converter_csv: CsvTradeMappingConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseOperatorInputRecord {
    pub work_item_id: String,
    pub status: SourceUniverseOperatorInputRecordStatus,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub source_uri: String,
    pub source_url: String,
    pub selected_object_sha256: String,
    pub selected_object_bytes: u64,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub accepted_tranche_id: String,
    pub output_prefix: String,
    pub instrument_key: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub raw_payload_container: RawPayloadContainer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_member: Option<String>,
    pub max_decoded_bytes: u64,
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
    pub schema_columns: Option<Vec<String>>,
    pub converter_csv: Option<CsvTradeMappingConfig>,
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseOperatorInputs {
    pub schema_version: String,
    pub input_id: String,
    pub status: SourceUniverseOperatorInputsStatus,
    pub gate_id: String,
    pub conversion_run_plan_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub operator_run_id_prefix: String,
    pub nt_venue: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub raw_payload_container: RawPayloadContainer,
    pub max_decoded_bytes: u64,
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
    pub planned_object_count: u64,
    pub planned_source_bytes: u64,
    pub conversion_run_count: u64,
    pub instrument_spec_count: u64,
    pub converter_mapping_count: u64,
    pub ready_input_count: u64,
    pub blocked_input_count: u64,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub converter_mappings: Vec<SourceUniverseOperatorConverterMapping>,
    pub instrument_specs: Vec<SourceUniverseOperatorInstrumentSpecRecord>,
    pub records: Vec<SourceUniverseOperatorInputRecord>,
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseOperatorInputsArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub record_count: u64,
}

pub fn write_source_universe_operator_inputs_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseOperatorInputsArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe operator-inputs spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseOperatorInputsSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source-universe operator-inputs spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_operator_inputs(&spec, base_dir)
}

pub fn write_source_universe_operator_inputs(
    spec: &SourceUniverseOperatorInputsSpec,
    base_dir: &Path,
) -> Result<SourceUniverseOperatorInputsArtifact> {
    let inputs = evaluate_source_universe_operator_inputs(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe operator-inputs directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_UNIVERSE_OPERATOR_INPUTS_FILE);
    let rewrite = if spec.overwrite_existing_artifacts {
        crate::reference_artifact::ReferenceArtifactRewrite::Overwrite
    } else {
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
    };
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_UNIVERSE_OPERATOR_INPUTS_FILE,
        &inputs,
        rewrite,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: |error| {
                anyhow::anyhow!("serialize source-universe operator-inputs: {error}")
            },
            read_existing_error: |path, error| {
                anyhow::anyhow!("read existing source-universe operator-inputs {path}: {error}")
            },
            mismatch_error: |path| {
                anyhow::anyhow!(
                    "dirty source-universe operator-inputs {path}: existing file content differs"
                )
            },
            write_error: |path, error| {
                anyhow::anyhow!("write source-universe operator-inputs {path}: {error}")
            },
        },
    )?;

    Ok(SourceUniverseOperatorInputsArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        record_count: inputs.records.len() as u64,
    })
}

pub fn evaluate_source_universe_operator_inputs(
    spec: &SourceUniverseOperatorInputsSpec,
    base_dir: &Path,
) -> Result<SourceUniverseOperatorInputs> {
    ensure!(
        !spec.input_id.trim().is_empty(),
        "input_id must not be empty"
    );
    ensure!(
        !spec.operator_run_id_prefix.trim().is_empty(),
        "operator_run_id_prefix must not be empty"
    );
    ensure!(
        !spec.nt_venue.trim().is_empty(),
        "nt_venue must not be empty"
    );
    ensure!(
        !spec.converter_identity.trim().is_empty(),
        "converter_identity must not be empty"
    );
    ensure!(
        !spec.converter_version.trim().is_empty(),
        "converter_version must not be empty"
    );
    ensure!(
        spec.max_decoded_bytes > 0,
        "max_decoded_bytes must be positive"
    );
    ensure!(spec.max_source_rows > 0, "max_source_rows must be positive");
    ensure!(
        spec.max_projected_row_groups > 0,
        "max_projected_row_groups must be positive"
    );
    ensure!(
        spec.max_wall_seconds > 0,
        "max_wall_seconds must be positive"
    );

    let (gates_path, gates_hash, gates): (
        PathBuf,
        String,
        SourceUniverseObjectGateMaterialization,
    ) = read_json_artifact(
        base_dir,
        &spec.source_universe_object_gates_path,
        "source_universe_object_gates",
    )?;
    let (run_plan_path, run_plan_hash, run_plan): (
        PathBuf,
        String,
        SourceUniverseConversionRunPlan,
    ) = read_json_artifact(
        base_dir,
        &spec.source_universe_conversion_run_plan_path,
        "source_universe_conversion_run_plan",
    )?;
    let (conversion_plan_path, conversion_plan_hash, conversion_plan): (
        PathBuf,
        String,
        SourceUniverseConversionPlan,
    ) = read_json_artifact(
        base_dir,
        &spec.source_universe_conversion_plan_path,
        "source_universe_conversion_plan",
    )?;
    let (metadata_path, metadata_hash, metadata): (
        PathBuf,
        String,
        VenueInstrumentMetadataSnapshot,
    ) = read_json_artifact(
        base_dir,
        &spec.instrument_metadata_snapshot_path,
        "instrument_metadata_snapshot",
    )?;

    ensure!(
        gates.status == SourceUniverseObjectGateStatus::Ready,
        "source-universe object gates are not ready"
    );
    ensure!(
        run_plan.status == SourceUniverseConversionRunPlanStatus::Ready,
        "source-universe conversion run plan is not ready"
    );
    ensure!(
        gates.universe_id == run_plan.universe_id
            && gates.gate_id == run_plan.gate_id
            && gates.queue_id == run_plan.queue_id
            && gates.manifest_id == run_plan.manifest_id,
        "source-universe object gates and run plan identity mismatch"
    );
    ensure!(
        gates.accepted_gate_count == run_plan.planned_object_count,
        "source-universe object gates and run plan object counts differ"
    );
    ensure!(
        gates.total_accepted_bytes == run_plan.planned_source_bytes,
        "source-universe object gates and run plan source bytes differ"
    );

    let converter_mappings = converter_mappings(&conversion_plan)?;
    let converter_mappings_by_key = converter_mappings
        .iter()
        .map(|mapping| {
            (
                (mapping.source_binding.clone(), mapping.category.clone()),
                mapping,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let metadata_by_key = metadata_records_by_key(metadata.records)?;

    let mut instrument_specs =
        BTreeMap::<String, SourceUniverseOperatorInstrumentSpecRecord>::new();
    let mut records = Vec::with_capacity(gates.records.len());
    let mut global_blocking_reasons = Vec::new();
    let mut seen_work_items = BTreeSet::new();

    for (index, gate) in gates.records.iter().enumerate() {
        ensure!(
            seen_work_items.insert(gate.work_item_id.clone()),
            "duplicate source-universe operator input work item {}",
            gate.work_item_id
        );
        let converter_mapping = converter_mappings_by_key
            .get(&(gate.source_binding.clone(), gate.category.clone()))
            .copied();
        let metadata_record = metadata_by_key.get(&(
            gate.source_binding.clone(),
            gate.category.clone(),
            gate.symbol.clone(),
        ));
        let instrument_key = instrument_key(&gate.source_binding, &gate.category, &gate.symbol);
        let mut blocking_reasons = Vec::new();
        if gate.gate_status != SourceUniverseObjectGateStatus::Ready {
            blocking_reasons.push("source_universe_object_gate_not_ready".to_string());
        }
        if gate.selected_object_sha256.trim().is_empty() {
            blocking_reasons.push("source_universe_object_gate_missing_sha256".to_string());
        }
        if converter_mapping.is_none() {
            blocking_reasons.push("missing_converter_mapping".to_string());
        }
        if metadata_record.is_none() {
            blocking_reasons.push("missing_instrument_metadata".to_string());
        }

        if let Some(metadata_record) = metadata_record {
            match v5_market_instruments_info_spec(metadata_record, &spec.nt_venue, spec) {
                Ok(instrument_spec) => {
                    if let Err(error) = instrument_spec.build_instrument_any() {
                        blocking_reasons.push(format!("invalid_nt_instrument_spec:{error}"));
                    } else {
                        instrument_specs
                            .entry(instrument_key.clone())
                            .or_insert_with(|| SourceUniverseOperatorInstrumentSpecRecord {
                                instrument_key: instrument_key.clone(),
                                source_binding: gate.source_binding.clone(),
                                category: gate.category.clone(),
                                symbol: gate.symbol.clone(),
                                nt_instrument_id: nt_instrument_id(&gate.symbol, &spec.nt_venue),
                                metadata_source_uri: metadata_record.source_uri.clone(),
                                instrument_spec,
                            });
                    }
                }
                Err(error) => {
                    blocking_reasons.push(format!("invalid_instrument_metadata:{error}"));
                }
            }
        }

        let schema_columns = converter_mapping.map(|mapping| mapping.schema_columns.clone());
        let converter_csv = converter_mapping.map(|mapping| mapping.converter_csv.clone());
        blocking_reasons.sort();
        blocking_reasons.dedup();
        let status = if blocking_reasons.is_empty() {
            SourceUniverseOperatorInputRecordStatus::Ready
        } else {
            SourceUniverseOperatorInputRecordStatus::Blocked
        };

        records.push(SourceUniverseOperatorInputRecord {
            work_item_id: gate.work_item_id.clone(),
            status,
            operator_run_id: format!("{}-{index:05}", spec.operator_run_id_prefix),
            source_binding: gate.source_binding.clone(),
            category: gate.category.clone(),
            symbol: gate.symbol.clone(),
            archive_date: gate.archive_date.clone(),
            source_uri: gate.source_uri.clone(),
            source_url: gate.source_url.clone(),
            selected_object_sha256: gate.selected_object_sha256.clone(),
            selected_object_bytes: gate.selected_object_bytes,
            source_proof_id: gate.source_proof_id.clone(),
            source_proof_version: gate.source_proof_version,
            accepted_tranche_id: gate.accepted_tranche_id.clone(),
            output_prefix: gate.output_prefix.clone(),
            instrument_key,
            converter_identity: spec.converter_identity.clone(),
            converter_version: spec.converter_version.clone(),
            raw_payload_container: spec.raw_payload_container,
            zip_member: zip_member_for_source_url(&gate.source_url, spec.raw_payload_container)?,
            max_decoded_bytes: spec.max_decoded_bytes,
            max_source_rows: spec.max_source_rows,
            max_projected_row_groups: spec.max_projected_row_groups,
            max_wall_seconds: spec.max_wall_seconds,
            schema_columns,
            converter_csv,
            blocking_reasons,
        });
    }

    let ready_input_count = records
        .iter()
        .filter(|record| record.status == SourceUniverseOperatorInputRecordStatus::Ready)
        .count() as u64;
    let blocked_input_count = records.len() as u64 - ready_input_count;
    if blocked_input_count > 0 {
        global_blocking_reasons.push("blocked_operator_input_records".to_string());
    }
    if ready_input_count != gates.accepted_gate_count {
        global_blocking_reasons
            .push("ready_operator_inputs_do_not_cover_all_object_gates".to_string());
    }
    global_blocking_reasons.sort();
    global_blocking_reasons.dedup();
    let status = if global_blocking_reasons.is_empty() {
        SourceUniverseOperatorInputsStatus::Ready
    } else {
        SourceUniverseOperatorInputsStatus::Blocked
    };

    Ok(SourceUniverseOperatorInputs {
        schema_version: SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION.to_string(),
        input_id: spec.input_id.clone(),
        status,
        gate_id: gates.gate_id,
        conversion_run_plan_id: run_plan.plan_id,
        universe_id: run_plan.universe_id,
        venue: run_plan.venue,
        source: run_plan.source,
        family: run_plan.family,
        table_family: run_plan.table_family,
        operator_run_id_prefix: spec.operator_run_id_prefix.clone(),
        nt_venue: spec.nt_venue.clone(),
        converter_identity: spec.converter_identity.clone(),
        converter_version: spec.converter_version.clone(),
        raw_payload_container: spec.raw_payload_container,
        max_decoded_bytes: spec.max_decoded_bytes,
        max_source_rows: spec.max_source_rows,
        max_projected_row_groups: spec.max_projected_row_groups,
        max_wall_seconds: spec.max_wall_seconds,
        planned_object_count: run_plan.planned_object_count,
        planned_source_bytes: run_plan.planned_source_bytes,
        conversion_run_count: run_plan.run_count,
        instrument_spec_count: instrument_specs.len() as u64,
        converter_mapping_count: converter_mappings.len() as u64,
        ready_input_count,
        blocked_input_count,
        artifact_refs: vec![
            artifact_ref(
                "source_universe_object_gates",
                gates_path,
                &spec.source_universe_object_gates_path,
                gates_hash,
            )?,
            artifact_ref(
                "source_universe_conversion_run_plan",
                run_plan_path,
                &spec.source_universe_conversion_run_plan_path,
                run_plan_hash,
            )?,
            artifact_ref(
                "source_universe_conversion_plan",
                conversion_plan_path,
                &spec.source_universe_conversion_plan_path,
                conversion_plan_hash,
            )?,
            artifact_ref(
                "instrument_metadata_snapshot",
                metadata_path,
                &spec.instrument_metadata_snapshot_path,
                metadata_hash,
            )?,
        ],
        converter_mappings,
        instrument_specs: instrument_specs.into_values().collect(),
        records,
        blocking_reasons: global_blocking_reasons,
    })
}

/// Map one venue instrument-info record to a typed NT instrument spec.
///
/// This parser implements exactly ONE venue REST schema family - the
/// `/v5/market/instruments-info` endpoint shape (`baseCoin`/`quoteCoin`/
/// `priceFilter`/`lotSizeFilter` fields plus the `category` taxonomy) of the
/// venue declared by the operator-input source bindings; venue identity stays
/// in that TOML, never in this code. The field-name literals below are that
/// schema's deserialization contract, the same role `#[serde(rename)]`
/// attributes would play in a typed struct. A record from any other venue
/// schema fails loud at the `required_*` lookups and surfaces as an
/// `invalid_instrument_metadata` blocking reason; supporting a second venue
/// means adding a sibling parser for its schema, not loosening this one.
fn v5_market_instruments_info_spec(
    record: &VenueInstrumentMetadataRecord,
    nt_venue: &str,
    spec: &SourceUniverseOperatorInputsSpec,
) -> Result<CatalogInstrumentSpec> {
    let instrument = &record.instrument;
    let symbol = required_string(instrument, "symbol")?;
    ensure!(symbol == record.symbol, "metadata symbol mismatch");
    let base_currency = required_string(instrument, "baseCoin")?;
    let quote_currency = required_string(instrument, "quoteCoin")?;
    let price_filter = required_object(instrument, "priceFilter")?;
    let lot_size_filter = required_object(instrument, "lotSizeFilter")?;
    let price_increment = required_string(price_filter, "tickSize")?;
    let nt_instrument_id = nt_instrument_id(&symbol, nt_venue);

    if record.category == "spot" {
        return Ok(CatalogInstrumentSpec::Spot(SpotInstrumentSpec {
            nt_instrument_id,
            raw_symbol: symbol,
            base_currency,
            quote_currency,
            price_increment,
            size_increment: required_string(lot_size_filter, "basePrecision")?,
            min_quantity: required_string(lot_size_filter, "minOrderQty")?,
            max_quantity: first_string(lot_size_filter, &["maxOrderQty", "maxLimitOrderQty"])?,
            min_notional: first_string(lot_size_filter, &["minOrderAmt", "minNotionalValue"])?,
            max_notional: first_string(lot_size_filter, &["maxOrderAmt"])
                .unwrap_or_else(|_| spec.default_spot_max_notional.clone()),
        }));
    }

    let contract_type = required_string(instrument, "contractType")?;
    let settlement_currency = required_string(instrument, "settleCoin")?;
    let is_inverse = instrument
        .get("isInverse")
        .and_then(Value::as_bool)
        .unwrap_or(record.category == "inverse");
    let max_notional = spec.default_derivative_max_notional.clone();
    let common = DerivativeSpecFields {
        nt_instrument_id,
        raw_symbol: symbol,
        base_currency,
        quote_currency,
        settlement_currency,
        is_inverse,
        price_increment,
        size_increment: required_string(lot_size_filter, "qtyStep")?,
        min_quantity: required_string(lot_size_filter, "minOrderQty")?,
        max_quantity: required_string(lot_size_filter, "maxOrderQty")?,
        min_notional: required_string(lot_size_filter, "minNotionalValue")?,
        max_notional,
        multiplier: Some(spec.default_derivative_multiplier.clone()),
        lot_size: Some(required_string(lot_size_filter, "qtyStep")?),
        max_price: Some(required_string(price_filter, "maxPrice")?),
        min_price: Some(required_string(price_filter, "minPrice")?),
        maker_fee: Some(spec.default_maker_fee.clone()),
        taker_fee: Some(spec.default_taker_fee.clone()),
    };

    if contract_type.ends_with("Perpetual") {
        Ok(CatalogInstrumentSpec::CryptoPerpetual(
            CryptoPerpetualInstrumentSpec {
                instrument_kind: CryptoPerpetualInstrumentKind::CryptoPerpetual,
                nt_instrument_id: common.nt_instrument_id,
                raw_symbol: common.raw_symbol,
                base_currency: common.base_currency,
                quote_currency: common.quote_currency,
                settlement_currency: common.settlement_currency,
                is_inverse: common.is_inverse,
                price_increment: common.price_increment,
                size_increment: common.size_increment,
                min_quantity: common.min_quantity,
                max_quantity: common.max_quantity,
                min_notional: common.min_notional,
                max_notional: common.max_notional,
                multiplier: common.multiplier,
                lot_size: common.lot_size,
                max_price: common.max_price,
                min_price: common.min_price,
                margin_init: None,
                margin_maint: None,
                maker_fee: common.maker_fee,
                taker_fee: common.taker_fee,
            },
        ))
    } else if contract_type.ends_with("Futures") {
        Ok(CatalogInstrumentSpec::CryptoFuture(
            CryptoFutureInstrumentSpec {
                instrument_kind: CryptoFutureInstrumentKind::CryptoFuture,
                nt_instrument_id: common.nt_instrument_id,
                raw_symbol: common.raw_symbol,
                base_currency: common.base_currency,
                quote_currency: common.quote_currency,
                settlement_currency: common.settlement_currency,
                is_inverse: common.is_inverse,
                activation_time_nanos: millis_string_to_nanos(instrument, "launchTime")?,
                expiration_time_nanos: millis_string_to_nanos(instrument, "deliveryTime")?,
                price_increment: common.price_increment,
                size_increment: common.size_increment,
                min_quantity: common.min_quantity,
                max_quantity: common.max_quantity,
                min_notional: common.min_notional,
                max_notional: common.max_notional,
                multiplier: common.multiplier,
                lot_size: common.lot_size,
                max_price: common.max_price,
                min_price: common.min_price,
                margin_init: None,
                margin_maint: None,
                maker_fee: common.maker_fee,
                taker_fee: common.taker_fee,
            },
        ))
    } else {
        anyhow::bail!("unsupported contractType {contract_type:?}")
    }
}

struct DerivativeSpecFields {
    nt_instrument_id: String,
    raw_symbol: String,
    base_currency: String,
    quote_currency: String,
    settlement_currency: String,
    is_inverse: bool,
    price_increment: String,
    size_increment: String,
    min_quantity: String,
    max_quantity: String,
    min_notional: String,
    max_notional: String,
    multiplier: Option<String>,
    lot_size: Option<String>,
    max_price: Option<String>,
    min_price: Option<String>,
    maker_fee: Option<String>,
    taker_fee: Option<String>,
}

fn converter_mappings(
    plan: &SourceUniverseConversionPlan,
) -> Result<Vec<SourceUniverseOperatorConverterMapping>> {
    let mut seen = BTreeSet::new();
    let mut mappings = Vec::new();
    for batch in &plan.category_batches {
        ensure!(
            seen.insert((batch.source_binding.clone(), batch.category.clone())),
            "duplicate converter mapping for source_binding={} category={}",
            batch.source_binding,
            batch.category
        );
        ensure!(
            batch.status == "converter_mapping_configured",
            "converter mapping for source_binding={} category={} is not configured",
            batch.source_binding,
            batch.category
        );
        mappings.push(SourceUniverseOperatorConverterMapping {
            source_binding: batch.source_binding.clone(),
            category: batch.category.clone(),
            schema_columns: batch.schema_columns.clone(),
            converter_csv: batch.converter_csv.clone(),
        });
    }
    Ok(mappings)
}

fn metadata_records_by_key(
    records: Vec<VenueInstrumentMetadataRecord>,
) -> Result<BTreeMap<(String, String, String), VenueInstrumentMetadataRecord>> {
    let mut by_key = BTreeMap::new();
    for record in records {
        let key = (
            record.source_binding.clone(),
            record.category.clone(),
            record.symbol.clone(),
        );
        ensure!(
            by_key.insert(key.clone(), record).is_none(),
            "duplicate instrument metadata record for source_binding={} category={} symbol={}",
            key.0,
            key.1,
            key.2
        );
    }
    Ok(by_key)
}

fn required_object<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    let child = value
        .get(key)
        .with_context(|| format!("missing object field {key:?}"))?;
    ensure!(child.is_object(), "field {key:?} must be an object");
    Ok(child)
}

fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("missing string field {key:?}"))
}

fn first_string(value: &Value, keys: &[&str]) -> Result<String> {
    for key in keys {
        if let Some(value) = value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(value.to_string());
        }
    }
    anyhow::bail!("missing string field in {keys:?}")
}

fn millis_string_to_nanos(value: &Value, key: &str) -> Result<u64> {
    let millis = required_string(value, key)?
        .parse::<u64>()
        .with_context(|| format!("parse millisecond timestamp field {key:?}"))?;
    millis
        .checked_mul(1_000_000)
        .with_context(|| format!("millisecond timestamp field {key:?} overflows nanoseconds"))
}

fn nt_instrument_id(symbol: &str, nt_venue: &str) -> String {
    format!("{symbol}.{nt_venue}")
}

fn instrument_key(source_binding: &str, category: &str, symbol: &str) -> String {
    format!("{source_binding}:{category}:{symbol}")
}

fn zip_member_for_source_url(
    source_url: &str,
    container: RawPayloadContainer,
) -> Result<Option<String>> {
    if container != RawPayloadContainer::SingleCsvZip {
        return Ok(None);
    }
    let file_name = source_url
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing source filename in {source_url:?}"))?;
    let zip_stem = file_name.strip_suffix(".zip").with_context(|| {
        format!("single_csv_zip source filename must end in .zip: {file_name:?}")
    })?;
    Ok(Some(format!("{zip_stem}.csv")))
}

fn artifact_ref(
    role: &str,
    path: PathBuf,
    spec_path: &Path,
    sha256: String,
) -> Result<ReferenceArtifactPin> {
    Ok(ReferenceArtifactPin {
        role: role.to_string(),
        path: portable_artifact_path_for_spec(&path, spec_path)?,
        sha256,
    })
}
fn read_json_artifact<T>(base_dir: &Path, path: &Path, role: &str) -> Result<(PathBuf, String, T)>
where
    T: for<'de> Deserialize<'de>,
{
    let resolved = resolve_existing_path(base_dir, path);
    let bytes =
        fs::read(&resolved).with_context(|| format!("read {role} {}", resolved.display()))?;
    let hash = sha256_hex(&bytes);
    let parsed = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {role} {}", path.display()))?;
    Ok((resolved, hash, parsed))
}
#[derive(Debug, Deserialize)]
struct SourceUniverseConversionPlan {
    category_batches: Vec<SourceUniverseConversionPlanCategoryBatch>,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseConversionPlanCategoryBatch {
    category: String,
    source_binding: String,
    status: String,
    schema_columns: Vec<String>,
    converter_csv: CsvTradeMappingConfig,
}

#[derive(Debug, Deserialize)]
struct VenueInstrumentMetadataSnapshot {
    records: Vec<VenueInstrumentMetadataRecord>,
}

#[derive(Debug, Clone, Deserialize)]
struct VenueInstrumentMetadataRecord {
    category: String,
    source_binding: String,
    source_uri: String,
    symbol: String,
    instrument: Value,
}
