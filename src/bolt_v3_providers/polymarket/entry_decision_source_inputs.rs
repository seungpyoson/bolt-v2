use std::{collections::BTreeMap, future::Future, pin::Pin};

use nautilus_model::{
    instruments::{Instrument, InstrumentAny},
    orderbook::OrderBook,
};
use nautilus_network::retry::RetryConfig;
use nautilus_polymarket::{
    http::{clob::PolymarketClobPublicClient, gamma::PolymarketGammaHttpClient},
    providers::PolymarketInstrumentProvider,
};
use serde::Deserialize;

use crate::{
    bolt_v3_market_families::{self, MarketSelectionTarget},
    bolt_v3_operator_artifacts::{
        BoltV3OperatorArtifactError, EntryDecisionSourceBookSideInput,
        EntryDecisionSourceInputRequest, EntryDecisionSourceInputsWritten,
        EntryDecisionSourceMarketInputs, EntryDecisionSourceProofFileRequest, read_file_bounded,
        selected_entry_decision_market, validate_entry_decision_source_proof_files,
        write_entry_decision_source_inputs_from_source_files,
    },
    bolt_v3_providers::EntryDecisionSourceProviderContext,
};

use super::{PolymarketDataConfig, PolymarketExecutionConfig};

const ENTRY_DECISION_FEE_RATE_SOURCE_SCHEMA_VERSION: u32 = 1;
const ENTRY_DECISION_FEE_RATE_SOURCE_RECORD_KIND: &str =
    "bolt_v3.entry_decision_fee_rate_source.v1";
const ENTRY_DECISION_UP_BOOK_LABEL: &str = "up";
const ENTRY_DECISION_DOWN_BOOK_LABEL: &str = "down";
const ENTRY_DECISION_FEE_ZERO_THRESHOLD: f64 = 0.0;
const ENTRY_DECISION_RETRY_INITIAL_ATTEMPT_COUNT: u64 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryDecisionFeeRateSource {
    schema_version: u32,
    record_kind: String,
    fee_bps_by_instrument_id: BTreeMap<String, f64>,
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
    let fee_rate_source: EntryDecisionFeeRateSource = read_decision_source_json_file(
        context.request.fee_rate_source_path,
        context.request.max_fee_rate_source_bytes,
    )?;
    validate_entry_decision_fee_rate_source(&fee_rate_source)?;
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
    require_entry_decision_fee_source(
        &fee_rate_source.fee_bps_by_instrument_id,
        &selected.up_instrument_id.to_string(),
    )?;
    require_entry_decision_fee_source(
        &fee_rate_source.fee_bps_by_instrument_id,
        &selected.down_instrument_id.to_string(),
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
                up_book: book_side_input_from_order_book(&up_book, ENTRY_DECISION_UP_BOOK_LABEL)?,
                down_book: book_side_input_from_order_book(
                    &down_book,
                    ENTRY_DECISION_DOWN_BOOK_LABEL,
                )?,
                fee_bps_by_instrument_id: fee_rate_source.fee_bps_by_instrument_id,
            },
            decision_source_output_path: context.request.decision_source_output_path,
            instrument_source_output_path: context.request.instrument_source_output_path,
        },
    )
}

fn read_decision_source_json_file<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    max_bytes: u64,
) -> Result<T, BoltV3OperatorArtifactError> {
    let bytes = read_file_bounded(path, max_bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3OperatorArtifactError::DecisionEvidenceSourceParse {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn validate_entry_decision_fee_rate_source(
    source: &EntryDecisionFeeRateSource,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.schema_version != ENTRY_DECISION_FEE_RATE_SOURCE_SCHEMA_VERSION {
        return Err(entry_decision_source_invalid(
            "entry decision fee source schema_version is invalid",
        ));
    }
    if source.record_kind != ENTRY_DECISION_FEE_RATE_SOURCE_RECORD_KIND {
        return Err(entry_decision_source_invalid(
            "entry decision fee source record_kind is invalid",
        ));
    }
    if source.fee_bps_by_instrument_id.is_empty() {
        return Err(entry_decision_source_invalid(
            "entry decision fee source requires instrument fee entries",
        ));
    }
    for (instrument_id, fee_bps) in &source.fee_bps_by_instrument_id {
        if instrument_id.trim().is_empty()
            || instrument_id.trim() != instrument_id
            || !fee_bps.is_finite()
            || *fee_bps < ENTRY_DECISION_FEE_ZERO_THRESHOLD
        {
            return Err(entry_decision_source_invalid(
                "entry decision fee source entry is invalid",
            ));
        }
    }
    Ok(())
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

fn require_entry_decision_fee_source(
    fee_bps_by_instrument_id: &BTreeMap<String, f64>,
    instrument_id: &str,
) -> Result<(), BoltV3OperatorArtifactError> {
    let Some(fee_bps) = fee_bps_by_instrument_id.get(instrument_id) else {
        return Err(entry_decision_source_invalid(
            "entry decision source fee map is missing a selected instrument",
        ));
    };
    if !fee_bps.is_finite() || *fee_bps < ENTRY_DECISION_FEE_ZERO_THRESHOLD {
        return Err(entry_decision_source_invalid(
            "entry decision source fee bps is invalid",
        ));
    }
    Ok(())
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
        liquidity_available: bid_quantity.min(ask_quantity),
    })
}

fn entry_decision_source_invalid(message: impl Into<String>) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
        message: message.into(),
    }
}
