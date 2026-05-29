use std::{collections::BTreeMap, future::Future, pin::Pin, str::FromStr};

use nautilus_model::{
    enums::LiquiditySide,
    instruments::{Instrument, InstrumentAny},
    orderbook::OrderBook,
};
use nautilus_network::retry::RetryConfig;
use nautilus_polymarket::{
    common::consts::LOT_SIZE_SCALE,
    execution::parse::{compute_commission, instrument_taker_fee},
    http::{clob::PolymarketClobPublicClient, gamma::PolymarketGammaHttpClient},
    providers::PolymarketInstrumentProvider,
};
use rust_decimal::{Decimal, prelude::FromPrimitive};
use serde::Serialize;

use crate::{
    bolt_v3_canary_proof_policy::{
        CanaryProofCandidate, CanaryProofInstrumentConstraints, CanaryProofOrderSide,
        CanaryProofPolicyInput, CanaryProofSizingMode, CanaryProofSourcePacket,
        build_canary_proof_candidate_source_artifact, build_canary_proof_order_intent_artifact,
    },
    bolt_v3_market_families::{self, MarketSelectionTarget},
    bolt_v3_operator_artifacts::{
        BoltV3OperatorArtifactError, CanaryProofArtifactsWritten, EntryDecisionSourceBookSideInput,
        EntryDecisionSourceInputRequest, EntryDecisionSourceInputsWritten,
        EntryDecisionSourceMarketInputs, EntryDecisionSourceProofFileRequest,
        build_entry_readiness_gate_session_from_source_proof_files, selected_entry_decision_market,
        validate_entry_decision_source_proof_files,
        write_entry_decision_source_inputs_from_source_files, write_json_artifact_create_new,
    },
    bolt_v3_providers::{CanaryProofArtifactsProviderContext, EntryDecisionSourceProviderContext},
};

use super::{PolymarketDataConfig, PolymarketExecutionConfig};

const ENTRY_DECISION_UP_BOOK_LABEL: &str = "up";
const ENTRY_DECISION_DOWN_BOOK_LABEL: &str = "down";
const ENTRY_DECISION_FEE_BPS_SCALE: f64 = 10_000.0;
const ENTRY_DECISION_FEE_PROBE_SIZE: i64 = 1;
const ENTRY_DECISION_RETRY_INITIAL_ATTEMPT_COUNT: u64 = 1;
const ENTRY_DECISION_GATE_PROVENANCE_RECORD_KIND: &str =
    "bolt_v3.polymarket_entry_decision_gate_provenance.v1";
const ENTRY_DECISION_GATE_PROVENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct PolymarketEntryDecisionGateProvenancePayload<'a> {
    schema_version: u32,
    record_kind: &'static str,
    provider_id: &'a str,
    provider_kind: &'a str,
    selected_market_key: &'a str,
    decision_source_sha256: &'a str,
    instrument_source_sha256: &'a str,
}

pub fn polymarket_entry_decision_gate_provenance_payload(
    provider_id: &str,
    provider_kind: &str,
    selected_market_key: &str,
    decision_source_sha256: &str,
    instrument_source_sha256: &str,
) -> serde_json::Value {
    serde_json::json!(PolymarketEntryDecisionGateProvenancePayload {
        schema_version: ENTRY_DECISION_GATE_PROVENANCE_SCHEMA_VERSION,
        record_kind: ENTRY_DECISION_GATE_PROVENANCE_RECORD_KIND,
        provider_id,
        provider_kind,
        selected_market_key,
        decision_source_sha256,
        instrument_source_sha256,
    })
}

pub fn collect_entry_decision_source_inputs(
    context: EntryDecisionSourceProviderContext<'_>,
) -> Pin<
    Box<
        dyn Future<Output = Result<EntryDecisionSourceInputsWritten, BoltV3OperatorArtifactError>>
            + '_,
    >,
> {
    Box::pin(async move { collect_entry_decision_source_inputs_inner(context).await })
}

async fn collect_entry_decision_source_inputs_inner(
    context: EntryDecisionSourceProviderContext<'_>,
) -> Result<EntryDecisionSourceInputsWritten, BoltV3OperatorArtifactError> {
    let proof_validation = validate_entry_decision_source_proof_files(
        context.loaded,
        context.strategy_instance_id,
        EntryDecisionSourceProofFileRequest {
            price_to_beat_source_path: context.request.price_to_beat_source_path,
            max_price_to_beat_source_bytes: context.request.max_price_to_beat_source_bytes,
            reference_quote_source_path: context.request.reference_quote_source_path,
            max_reference_quote_source_bytes: context.request.max_reference_quote_source_bytes,
            realized_volatility_source_path: context.request.realized_volatility_source_path,
            max_realized_volatility_source_bytes: context
                .request
                .max_realized_volatility_source_bytes,
        },
    )?;
    let source_config =
        polymarket_source_config_for_strategy(context.loaded, context.strategy_instance_id)?;
    let mut instruments = load_polymarket_instruments_for_entry_decision_source(
        context.loaded,
        context.strategy_instance_id,
        &source_config,
        proof_validation.market_selection_timestamp_ms,
    )
    .await?;
    instruments.sort_by_key(|instrument| instrument.id().to_string());
    let selected = selected_entry_decision_market(
        context.loaded,
        context.strategy_instance_id,
        &instruments,
        proof_validation.market_selection_timestamp_ms,
    )?;
    let clob_client = PolymarketClobPublicClient::new(
        Some(source_config.data.base_url_http.clone()),
        source_config.data.http_timeout_secs,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("failed to create CLOB public client: {source}"),
        },
    )?;
    let up_instrument = instrument_for_id(&instruments, &selected.up_instrument_id.to_string())?;
    let down_instrument =
        instrument_for_id(&instruments, &selected.down_instrument_id.to_string())?;
    let up_token_id = up_instrument.raw_symbol().to_string();
    let down_token_id = down_instrument.raw_symbol().to_string();
    let up_book = clob_client
        .request_book_snapshot(
            selected.up_instrument_id,
            up_token_id.as_str(),
            up_instrument.price_precision(),
            up_instrument.size_precision(),
        )
        .await
        .map_err(
            |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("failed to fetch up book snapshot: {source}"),
            },
        )?;
    let down_book = clob_client
        .request_book_snapshot(
            selected.down_instrument_id,
            down_token_id.as_str(),
            down_instrument.price_precision(),
            down_instrument.size_precision(),
        )
        .await
        .map_err(
            |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("failed to fetch down book snapshot: {source}"),
            },
        )?;
    let up_book_input = book_side_input_from_order_book(&up_book, ENTRY_DECISION_UP_BOOK_LABEL)?;
    let down_book_input =
        book_side_input_from_order_book(&down_book, ENTRY_DECISION_DOWN_BOOK_LABEL)?;
    let fee_bps_by_instrument_id = entry_decision_fee_bps_by_instrument_id(
        up_instrument,
        down_instrument,
        up_book_input.best_ask,
        down_book_input.best_ask,
    )?;

    write_entry_decision_source_inputs_from_source_files(
        context.loaded,
        context.strategy_instance_id,
        EntryDecisionSourceInputRequest {
            price_to_beat_source_path: context.request.price_to_beat_source_path,
            max_price_to_beat_source_bytes: context.request.max_price_to_beat_source_bytes,
            reference_quote_source_path: context.request.reference_quote_source_path,
            max_reference_quote_source_bytes: context.request.max_reference_quote_source_bytes,
            realized_volatility_source_path: context.request.realized_volatility_source_path,
            max_realized_volatility_source_bytes: context
                .request
                .max_realized_volatility_source_bytes,
            market_inputs: EntryDecisionSourceMarketInputs {
                instruments: &instruments,
                up_book: up_book_input,
                down_book: down_book_input,
                fee_bps_by_instrument_id,
            },
            decision_source_output_path: context.request.decision_source_output_path,
            instrument_source_output_path: context.request.instrument_source_output_path,
            fee_rate_source_output_path: context.request.fee_rate_source_output_path,
        },
    )
}

pub fn collect_canary_proof_artifacts(
    context: CanaryProofArtifactsProviderContext<'_>,
) -> Pin<
    Box<dyn Future<Output = Result<CanaryProofArtifactsWritten, BoltV3OperatorArtifactError>> + '_>,
> {
    Box::pin(async move { collect_canary_proof_artifacts_inner(context).await })
}

async fn collect_canary_proof_artifacts_inner(
    context: CanaryProofArtifactsProviderContext<'_>,
) -> Result<CanaryProofArtifactsWritten, BoltV3OperatorArtifactError> {
    let proof_request = EntryDecisionSourceProofFileRequest {
        price_to_beat_source_path: context.request.price_to_beat_source_path,
        max_price_to_beat_source_bytes: context.request.max_price_to_beat_source_bytes,
        reference_quote_source_path: context.request.reference_quote_source_path,
        max_reference_quote_source_bytes: context.request.max_reference_quote_source_bytes,
        realized_volatility_source_path: context.request.realized_volatility_source_path,
        max_realized_volatility_source_bytes: context.request.max_realized_volatility_source_bytes,
    };
    let proof_validation = validate_entry_decision_source_proof_files(
        context.loaded,
        context.strategy_instance_id,
        proof_request,
    )?;
    let source_config =
        polymarket_source_config_for_strategy(context.loaded, context.strategy_instance_id)?;
    let mut instruments = load_polymarket_instruments_for_entry_decision_source(
        context.loaded,
        context.strategy_instance_id,
        &source_config,
        proof_validation.market_selection_timestamp_ms,
    )
    .await?;
    instruments.sort_by_key(|instrument| instrument.id().to_string());
    let selected = selected_entry_decision_market(
        context.loaded,
        context.strategy_instance_id,
        &instruments,
        proof_validation.market_selection_timestamp_ms,
    )?;
    let gate_session = build_entry_readiness_gate_session_from_source_proof_files(
        context.loaded,
        context.strategy_instance_id,
        &selected,
        proof_request,
    )?;
    let gate_session_written =
        write_json_artifact_create_new(context.request.gate_session_output_path, &gate_session)?;

    let clob_client = PolymarketClobPublicClient::new(
        Some(source_config.data.base_url_http.clone()),
        source_config.data.http_timeout_secs,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("failed to create CLOB public client: {source}"),
        },
    )?;
    let up_instrument = instrument_for_id(&instruments, &selected.up_instrument_id.to_string())?;
    let down_instrument =
        instrument_for_id(&instruments, &selected.down_instrument_id.to_string())?;
    let up_token_id = up_instrument.raw_symbol().to_string();
    let down_token_id = down_instrument.raw_symbol().to_string();
    let up_book = clob_client
        .request_book_snapshot(
            selected.up_instrument_id,
            up_token_id.as_str(),
            up_instrument.price_precision(),
            up_instrument.size_precision(),
        )
        .await
        .map_err(
            |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("failed to fetch up book snapshot: {source}"),
            },
        )?;
    let down_book = clob_client
        .request_book_snapshot(
            selected.down_instrument_id,
            down_token_id.as_str(),
            down_instrument.price_precision(),
            down_instrument.size_precision(),
        )
        .await
        .map_err(
            |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("failed to fetch down book snapshot: {source}"),
            },
        )?;
    let proof_policy = live_canary_proof_policy_input(
        context.loaded,
        context.strategy_instance_id,
        gate_session.session_hash.as_str(),
    )?;
    let mut candidates = Vec::new();
    candidates.extend(canary_proof_candidate_from_best_ask(
        up_instrument,
        &up_book,
        &proof_policy,
    )?);
    candidates.extend(canary_proof_candidate_from_best_ask(
        down_instrument,
        &down_book,
        &proof_policy,
    )?);
    if candidates.is_empty() {
        return Err(entry_decision_source_invalid(
            "canary proof source found no orderable ask candidate",
        ));
    }
    let source_packet = CanaryProofSourcePacket {
        current_source_ref: gate_session.session_hash.clone(),
    };
    let candidate_source = build_canary_proof_candidate_source_artifact(&source_packet, candidates)
        .map_err(|source| {
            entry_decision_source_invalid(format!(
                "canary proof candidate source rejected: {source:?}"
            ))
        })?;
    let order_input = CanaryProofPolicyInput {
        candidates: candidate_source.candidates.clone(),
        ..proof_policy
    };
    let order_intent = build_canary_proof_order_intent_artifact(&candidate_source, &order_input)
        .map_err(|source| {
            entry_decision_source_invalid(format!("canary proof order intent rejected: {source:?}"))
        })?;
    let candidate_source_written = write_json_artifact_create_new(
        context.request.candidate_source_output_path,
        &candidate_source,
    )?;
    let order_intent_written =
        write_json_artifact_create_new(context.request.order_intent_output_path, &order_intent)?;
    Ok(CanaryProofArtifactsWritten {
        gate_session: gate_session_written,
        candidate_source: candidate_source_written,
        order_intent: order_intent_written,
    })
}

struct PolymarketSourceConfig {
    data: PolymarketDataConfig,
    execution: PolymarketExecutionConfig,
}

fn polymarket_source_config_for_strategy(
    loaded: &crate::bolt_v3_config::LoadedBoltV3Config,
    strategy_instance_id: &str,
) -> Result<PolymarketSourceConfig, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let client_key = strategy.config.execution_client_id.as_str();
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("execution client `{client_key}` is not loaded"),
        }
    })?;
    let data = client.data.as_ref().ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("execution client `{client_key}` has no data block"),
        }
    })?;
    let data = data.clone().try_into().map_err(|source: toml::de::Error| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("failed to parse data source config: {source}"),
        }
    })?;
    let execution = client.execution.as_ref().ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("execution client `{client_key}` has no execution block"),
        }
    })?;
    let execution = execution
        .clone()
        .try_into()
        .map_err(|source: toml::de::Error| {
            BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("failed to parse execution source config: {source}"),
            }
        })?;
    Ok(PolymarketSourceConfig { data, execution })
}

async fn load_polymarket_instruments_for_entry_decision_source(
    loaded: &crate::bolt_v3_config::LoadedBoltV3Config,
    strategy_instance_id: &str,
    source_config: &PolymarketSourceConfig,
    now_milliseconds: u64,
) -> Result<Vec<InstrumentAny>, BoltV3OperatorArtifactError> {
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == strategy_instance_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: format!("strategy instance `{strategy_instance_id}` is not loaded"),
            },
        )?;
    let target =
        bolt_v3_market_families::target_runtime_fields_from_target(&strategy.config.target)
            .map_err(|error| {
                BoltV3OperatorArtifactError::MarketSelection(anyhow::anyhow!(error))
            })?;
    let selection_target = MarketSelectionTarget {
        family_key: &target.rotating_market_family,
        underlying_asset: &target.underlying_asset,
        cadence_seconds: target.cadence_seconds,
        cadence_slug_token: &target.cadence_slug_token,
    };
    let candidate_windows =
        bolt_v3_market_families::market_selection_candidate_windows_from_target(
            selection_target,
            now_milliseconds,
        )
        .map_err(|error| BoltV3OperatorArtifactError::MarketSelection(anyhow::anyhow!(error)))?;
    let slugs: Vec<String> = candidate_windows
        .into_iter()
        .map(|candidate| candidate.market_slug)
        .collect();
    if slugs.is_empty() {
        return Err(
            BoltV3OperatorArtifactError::MarketSelectionPrerequisiteUnproven {
                prerequisite: "entry decision source requires configured market slugs",
            },
        );
    }
    let gamma_client = PolymarketGammaHttpClient::new(
        Some(source_config.data.base_url_gamma.clone()),
        source_config.data.http_timeout_secs,
        retry_config_from_execution_config(&source_config.execution)?,
    )
    .map_err(
        |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("failed to create Gamma public client: {source}"),
        },
    )?;
    let mut provider = PolymarketInstrumentProvider::new(gamma_client);
    provider.load_by_slugs(slugs).await.map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("failed to load instruments by configured slugs: {source}"),
        }
    })?;
    let mut instruments: Vec<InstrumentAny> = provider.build_token_map().into_values().collect();
    instruments.sort_by_key(|instrument| instrument.id().to_string());
    Ok(instruments)
}

fn retry_config_from_execution_config(
    execution: &PolymarketExecutionConfig,
) -> Result<RetryConfig, BoltV3OperatorArtifactError> {
    let max_retries = u32::try_from(execution.max_retries).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("failed to convert configured max retries: {source}"),
        }
    })?;
    let operation_timeout_ms =
        u64::try_from(std::time::Duration::from_secs(execution.http_timeout_secs).as_millis())
            .map_err(
                |source| BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                    message: format!("failed to convert configured HTTP timeout: {source}"),
                },
            )?;
    if std::time::Duration::from_millis(execution.retry_delay_initial_ms).is_zero()
        || execution.retry_delay_initial_ms > execution.retry_delay_max_ms
    {
        return Err(entry_decision_source_invalid(
            "entry decision source retry_delay_initial_ms must be positive and <= retry_delay_max_ms",
        ));
    }
    let backoff_factor =
        execution.retry_delay_max_ms as f64 / execution.retry_delay_initial_ms as f64;
    let configured_attempt_count =
        u64::from(max_retries).saturating_add(ENTRY_DECISION_RETRY_INITIAL_ATTEMPT_COUNT);
    let max_elapsed_ms = operation_timeout_ms
        .saturating_mul(configured_attempt_count)
        .saturating_add(
            execution
                .retry_delay_max_ms
                .saturating_mul(u64::from(max_retries)),
        );
    Ok(RetryConfig {
        max_retries,
        initial_delay_ms: execution.retry_delay_initial_ms,
        max_delay_ms: execution.retry_delay_max_ms,
        backoff_factor,
        jitter_ms: execution.retry_delay_initial_ms,
        operation_timeout_ms: Some(operation_timeout_ms),
        immediate_first: std::time::Duration::from_millis(execution.retry_delay_initial_ms)
            .is_zero(),
        max_elapsed_ms: Some(max_elapsed_ms),
    })
}

fn instrument_for_id<'a>(
    instruments: &'a [InstrumentAny],
    instrument_id: &str,
) -> Result<&'a InstrumentAny, BoltV3OperatorArtifactError> {
    instruments
        .iter()
        .find(|instrument| instrument.id().to_string() == instrument_id)
        .ok_or_else(
            || BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
                message: "selected instrument is missing from instrument source".to_string(),
            },
        )
}

fn canary_proof_candidate_from_best_ask(
    instrument: &InstrumentAny,
    book: &OrderBook,
    policy: &CanaryProofPolicyInput,
) -> Result<Option<CanaryProofCandidate>, BoltV3OperatorArtifactError> {
    let Some(best_ask) = book.best_ask_price() else {
        return Ok(None);
    };
    let Some(ask_quantity) = book.best_ask_size() else {
        return Ok(None);
    };
    let sizing_price = decimal_from_display(best_ask, "canary proof best ask")?;
    let available_quantity = decimal_from_display(ask_quantity, "canary proof ask quantity")?;
    if sizing_price <= Decimal::ZERO || available_quantity <= Decimal::ZERO {
        return Ok(None);
    }
    let instrument_quantity_step =
        decimal_from_display(instrument.size_increment(), "canary proof quantity step")?;
    let clob_amount_step = polymarket_clob_lot_size_step();
    let quantity_step = if instrument_quantity_step > clob_amount_step {
        instrument_quantity_step
    } else {
        clob_amount_step
    };
    let min_quantity = instrument
        .min_quantity()
        .map(|quantity| decimal_from_display(quantity, "canary proof minimum quantity"))
        .transpose()?;
    Ok(Some(CanaryProofCandidate {
        strategy_instance_id: policy.strategy_instance_id.clone(),
        execution_client_id: policy.execution_client_id.clone(),
        instrument_id: instrument.id().to_string(),
        order_side: CanaryProofOrderSide::Buy,
        candidate_score: -sizing_price,
        source_refs: vec![policy.current_source_ref.clone()],
        sizing_price,
        constraints: CanaryProofInstrumentConstraints {
            sizing_mode: CanaryProofSizingMode::BaseQuantity,
            quantity_step,
            notional_step: Some(clob_amount_step),
            min_quantity,
            min_notional: None,
        },
    }))
}

fn polymarket_clob_lot_size_step() -> Decimal {
    Decimal::new(1, LOT_SIZE_SCALE)
}

fn live_canary_proof_policy_input(
    loaded: &crate::bolt_v3_config::LoadedBoltV3Config,
    strategy_instance_id: &str,
    current_source_ref: &str,
) -> Result<CanaryProofPolicyInput, BoltV3OperatorArtifactError> {
    let live_canary = loaded.root.live_canary.as_ref().ok_or_else(|| {
        entry_decision_source_invalid("live_canary block is required for canary proof artifacts")
    })?;
    let policy = live_canary.proof_policy.as_ref().ok_or_else(|| {
        entry_decision_source_invalid(
            "live_canary.proof_policy block is required for canary proof artifacts",
        )
    })?;
    if !policy.enabled {
        return Err(entry_decision_source_invalid(
            "live_canary.proof_policy must be enabled for canary proof artifacts",
        ));
    }
    if policy.strategy_instance_id != strategy_instance_id {
        return Err(entry_decision_source_invalid(
            "live_canary.proof_policy.strategy_instance_id does not match requested strategy",
        ));
    }
    Ok(CanaryProofPolicyInput {
        strategy_instance_id: policy.strategy_instance_id.clone(),
        execution_client_id: policy.execution_client_id.clone(),
        proof_claim: policy.proof_claim.clone(),
        proof_notional: decimal_from_str(policy.proof_notional.as_str(), "canary proof notional")?,
        max_notional_per_order: decimal_from_str(
            live_canary.max_notional_per_order.as_str(),
            "canary proof max notional per order",
        )?,
        allow_negative_expected_ev: policy.allow_negative_expected_ev,
        source_ready: true,
        current_source_ref: current_source_ref.to_string(),
        candidates: Vec::new(),
    })
}

fn decimal_from_display(
    value: impl ToString,
    field: &'static str,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    decimal_from_str(value.to_string().as_str(), field)
}

fn decimal_from_str(
    value: &str,
    field: &'static str,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    Decimal::from_str(value.trim()).map_err(|source| {
        entry_decision_source_invalid(format!("{field} is not decimal: {source}"))
    })
}

fn entry_decision_fee_bps_by_instrument_id(
    up_instrument: &InstrumentAny,
    down_instrument: &InstrumentAny,
    up_entry_price: f64,
    down_entry_price: f64,
) -> Result<BTreeMap<String, f64>, BoltV3OperatorArtifactError> {
    Ok(BTreeMap::from([
        (
            up_instrument.id().to_string(),
            effective_taker_fee_bps_from_nt(up_instrument, up_entry_price)?,
        ),
        (
            down_instrument.id().to_string(),
            effective_taker_fee_bps_from_nt(down_instrument, down_entry_price)?,
        ),
    ]))
}

fn effective_taker_fee_bps_from_nt(
    instrument: &InstrumentAny,
    entry_price: f64,
) -> Result<f64, BoltV3OperatorArtifactError> {
    let price = Decimal::from_f64(entry_price).ok_or_else(|| {
        entry_decision_source_invalid("entry decision source fee price is invalid")
    })?;
    if price <= Decimal::ZERO || price >= Decimal::ONE {
        return Err(entry_decision_source_invalid(
            "entry decision source fee price is invalid",
        ));
    }
    let commission = compute_commission(
        instrument_taker_fee(instrument),
        Decimal::from(ENTRY_DECISION_FEE_PROBE_SIZE),
        price,
        LiquiditySide::Taker,
    );
    let fee_bps = commission / entry_price * ENTRY_DECISION_FEE_BPS_SCALE;
    if !commission.is_finite() || commission.is_sign_negative() || !fee_bps.is_finite() {
        return Err(entry_decision_source_invalid(
            "entry decision source fee bps is invalid",
        ));
    }
    Ok(fee_bps)
}

fn book_side_input_from_order_book(
    book: &OrderBook,
    label: &'static str,
) -> Result<EntryDecisionSourceBookSideInput, BoltV3OperatorArtifactError> {
    let best_bid = book.best_bid_price().ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("entry decision source {label} book is missing best bid"),
        }
    })?;
    let bid_quantity = book.best_bid_size().ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("entry decision source {label} book is missing bid quantity"),
        }
    })?;
    let best_ask = book.best_ask_price().ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("entry decision source {label} book is missing best ask"),
        }
    })?;
    let ask_quantity = book.best_ask_size().ok_or_else(|| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
            message: format!("entry decision source {label} book is missing ask quantity"),
        }
    })?;
    let bid_quantity = bid_quantity.as_f64();
    let ask_quantity = ask_quantity.as_f64();
    Ok(EntryDecisionSourceBookSideInput {
        best_bid: best_bid.as_f64(),
        bid_quantity,
        best_ask: best_ask.as_f64(),
        ask_quantity,
        liquidity_available: bid_quantity + ask_quantity,
    })
}

fn entry_decision_source_invalid(message: impl Into<String>) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use nautilus_model::{
        data::BookOrder,
        enums::{BookType, OrderSide},
        identifiers::{AccountId, InstrumentId},
        orderbook::OrderBook,
        types::{Price, Quantity},
    };
    use nautilus_network::websocket::TransportBackend;

    use super::*;
    use crate::bolt_v3_providers::polymarket::PolymarketSignatureType;

    #[test]
    fn retry_config_rejects_zero_initial_delay() {
        let execution = execution_config_with_retry_delays(0, 2_000);

        let error = retry_config_from_execution_config(&execution)
            .expect_err("zero initial retry delay must fail closed");

        assert!(format!("{error}").contains("retry_delay_initial_ms"));
    }

    #[test]
    fn book_side_input_reports_total_top_of_book_liquidity() {
        let mut book = OrderBook::new(
            InstrumentId::from("0xentry-source-book.POLYMARKET"),
            BookType::L2_MBP,
        );
        book.add(
            BookOrder::new(OrderSide::Buy, Price::from("0.50"), Quantity::from("25"), 1),
            0,
            1,
            1.into(),
        );
        book.add(
            BookOrder::new(
                OrderSide::Sell,
                Price::from("0.52"),
                Quantity::from("100"),
                2,
            ),
            0,
            2,
            2.into(),
        );

        let input = book_side_input_from_order_book(&book, "test")
            .expect("two-sided top of book should produce source input");

        assert_eq!(input.bid_quantity, 25.0);
        assert_eq!(input.ask_quantity, 100.0);
        assert_eq!(input.liquidity_available, 125.0);
    }

    fn execution_config_with_retry_delays(
        retry_delay_initial_ms: u64,
        retry_delay_max_ms: u64,
    ) -> PolymarketExecutionConfig {
        PolymarketExecutionConfig {
            account_id: AccountId::from("POLYMARKET-TEST"),
            signature_type: PolymarketSignatureType::PolyProxy,
            funder: Some("0x1111111111111111111111111111111111111111".to_string()),
            base_url_http: "https://clob.polymarket.test".to_string(),
            base_url_ws: "wss://ws-subscriptions-clob.polymarket.test/ws/user".to_string(),
            base_url_data_api: "https://data-api.polymarket.test".to_string(),
            http_timeout_secs: 60,
            max_retries: 3,
            retry_delay_initial_ms,
            retry_delay_max_ms,
            ack_timeout_secs: 5,
            fee_cache_ttl_secs: 300,
            transport_backend: TransportBackend::Sockudo,
            on_chain_collateral: None,
        }
    }
}
