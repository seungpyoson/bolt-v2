//! Source-universe operator execution-pack materialization.
//!
//! This artifact turns executable source-universe work-order records into the
//! existing single-object operator inputs: a materialized run spec plus a ready
//! backfill execution plan per record. It does not fetch payload bytes or run
//! conversion.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use nautilus_model::{data::BarType, identifiers::InstrumentId};
use serde::{Deserialize, Serialize};
use toml::Value;

use crate::atomic_artifact_write::atomic_write;
use crate::hashing::sha256_hex;
use crate::path_resolution::{
    portable_artifact_path_for_spec, resolve_existing_path, resolve_output_dir,
};
use crate::reference_artifact::ReferenceArtifactPin;
use crate::{
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE, BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION,
        BackfillAcceptedTrancheManifest, BackfillAcceptedTrancheObject,
        BackfillAcceptedTrancheStatus,
    },
    backfill_execution_plan::{
        BackfillExecutionPlanStatus, BackfillExecutionRunBinding, BackfillExecutionWorkBudget,
        evaluate_backfill_execution_plan, write_backfill_execution_plan_with_overwrite,
    },
    canonical_trades::{CanonicalInstrumentIdentity, ConverterConfig, RawPayloadConfig},
    catalog_projection::CatalogInstrumentSpec,
    operator::RunSpec,
    source_proof::{AcceptanceScope, SourceProofReport, SourceProofStatus},
    source_universe_conversion_work_order::{
        SourceUniverseConversionWorkOrder, SourceUniverseConversionWorkOrderRecord,
    },
    source_universe_object_gates::{
        SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateRecord,
    },
    source_universe_operator_inputs::{
        SourceUniverseOperatorInputRecord, SourceUniverseOperatorInputRecordStatus,
        SourceUniverseOperatorInputs, SourceUniverseOperatorInstrumentSpecRecord,
    },
};

pub const SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION: &str = "source-universe-execution-pack.v1";
pub const SOURCE_UNIVERSE_EXECUTION_PACK_FILE: &str = "source-universe-execution-pack.json";
pub const SOURCE_UNIVERSE_EXECUTION_PACK_RUN_SPEC_FILE: &str = "run-spec.toml";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionPackSpec {
    pub pack_id: String,
    pub source_universe_conversion_work_order_path: PathBuf,
    pub run_spec_template_path: PathBuf,
    pub output_dir: PathBuf,
    pub venue_account_types: SourceUniverseExecutionPackVenueAccountTypes,
    #[serde(default)]
    pub overwrite_existing_artifacts: bool,
    #[serde(default)]
    pub record_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionPackVenueAccountTypes {
    pub spot: String,
    pub crypto_perpetual: String,
    pub crypto_future: String,
}

impl SourceUniverseExecutionPackVenueAccountTypes {
    fn account_type_for(&self, instrument_spec: &CatalogInstrumentSpec) -> Result<&str> {
        let value = match instrument_spec {
            CatalogInstrumentSpec::Spot(_) => &self.spot,
            CatalogInstrumentSpec::CryptoPerpetual(_) => &self.crypto_perpetual,
            CatalogInstrumentSpec::CryptoFuture(_) => &self.crypto_future,
            // The source-universe execution pack maps crypto-venue REST
            // instrument families to venue account types. Binary options come
            // from prediction-market archives through the generic catalog
            // projection, never through this crypto-venue pipeline, so one
            // reaching here is a contract violation rather than a missing
            // config field.
            CatalogInstrumentSpec::BinaryOption(_) => bail!(
                "source-universe execution pack does not support binary-option instrument specs"
            ),
        };
        ensure!(
            !value.trim().is_empty(),
            "source-universe execution-pack venue account type must not be empty"
        );
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseExecutionPackStatus {
    Ready,
    PartiallyReady,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionPackRecord {
    pub sequence: u64,
    pub work_item_id: String,
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
    pub run_spec_path: PathBuf,
    pub run_spec_sha256: String,
    pub accepted_tranche_path: PathBuf,
    pub accepted_tranche_sha256: String,
    pub execution_plan_path: PathBuf,
    pub execution_plan_sha256: String,
}

impl SourceUniverseExecutionPackRecord {
    /// The on-disk artifact paths this record advertises: its run spec, accepted
    /// tranche manifest, and execution plan. Single source of truth for which
    /// files a record points at, so eviction/restore tooling and the eviction
    /// guard test enumerate the full set rather than a hand-picked subset. Add
    /// any new artifact-path field here so every consumer stays exhaustive.
    pub fn artifact_paths(&self) -> [&Path; 3] {
        [
            &self.run_spec_path,
            &self.accepted_tranche_path,
            &self.execution_plan_path,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionPack {
    pub schema_version: String,
    pub pack_id: String,
    pub status: SourceUniverseExecutionPackStatus,
    pub work_order_id: String,
    pub input_id: String,
    pub gate_id: String,
    pub conversion_run_plan_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub planned_object_count: u64,
    pub executable_record_count: u64,
    pub withheld_record_count: u64,
    pub selected_record_count: u64,
    pub materialized_record_count: u64,
    pub skipped_executable_record_count: u64,
    pub executable_source_bytes: u64,
    pub materialized_source_bytes: u64,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub records: Vec<SourceUniverseExecutionPackRecord>,
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseExecutionPackArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub materialized_record_count: u64,
}

pub fn write_source_universe_execution_pack_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseExecutionPackArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe execution-pack spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseExecutionPackSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source-universe execution-pack spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_execution_pack(&spec, base_dir)
}

pub fn write_source_universe_execution_pack(
    spec: &SourceUniverseExecutionPackSpec,
    base_dir: &Path,
) -> Result<SourceUniverseExecutionPackArtifact> {
    ensure!(
        !spec.pack_id.trim().is_empty(),
        "source-universe execution-pack pack_id must not be empty"
    );
    if let Some(record_limit) = spec.record_limit {
        ensure!(record_limit > 0, "record_limit must be positive when set");
    }

    let (work_order_path, work_order_hash, work_order): (
        PathBuf,
        String,
        SourceUniverseConversionWorkOrder,
    ) = read_json_artifact(
        base_dir,
        &spec.source_universe_conversion_work_order_path,
        None,
        "source_universe_conversion_work_order",
    )?;
    let operator_inputs_ref =
        work_order_artifact_ref(&work_order, "source_universe_operator_inputs")?;
    let (operator_inputs_path, operator_inputs_hash, operator_inputs): (
        PathBuf,
        String,
        SourceUniverseOperatorInputs,
    ) = read_json_artifact(
        base_dir,
        &operator_inputs_ref.path,
        Some(operator_inputs_ref.sha256.as_str()),
        "source_universe_operator_inputs",
    )?;
    let object_gates_ref =
        operator_inputs_artifact_ref(&operator_inputs, "source_universe_object_gates")?;
    let (object_gates_path, object_gates_hash, object_gates): (
        PathBuf,
        String,
        SourceUniverseObjectGateMaterialization,
    ) = read_json_artifact(
        base_dir,
        &object_gates_ref.path,
        Some(object_gates_ref.sha256.as_str()),
        "source_universe_object_gates",
    )?;

    let template_path = resolve_existing_path(base_dir, &spec.run_spec_template_path);
    let template_text = fs::read_to_string(&template_path)
        .with_context(|| format!("read run-spec template {}", template_path.display()))?;
    let template_hash = sha256_hex(template_text.as_bytes());
    let template: Value = toml::from_str(&template_text)
        .with_context(|| format!("parse run-spec template TOML {}", template_path.display()))?;
    let _: RunSpec = toml::from_str(&template_text).with_context(|| {
        format!(
            "run-spec template does not deserialize {}",
            template_path.display()
        )
    })?;

    let proofs = source_proofs_by_id(base_dir, &object_gates)?;
    let inputs_by_work_item = operator_inputs_by_work_item(&operator_inputs)?;
    let instruments_by_key = instruments_by_key(&operator_inputs)?;
    let gates_by_work_item = gates_by_work_item(&object_gates)?;

    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe execution-pack output directory {}",
            output_dir.display()
        )
    })?;

    let selected_records = selected_records(&work_order, spec.record_limit);
    let mut materialized_records = Vec::with_capacity(selected_records.len());
    let mut materialized_source_bytes = 0_u64;
    let mut used_source_proof_ids = BTreeSet::new();

    for record in &selected_records {
        let input = inputs_by_work_item
            .get(&record.work_item_id)
            .with_context(|| format!("missing operator input for {}", record.work_item_id))?;
        ensure!(
            input.status == SourceUniverseOperatorInputRecordStatus::Ready,
            "operator input {} is not ready",
            record.work_item_id
        );
        ensure!(
            input.operator_run_id == record.operator_run_id
                && input.source_binding == record.source_binding
                && input.instrument_key == record.instrument_key
                && input.selected_object_sha256 == record.selected_object_sha256,
            "operator input and work-order record drift for {}",
            record.work_item_id
        );
        let instrument = instruments_by_key
            .get(&record.instrument_key)
            .with_context(|| format!("missing instrument spec for {}", record.instrument_key))?;
        let proof = &proofs
            .get(&record.source_proof_id)
            .with_context(|| {
                format!(
                    "missing source proof {} for {}",
                    record.source_proof_id, record.work_item_id
                )
            })?
            .report;
        ensure!(
            proof.source_proof_version == record.source_proof_version,
            "source proof version mismatch for {}",
            record.work_item_id
        );
        ensure!(
            proof.status == SourceProofStatus::Accepted,
            "source proof {} is not accepted",
            proof.source_proof_id
        );
        used_source_proof_ids.insert(record.source_proof_id.clone());

        let run_dir = output_dir.join("runs").join(format!(
            "{:05}-{}",
            record.sequence,
            slug(&record.operator_run_id)
        ));
        fs::create_dir_all(&run_dir).with_context(|| {
            format!(
                "create source-universe execution run dir {}",
                run_dir.display()
            )
        })?;

        let run_spec_text = materialize_run_spec(
            &template,
            record,
            input,
            instrument,
            proof,
            &operator_inputs,
            &spec.venue_account_types,
        )?;
        let run_spec_bytes = run_spec_text.as_bytes();
        let run_spec_hash = sha256_hex(run_spec_bytes);
        let run_spec_path = run_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_RUN_SPEC_FILE);
        write_bytes_if_clean(
            &run_spec_path,
            run_spec_bytes,
            spec.overwrite_existing_artifacts,
        )?;
        let run_spec: RunSpec = toml::from_str(&run_spec_text).with_context(|| {
            format!(
                "materialized run spec does not deserialize {}",
                run_spec_path.display()
            )
        })?;

        let gate = gates_by_work_item.get(&record.work_item_id).copied();
        let accepted_tranche =
            accepted_tranche_for_record(record, proof, &operator_inputs.table_family, gate);
        let accepted_tranche_path = run_dir.join(BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE);
        let rewrite = if spec.overwrite_existing_artifacts {
            crate::reference_artifact::ReferenceArtifactRewrite::OverwriteIfChanged
        } else {
            crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
        };
        let accepted_tranche_artifact =
            crate::reference_artifact::write_reference_artifact_with_len(
                &accepted_tranche_path,
                BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE,
                &accepted_tranche,
                rewrite,
            )
            .with_context(|| {
                format!(
                    "write source-universe accepted tranche {}",
                    accepted_tranche_path.display()
                )
            })?;
        let accepted_tranche_hash = accepted_tranche_artifact.pin.sha256.clone();

        let execution_plan = evaluate_backfill_execution_plan(
            format!("{}:execution-plan", record.operator_run_id),
            accepted_tranche_hash.clone(),
            &accepted_tranche,
            run_spec_hash.clone(),
            &BackfillExecutionRunBinding::from_run_spec(&run_spec),
            BackfillExecutionWorkBudget {
                max_source_rows: record.max_source_rows,
                max_projected_row_groups: record.max_projected_row_groups,
                max_wall_seconds: record.max_wall_seconds,
                require_object_selection_metadata: false,
            },
        );
        ensure!(
            execution_plan.status == BackfillExecutionPlanStatus::Ready,
            "source-universe execution plan for {} is not ready: {:?}",
            record.work_item_id,
            execution_plan.blocking_issues
        );
        let execution_plan_artifact = write_backfill_execution_plan_with_overwrite(
            &run_dir,
            &execution_plan,
            spec.overwrite_existing_artifacts,
        )
        .with_context(|| format!("write execution plan for {}", record.work_item_id))?;

        materialized_source_bytes += record.selected_object_bytes;
        materialized_records.push(SourceUniverseExecutionPackRecord {
            sequence: record.sequence,
            work_item_id: record.work_item_id.clone(),
            operator_run_id: record.operator_run_id.clone(),
            source_binding: record.source_binding.clone(),
            category: record.category.clone(),
            symbol: record.symbol.clone(),
            archive_date: record.archive_date.clone(),
            source_uri: record.source_uri.clone(),
            source_url: record.source_url.clone(),
            selected_object_sha256: record.selected_object_sha256.clone(),
            selected_object_bytes: record.selected_object_bytes,
            source_proof_id: record.source_proof_id.clone(),
            source_proof_version: record.source_proof_version,
            accepted_tranche_id: record.accepted_tranche_id.clone(),
            output_prefix: run_spec.manifest.output_prefix.clone(),
            run_spec_path: portable_artifact_path_for_spec(&run_spec_path, &spec.output_dir)?,
            run_spec_sha256: run_spec_hash,
            accepted_tranche_path: portable_artifact_path_for_spec(
                &accepted_tranche_path,
                &spec.output_dir,
            )?,
            accepted_tranche_sha256: accepted_tranche_hash,
            execution_plan_path: portable_artifact_path_for_spec(
                &execution_plan_artifact.path,
                &spec.output_dir,
            )?,
            execution_plan_sha256: execution_plan_artifact.content_hash,
        });
    }

    let materialized_record_count = materialized_records.len() as u64;
    let selected_record_count = selected_records.len() as u64;
    let skipped_executable_record_count = work_order
        .executable_record_count
        .saturating_sub(materialized_record_count);
    let mut blocking_reasons = work_order.blocking_reasons.clone();
    if materialized_record_count == 0 {
        blocking_reasons.push("no_source_universe_execution_records_materialized".to_string());
    }
    if skipped_executable_record_count > 0 {
        blocking_reasons.push("record_limit_skipped_executable_records".to_string());
    }
    blocking_reasons.sort();
    blocking_reasons.dedup();
    let status = if materialized_record_count == 0 {
        SourceUniverseExecutionPackStatus::Blocked
    } else if skipped_executable_record_count > 0 || work_order.withheld_record_count > 0 {
        SourceUniverseExecutionPackStatus::PartiallyReady
    } else {
        SourceUniverseExecutionPackStatus::Ready
    };

    let mut artifact_refs = vec![
        ReferenceArtifactPin {
            role: "source_universe_conversion_work_order".to_string(),
            path: portable_artifact_path_for_spec(
                &work_order_path,
                &spec.source_universe_conversion_work_order_path,
            )?,
            sha256: work_order_hash,
        },
        ReferenceArtifactPin {
            role: "source_universe_operator_inputs".to_string(),
            path: portable_artifact_path_for_spec(
                &operator_inputs_path,
                &operator_inputs_ref.path,
            )?,
            sha256: operator_inputs_hash,
        },
        ReferenceArtifactPin {
            role: "source_universe_object_gates".to_string(),
            path: portable_artifact_path_for_spec(&object_gates_path, &object_gates_ref.path)?,
            sha256: object_gates_hash,
        },
        ReferenceArtifactPin {
            role: "run_spec_template".to_string(),
            path: portable_artifact_path_for_spec(&template_path, &spec.run_spec_template_path)?,
            sha256: template_hash,
        },
    ];
    for proof_id in used_source_proof_ids {
        // Reuse the artifact ref `source_proofs_by_id` already read, sha-verified,
        // and parsed for this id; every used id is guaranteed present in `proofs`
        // because it was selected from the same validated map above. No second
        // filesystem read, so there is no read/parse failure to swallow.
        let proof_ref = &proofs
            .get(&proof_id)
            .with_context(|| format!("missing validated source proof {proof_id} for artifact ref"))?
            .artifact_ref;
        artifact_refs.push(ReferenceArtifactPin {
            role: "source_proof".to_string(),
            path: portable_artifact_path_for_spec(
                &resolve_existing_path(base_dir, &proof_ref.path),
                &proof_ref.path,
            )?,
            sha256: proof_ref.sha256.clone(),
        });
    }

    let pack = SourceUniverseExecutionPack {
        schema_version: SOURCE_UNIVERSE_EXECUTION_PACK_SCHEMA_VERSION.to_string(),
        pack_id: spec.pack_id.clone(),
        status,
        work_order_id: work_order.work_order_id,
        input_id: work_order.input_id,
        gate_id: work_order.gate_id,
        conversion_run_plan_id: work_order.conversion_run_plan_id,
        universe_id: work_order.universe_id,
        venue: work_order.venue,
        source: work_order.source,
        family: work_order.family,
        table_family: work_order.table_family,
        planned_object_count: work_order.planned_object_count,
        executable_record_count: work_order.executable_record_count,
        withheld_record_count: work_order.withheld_record_count,
        selected_record_count,
        materialized_record_count,
        skipped_executable_record_count,
        executable_source_bytes: work_order.executable_source_bytes,
        materialized_source_bytes,
        artifact_refs,
        records: materialized_records,
        blocking_reasons,
    };

    let pack_path = output_dir.join(SOURCE_UNIVERSE_EXECUTION_PACK_FILE);
    let rewrite = if spec.overwrite_existing_artifacts {
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteIfChanged
    } else {
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
    };
    let pack_artifact = crate::reference_artifact::write_reference_artifact_with_len(
        &pack_path,
        SOURCE_UNIVERSE_EXECUTION_PACK_FILE,
        &pack,
        rewrite,
    )
    .with_context(|| {
        format!(
            "write source-universe execution pack {}",
            pack_path.display()
        )
    })?;
    Ok(SourceUniverseExecutionPackArtifact {
        path: pack_path,
        content_hash: pack_artifact.pin.sha256,
        bytes: pack_artifact.bytes,
        materialized_record_count,
    })
}

fn selected_records(
    work_order: &SourceUniverseConversionWorkOrder,
    record_limit: Option<u64>,
) -> Vec<&SourceUniverseConversionWorkOrderRecord> {
    let limit = record_limit
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);
    work_order.records.iter().take(limit).collect()
}

fn materialize_run_spec(
    template: &Value,
    record: &SourceUniverseConversionWorkOrderRecord,
    input: &SourceUniverseOperatorInputRecord,
    instrument: &SourceUniverseOperatorInstrumentSpecRecord,
    proof: &SourceProofReport,
    operator_inputs: &SourceUniverseOperatorInputs,
    venue_account_types: &SourceUniverseExecutionPackVenueAccountTypes,
) -> Result<String> {
    let mut value = template.clone();
    set_table_value(
        &mut value,
        "accepted_object",
        Value::try_from(accepted_object_value(record, input))
            .context("serialize accepted object to TOML")?,
    )?;

    let mut source_proof = proof.clone();
    source_proof.source_binding = record.source_binding.clone();
    source_proof.table_family = operator_inputs.table_family.clone();
    source_proof.product_category = record.category.clone();
    source_proof.raw_sample_uri = record.source_uri.clone();
    source_proof.raw_sample_hash = record.selected_object_sha256.clone();
    source_proof.acceptance_scope = Some(AcceptanceScope {
        planned_objects: 1,
        completed_objects: 1,
        failed_objects: 0,
        skipped_objects: 0,
        accepted_bytes: record.selected_object_bytes,
        selector_scope_violations: 0,
    });
    let accepted_by = source_proof
        .accepted_by
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("source proof {} missing accepted_by", proof.source_proof_id))?
        .clone();
    let accepted_at_utc = source_proof
        .accepted_at
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("source proof {} missing accepted_at", proof.source_proof_id))?
        .clone();
    set_table_value(
        &mut value,
        "source_proof",
        Value::try_from(&source_proof).context("serialize source proof to TOML")?,
    )?;
    let root = value
        .as_table_mut()
        .with_context(|| "run-spec template root must be a TOML table")?;
    root.insert("accepted_by".to_string(), Value::String(accepted_by));
    root.insert(
        "accepted_at_utc".to_string(),
        Value::String(accepted_at_utc),
    );

    set_table_value(
        &mut value,
        "instrument_spec",
        Value::try_from(&instrument.instrument_spec)
            .context("serialize instrument spec to TOML")?,
    )?;
    set_table_value(
        &mut value,
        "identity",
        Value::try_from(CanonicalInstrumentIdentity {
            instrument_id: record.symbol.clone(),
            venue_symbol: record.symbol.clone(),
            nt_instrument_id: instrument.nt_instrument_id.clone(),
        })
        .context("serialize instrument identity to TOML")?,
    )?;
    let converter_csv = input
        .converter_csv
        .clone()
        .with_context(|| format!("missing converter CSV mapping for {}", record.work_item_id))?;
    set_table_value(
        &mut value,
        "converter",
        Value::try_from(ConverterConfig {
            identity: record.converter_identity.clone(),
            version: record.converter_version.clone(),
            raw_payload: RawPayloadConfig {
                container: record.raw_payload_container,
                max_object_bytes: record.selected_object_bytes,
                max_decoded_bytes: record.max_decoded_bytes,
                zip_member: record.zip_member.clone(),
                max_member_bytes: None,
                member_suffix: None,
            },
            csv: converter_csv,
            bars: None,
            paged_json_bars: None,
            jsonl_bars: None,
            deltas: None,
            quotes: None,
            seeded_l2_quotes: None,
        })
        .context("serialize converter config to TOML")?,
    )?;

    let manifest = required_table_mut(&mut value, &["manifest"])?;
    manifest.insert(
        "run_id".to_string(),
        Value::String(record.operator_run_id.clone()),
    );
    manifest.insert(
        "venue_binding_key".to_string(),
        Value::String(record.source_binding.clone()),
    );
    manifest.insert(
        "source_proof_id".to_string(),
        Value::String(record.source_proof_id.clone()),
    );
    manifest.insert(
        "source_proof_version".to_string(),
        Value::Integer(i64::from(record.source_proof_version)),
    );
    let output_prefix = resolved_output_prefix(manifest, &record.output_prefix)?;
    manifest.insert("output_prefix".to_string(), Value::String(output_prefix));
    if let Some(venue) = manifest.get_mut("venue").and_then(Value::as_table_mut) {
        venue.insert(
            "nt_venue".to_string(),
            Value::String(operator_inputs.nt_venue.clone()),
        );
    }
    patch_venue_account_type(manifest, &instrument.instrument_spec, venue_account_types)?;
    patch_catalog_inputs(manifest, &instrument.nt_instrument_id)?;
    patch_strategy_bar_type(manifest, &instrument.nt_instrument_id)?;

    let materialized =
        toml::to_string_pretty(&value).context("serialize materialized run spec TOML")?;
    let _: RunSpec = toml::from_str(&materialized)
        .context("materialized source-universe run spec does not deserialize")?;
    Ok(materialized)
}

fn accepted_object_value(
    record: &SourceUniverseConversionWorkOrderRecord,
    input: &SourceUniverseOperatorInputRecord,
) -> serde_json::Value {
    let mut object = serde_json::json!({
        "s3_uri": record.source_uri,
        "source_url": record.source_url,
        "sha256": record.selected_object_sha256,
        "bytes": record.selected_object_bytes,
        "archive_date": record.archive_date
    });
    if let Some(schema_columns) = &input.schema_columns {
        object["schema_columns"] = serde_json::Value::Array(
            schema_columns
                .iter()
                .map(|column| serde_json::Value::String(column.clone()))
                .collect(),
        );
    }
    object
}

fn patch_catalog_inputs(manifest: &mut toml::Table, nt_instrument_id: &str) -> Result<()> {
    let Some(inputs) = manifest
        .get_mut("catalog_inputs")
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for input in inputs {
        if let Some(table) = input.as_table_mut() {
            table.insert(
                "nt_instrument_id".to_string(),
                Value::String(nt_instrument_id.to_string()),
            );
        }
    }
    Ok(())
}

fn patch_strategy_bar_type(manifest: &mut toml::Table, nt_instrument_id: &str) -> Result<()> {
    let Some(parameters) = manifest
        .get_mut("strategy")
        .and_then(Value::as_table_mut)
        .and_then(|strategy| strategy.get_mut("parameters"))
        .and_then(Value::as_table_mut)
    else {
        return Ok(());
    };
    let Some(existing) = parameters.get("bar_type").and_then(Value::as_str) else {
        return Ok(());
    };
    // NT owns bar-type syntax: parse the template value loud (NT splits from
    // the right, so hyphenated instrument ids stay intact), rebind only the
    // instrument id, and let NT render the result. A malformed template is an
    // error, never a substituted default cadence.
    let template: BarType = existing.parse().map_err(|error| {
        anyhow::anyhow!(
            "run-spec template strategy.parameters.bar_type {existing:?} is not a valid NT bar type: {error}"
        )
    })?;
    let instrument_id: InstrumentId = nt_instrument_id.parse().map_err(|error| {
        anyhow::anyhow!(
            "nt_instrument_id {nt_instrument_id:?} is not a valid NT instrument id: {error}"
        )
    })?;
    let rebound = match template {
        BarType::Standard {
            spec,
            aggregation_source,
            ..
        } => BarType::Standard {
            instrument_id,
            spec,
            aggregation_source,
        },
        BarType::Composite {
            spec,
            aggregation_source,
            composite_step,
            composite_aggregation,
            composite_aggregation_source,
            ..
        } => BarType::Composite {
            instrument_id,
            spec,
            aggregation_source,
            composite_step,
            composite_aggregation,
            composite_aggregation_source,
        },
    };
    parameters.insert("bar_type".to_string(), Value::String(rebound.to_string()));
    Ok(())
}

fn patch_venue_account_type(
    manifest: &mut toml::Table,
    instrument_spec: &CatalogInstrumentSpec,
    venue_account_types: &SourceUniverseExecutionPackVenueAccountTypes,
) -> Result<()> {
    let account_type = venue_account_types.account_type_for(instrument_spec)?;
    let venue = manifest
        .get_mut("venue")
        .and_then(Value::as_table_mut)
        .with_context(|| "run-spec template manifest.venue table is required")?;
    venue.insert(
        "account_type".to_string(),
        Value::String(account_type.to_owned()),
    );
    Ok(())
}

fn resolved_output_prefix(manifest: &toml::Table, output_prefix: &str) -> Result<String> {
    if output_prefix.contains("://") {
        return Ok(output_prefix.to_string());
    }
    let artifact_root = manifest
        .get("artifact_root")
        .and_then(Value::as_str)
        .with_context(|| "run-spec template manifest.artifact_root is required")?;
    let template_output_prefix = manifest
        .get("output_prefix")
        .and_then(Value::as_str)
        .with_context(|| "run-spec template manifest.output_prefix is required")?;
    let prefix_base =
        output_prefix_base_from_template(artifact_root, template_output_prefix).with_context(
            || {
                format!(
                    "template output_prefix {template_output_prefix:?} is not under artifact_root {artifact_root:?}"
                )
            },
        )?;
    Ok(format!(
        "{}/{}",
        prefix_base.trim_end_matches('/'),
        output_prefix.trim_start_matches('/')
    ))
}

fn output_prefix_base_from_template(
    artifact_root: &str,
    template_output_prefix: &str,
) -> Option<String> {
    let artifact_root = artifact_root.trim_end_matches('/');
    let remainder = template_output_prefix
        .strip_prefix(artifact_root)?
        .trim_start_matches('/');
    let first_segment = remainder.split('/').next()?;
    if first_segment.is_empty() {
        None
    } else {
        Some(format!("{artifact_root}/{first_segment}"))
    }
}

fn accepted_tranche_for_record(
    record: &SourceUniverseConversionWorkOrderRecord,
    proof: &SourceProofReport,
    table_family: &str,
    gate: Option<&SourceUniverseObjectGateRecord>,
) -> BackfillAcceptedTrancheManifest {
    BackfillAcceptedTrancheManifest {
        schema_version: BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION.to_string(),
        tranche_id: record.accepted_tranche_id.clone(),
        status: BackfillAcceptedTrancheStatus::Accepted,
        source_proof_scope_report_id: gate
            .map(|gate| gate.source_proof_scope_report_id.clone())
            .unwrap_or_else(|| format!("{}:source-proof-scope", record.accepted_tranche_id)),
        source_proof_scope_report_hash: gate
            .map(|gate| gate.source_proof_hash.clone())
            .unwrap_or_else(|| record.selected_object_sha256.clone()),
        source_proof_id: record.source_proof_id.clone(),
        source_proof_version: record.source_proof_version,
        source_binding: record.source_binding.clone(),
        table_family: table_family.to_string(),
        source_usage_scope: proof.usage_scope,
        parent_manifest_id: gate
            .map(|gate| gate.category_manifest_id.clone())
            .unwrap_or_else(|| record.source_binding.clone()),
        object_level_tranche_required: true,
        object_count: 1,
        accepted_bytes: record.selected_object_bytes,
        objects: vec![BackfillAcceptedTrancheObject {
            s3_uri: record.source_uri.clone(),
            source_url: record.source_url.clone(),
            sha256: record.selected_object_sha256.clone(),
            bytes: record.selected_object_bytes,
            archive_date: record.archive_date.clone(),
            source_row_groups: Vec::new(),
            predicate_ref: None,
        }],
        blocking_issues: Vec::new(),
    }
}

/// A source proof read, sha-verified, and parsed once at the single consume
/// boundary, kept alongside the gate artifact ref it came from. Bundling the
/// validated report with its ref means the artifact-ref selection loop can reuse
/// this proven read instead of re-opening (and re-parsing) the same file, so a
/// later read/parse failure can never be silently downgraded to "not a match".
struct ValidatedSourceProof {
    report: SourceProofReport,
    artifact_ref: ReferenceArtifactPin,
}

fn source_proofs_by_id(
    base_dir: &Path,
    gates: &SourceUniverseObjectGateMaterialization,
) -> Result<BTreeMap<String, ValidatedSourceProof>> {
    let mut proofs = BTreeMap::new();
    for artifact in gates
        .artifact_refs
        .iter()
        .filter(|artifact| artifact.role == "source_proof")
    {
        let (_, _, report): (PathBuf, String, SourceProofReport) = read_json_artifact(
            base_dir,
            &artifact.path,
            Some(artifact.sha256.as_str()),
            "source_proof",
        )?;
        ensure!(
            proofs
                .insert(
                    report.source_proof_id.clone(),
                    ValidatedSourceProof {
                        report,
                        artifact_ref: artifact.clone(),
                    },
                )
                .is_none(),
            "duplicate source proof id in source-universe object gates"
        );
    }
    ensure!(
        !proofs.is_empty(),
        "source-universe object gates do not reference any source proofs"
    );
    Ok(proofs)
}

fn operator_inputs_by_work_item(
    inputs: &SourceUniverseOperatorInputs,
) -> Result<BTreeMap<String, &SourceUniverseOperatorInputRecord>> {
    let mut records = BTreeMap::new();
    for record in &inputs.records {
        ensure!(
            records
                .insert(record.work_item_id.clone(), record)
                .is_none(),
            "duplicate operator input work item {}",
            record.work_item_id
        );
    }
    Ok(records)
}

fn instruments_by_key(
    inputs: &SourceUniverseOperatorInputs,
) -> Result<BTreeMap<String, &SourceUniverseOperatorInstrumentSpecRecord>> {
    let mut records = BTreeMap::new();
    for record in &inputs.instrument_specs {
        ensure!(
            records
                .insert(record.instrument_key.clone(), record)
                .is_none(),
            "duplicate source-universe instrument spec {}",
            record.instrument_key
        );
    }
    Ok(records)
}

fn gates_by_work_item(
    gates: &SourceUniverseObjectGateMaterialization,
) -> Result<BTreeMap<String, &SourceUniverseObjectGateRecord>> {
    let mut records = BTreeMap::new();
    for record in &gates.records {
        ensure!(
            records
                .insert(record.work_item_id.clone(), record)
                .is_none(),
            "duplicate source-universe object gate {}",
            record.work_item_id
        );
    }
    Ok(records)
}

fn work_order_artifact_ref<'a>(
    work_order: &'a SourceUniverseConversionWorkOrder,
    role: &str,
) -> Result<&'a ReferenceArtifactPin> {
    work_order
        .artifact_refs
        .iter()
        .find(|artifact| artifact.role == role)
        .with_context(|| format!("source-universe work order missing artifact ref {role}"))
}

fn operator_inputs_artifact_ref<'a>(
    inputs: &'a SourceUniverseOperatorInputs,
    role: &str,
) -> Result<&'a ReferenceArtifactPin> {
    inputs
        .artifact_refs
        .iter()
        .find(|artifact| artifact.role == role)
        .with_context(|| format!("source-universe operator inputs missing artifact ref {role}"))
}

fn read_json_artifact<T>(
    base_dir: &Path,
    path: &Path,
    expected_sha256: Option<&str>,
    role: &str,
) -> Result<(PathBuf, String, T)>
where
    T: for<'de> Deserialize<'de>,
{
    let resolved = resolve_existing_path(base_dir, path);
    let bytes = fs::read(&resolved)
        .with_context(|| format!("read {role} artifact {}", resolved.display()))?;
    let actual_sha256 = sha256_hex(&bytes);
    if let Some(expected_sha256) = expected_sha256 {
        ensure!(
            actual_sha256 == expected_sha256,
            "{role} artifact {} hash mismatch: expected {}, got {}",
            resolved.display(),
            expected_sha256,
            actual_sha256
        );
    }
    let value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {role} artifact {}", resolved.display()))?;
    Ok((resolved, actual_sha256, value))
}

fn set_table_value(value: &mut Value, key: &'static str, new_value: Value) -> Result<()> {
    let table = value
        .as_table_mut()
        .with_context(|| "run-spec template root must be a TOML table")?;
    if !new_value.is_table() {
        bail!("replacement value for {key} must be a TOML table");
    }
    table.insert(key.to_string(), new_value);
    Ok(())
}

fn required_table_mut<'a>(
    value: &'a mut Value,
    path: &[&'static str],
) -> Result<&'a mut toml::Table> {
    let mut current = value;
    for key in path {
        current = current
            .get_mut(*key)
            .with_context(|| format!("run-spec template missing TOML table {}", path.join(".")))?;
    }
    current.as_table_mut().with_context(|| {
        format!(
            "run-spec template value {} is not a TOML table",
            path.join(".")
        )
    })
}

fn write_bytes_if_clean(path: &Path, bytes: &[u8], overwrite_existing: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create artifact directory {}", parent.display()))?;
    }
    if path.exists() {
        let existing =
            fs::read(path).with_context(|| format!("read existing artifact {}", path.display()))?;
        if existing != bytes {
            ensure!(
                overwrite_existing,
                "dirty artifact {}: existing file content differs",
                path.display()
            );
            atomic_write(path, bytes)
                .with_context(|| format!("write artifact {}", path.display()))?;
        }
    } else {
        atomic_write(path, bytes).with_context(|| format!("write artifact {}", path.display()))?;
    }
    Ok(())
}
fn slug(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            slug.push(character);
        } else {
            slug.push('-');
        }
    }
    if slug.is_empty() {
        "run".to_string()
    } else {
        slug
    }
}
