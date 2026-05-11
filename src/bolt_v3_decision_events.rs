#![allow(unexpected_cfgs)]

use std::{path::Path, sync::Arc};

use anyhow::{Result, bail};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::data::{CustomData, CustomDataTrait, DataType};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_persistence_macros::custom_data;
use nautilus_serialization::ensure_custom_data_registered;
use serde_json::Value;

use crate::bolt_v3_config::{CatalogFsProtocol, PersistenceBlock, RotationKind};

pub const BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE: &str = "BoltV3MarketSelectionDecisionEvent";
pub const BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE: &str = "BoltV3EntryEvaluationDecisionEvent";
pub const BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE: &str =
    "BoltV3EntryOrderSubmissionDecisionEvent";
pub const BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE: &str =
    "BoltV3EntryPreSubmitRejectionDecisionEvent";
pub const BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE: &str = "BoltV3ExitEvaluationDecisionEvent";
pub const BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE: &str =
    "BoltV3ExitOrderSubmissionDecisionEvent";
pub const BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE: &str =
    "BoltV3ExitPreSubmitRejectionDecisionEvent";
pub const BOLT_V3_MARKET_SELECTION_FAILURE_REASONS: &[&str] = &[
    "request_instruments_failed",
    "instruments_not_in_cache",
    "no_selected_market",
    "ambiguous_selected_market",
    "price_to_beat_unavailable",
    "price_to_beat_ambiguous",
];
const MARKET_SELECTION_RESULT: &str = "market_selection_result";
const ENTRY_EVALUATION: &str = "entry_evaluation";
const ENTRY_ORDER_SUBMISSION: &str = "entry_order_submission";
const ENTRY_PRE_SUBMIT_REJECTION: &str = "entry_pre_submit_rejection";
const EXIT_EVALUATION: &str = "exit_evaluation";
const EXIT_ORDER_SUBMISSION: &str = "exit_order_submission";
const EXIT_PRE_SUBMIT_REJECTION: &str = "exit_pre_submit_rejection";

pub fn validate_bolt_v3_market_selection_failure_reason(reason: &str) -> Result<()> {
    if BOLT_V3_MARKET_SELECTION_FAILURE_REASONS.contains(&reason) {
        return Ok(());
    }

    bail!("unsupported market_selection_failure_reason `{reason}`")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3DecisionEventCommonFields {
    pub schema_version: u64,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3MarketSelectionResultFacts {
    pub market_selection_type: String,
    pub market_selection_timestamp_milliseconds: u64,
    pub market_selection_outcome: String,
    pub market_selection_failure_reason: Option<String>,
    pub rotating_market_family: Option<String>,
    pub underlying_asset: Option<String>,
    pub cadence_seconds: Option<i64>,
    pub market_selection_rule: Option<String>,
    pub retry_interval_seconds: Option<u64>,
    pub blocked_after_seconds: Option<u64>,
    pub polymarket_condition_id: Option<String>,
    pub polymarket_market_slug: Option<String>,
    pub polymarket_question_id: Option<String>,
    pub up_instrument_id: Option<String>,
    pub down_instrument_id: Option<String>,
    pub selected_market_observed_timestamp: Option<u64>,
    pub polymarket_market_start_timestamp_milliseconds: Option<u64>,
    pub polymarket_market_end_timestamp_milliseconds: Option<u64>,
    pub price_to_beat_value: Option<f64>,
    pub price_to_beat_observed_timestamp: Option<u64>,
    pub price_to_beat_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3EntryEvaluationFacts {
    pub updown_side: Option<String>,
    pub entry_decision: String,
    pub entry_no_action_reason: Option<String>,
    pub seconds_to_market_end: u64,
    pub has_selected_market_open_orders: bool,
    pub updown_market_mechanical_outcome: String,
    pub updown_market_mechanical_rejection_reason: Option<String>,
    pub entry_filled_notional: f64,
    pub open_entry_notional: f64,
    pub strategy_remaining_entry_capacity: f64,
    pub archetype_metrics: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3OrderSubmissionFacts {
    pub order_type: String,
    pub time_in_force: String,
    pub instrument_id: String,
    pub side: String,
    pub price: f64,
    pub quantity: f64,
    pub is_quote_quantity: bool,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3RejectedOrderFacts {
    pub order_type: Option<String>,
    pub time_in_force: Option<String>,
    pub instrument_id: Option<String>,
    pub side: Option<String>,
    pub price: Option<f64>,
    pub quantity: Option<f64>,
    pub is_quote_quantity: Option<bool>,
    pub is_post_only: Option<bool>,
    pub is_reduce_only: Option<bool>,
    pub client_order_id: Option<String>,
}

impl From<BoltV3OrderSubmissionFacts> for BoltV3RejectedOrderFacts {
    fn from(facts: BoltV3OrderSubmissionFacts) -> Self {
        Self {
            order_type: Some(facts.order_type),
            time_in_force: Some(facts.time_in_force),
            instrument_id: Some(facts.instrument_id),
            side: Some(facts.side),
            price: Some(facts.price),
            quantity: Some(facts.quantity),
            is_quote_quantity: Some(facts.is_quote_quantity),
            is_post_only: Some(facts.is_post_only),
            is_reduce_only: Some(facts.is_reduce_only),
            client_order_id: facts.client_order_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3PreSubmitRejectionFacts {
    pub order: BoltV3RejectedOrderFacts,
    pub rejection_reason: String,
    pub authoritative_position_quantity: Option<f64>,
    pub authoritative_sellable_quantity: Option<f64>,
    pub open_exit_order_quantity: Option<f64>,
    pub uncovered_position_quantity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3ExitEvaluationFacts {
    pub authoritative_position_quantity: Option<f64>,
    pub authoritative_sellable_quantity: Option<f64>,
    pub open_exit_order_quantity: Option<f64>,
    pub uncovered_position_quantity: Option<f64>,
    pub exit_order_mechanical_outcome: String,
    pub exit_order_mechanical_rejection_reason: Option<String>,
    pub exit_decision: String,
    pub exit_decision_reason: String,
    pub archetype_metrics: Value,
}

#[custom_data]
pub struct BoltV3MarketSelectionDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

#[custom_data]
pub struct BoltV3EntryEvaluationDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

#[custom_data]
pub struct BoltV3EntryOrderSubmissionDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

#[custom_data]
pub struct BoltV3EntryPreSubmitRejectionDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

#[custom_data]
pub struct BoltV3ExitEvaluationDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

#[custom_data]
pub struct BoltV3ExitOrderSubmissionDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

#[custom_data]
pub struct BoltV3ExitPreSubmitRejectionDecisionEvent {
    pub schema_version: u64,
    pub decision_event_type: String,
    pub decision_trace_id: String,
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub trader_id: String,
    pub client_id: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_id: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
    pub configured_target_id: String,
    pub event_facts: Params,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

impl BoltV3MarketSelectionDecisionEvent {
    pub fn market_selection_result(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3MarketSelectionResultFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        validate_market_selection_result_facts(&facts)?;

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: MARKET_SELECTION_RESULT.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: market_selection_result_facts_to_params(facts),
            ts_event,
            ts_init,
        })
    }
}

impl BoltV3EntryEvaluationDecisionEvent {
    pub fn entry_evaluation(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3EntryEvaluationFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        validate_entry_evaluation_facts(&facts)?;

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: ENTRY_EVALUATION.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: entry_evaluation_facts_to_params(facts),
            ts_event,
            ts_init,
        })
    }
}

impl BoltV3EntryOrderSubmissionDecisionEvent {
    pub fn entry_order_submission(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3OrderSubmissionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        if facts.client_order_id.is_none() {
            bail!("client_order_id must be non-null for entry_order_submission");
        }

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: ENTRY_ORDER_SUBMISSION.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: order_submission_facts_to_params(facts),
            ts_event,
            ts_init,
        })
    }
}

impl BoltV3EntryPreSubmitRejectionDecisionEvent {
    pub fn entry_pre_submit_rejection(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3PreSubmitRejectionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        if facts.rejection_reason.is_empty() {
            bail!("entry_pre_submit_rejection_reason must be non-empty");
        }

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: ENTRY_PRE_SUBMIT_REJECTION.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: pre_submit_rejection_facts_to_params(
                facts,
                "entry_pre_submit_rejection_reason",
                false,
            ),
            ts_event,
            ts_init,
        })
    }
}

impl BoltV3ExitEvaluationDecisionEvent {
    pub fn exit_evaluation(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3ExitEvaluationFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        validate_exit_evaluation_facts(&facts)?;

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: EXIT_EVALUATION.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: exit_evaluation_facts_to_params(facts),
            ts_event,
            ts_init,
        })
    }
}

impl BoltV3ExitOrderSubmissionDecisionEvent {
    pub fn exit_order_submission(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3OrderSubmissionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        if facts.client_order_id.is_none() {
            bail!("client_order_id must be non-null for exit_order_submission");
        }

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: EXIT_ORDER_SUBMISSION.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: order_submission_facts_to_params(facts),
            ts_event,
            ts_init,
        })
    }
}

impl BoltV3ExitPreSubmitRejectionDecisionEvent {
    pub fn exit_pre_submit_rejection(
        common: BoltV3DecisionEventCommonFields,
        facts: BoltV3PreSubmitRejectionFacts,
        ts_event: UnixNanos,
        ts_init: UnixNanos,
    ) -> Result<Self> {
        if facts.rejection_reason.is_empty() {
            bail!("exit_pre_submit_rejection_reason must be non-empty");
        }
        if facts.authoritative_position_quantity.is_none()
            || facts.authoritative_sellable_quantity.is_none()
            || facts.open_exit_order_quantity.is_none()
            || facts.uncovered_position_quantity.is_none()
        {
            bail!("exit_pre_submit_rejection requires non-null exit position quantities");
        }

        Ok(Self {
            schema_version: common.schema_version,
            decision_event_type: EXIT_PRE_SUBMIT_REJECTION.to_string(),
            decision_trace_id: common.decision_trace_id,
            strategy_instance_id: common.strategy_instance_id,
            strategy_archetype: common.strategy_archetype,
            trader_id: common.trader_id,
            client_id: common.client_id,
            venue: common.venue,
            runtime_mode: common.runtime_mode,
            release_id: common.release_id,
            config_hash: common.config_hash,
            nautilus_trader_revision: common.nautilus_trader_revision,
            configured_target_id: common.configured_target_id,
            event_facts: pre_submit_rejection_facts_to_params(
                facts,
                "exit_pre_submit_rejection_reason",
                true,
            ),
            ts_event,
            ts_init,
        })
    }
}

pub fn register_bolt_v3_decision_event_types() {
    ensure_custom_data_registered::<BoltV3MarketSelectionDecisionEvent>();
    ensure_custom_data_registered::<BoltV3EntryEvaluationDecisionEvent>();
    ensure_custom_data_registered::<BoltV3EntryOrderSubmissionDecisionEvent>();
    ensure_custom_data_registered::<BoltV3EntryPreSubmitRejectionDecisionEvent>();
    ensure_custom_data_registered::<BoltV3ExitEvaluationDecisionEvent>();
    ensure_custom_data_registered::<BoltV3ExitOrderSubmissionDecisionEvent>();
    ensure_custom_data_registered::<BoltV3ExitPreSubmitRejectionDecisionEvent>();
}

pub struct BoltV3DecisionEventCatalogHandoff {
    catalog: ParquetDataCatalog,
    replace_existing: bool,
}

impl BoltV3DecisionEventCatalogHandoff {
    pub fn from_persistence_block(block: &PersistenceBlock) -> Result<Self> {
        match block.streaming.catalog_fs_protocol {
            CatalogFsProtocol::File => {}
        }
        match block.streaming.rotation_kind {
            RotationKind::None => {}
        }

        Self::new(&block.catalog_directory, block.streaming.replace_existing)
    }

    fn new(catalog_directory: impl AsRef<Path>, replace_existing: bool) -> Result<Self> {
        register_bolt_v3_decision_event_types();
        Ok(Self {
            catalog: ParquetDataCatalog::new(catalog_directory.as_ref(), None, None, None, None),
            replace_existing,
        })
    }

    pub fn write_market_selection_result(
        &mut self,
        event: BoltV3MarketSelectionDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    pub fn write_entry_evaluation(
        &mut self,
        event: BoltV3EntryEvaluationDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    pub fn write_entry_order_submission(
        &mut self,
        event: BoltV3EntryOrderSubmissionDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    pub fn write_entry_pre_submit_rejection(
        &mut self,
        event: BoltV3EntryPreSubmitRejectionDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    pub fn write_exit_evaluation(
        &mut self,
        event: BoltV3ExitEvaluationDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_EXIT_EVALUATION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    pub fn write_exit_order_submission(
        &mut self,
        event: BoltV3ExitOrderSubmissionDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    pub fn write_exit_pre_submit_rejection(
        &mut self,
        event: BoltV3ExitPreSubmitRejectionDecisionEvent,
    ) -> Result<()> {
        self.write_event(
            BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE,
            event.configured_target_id.clone(),
            event,
        )
    }

    fn write_event<T>(
        &mut self,
        type_name: &'static str,
        identifier: String,
        event: T,
    ) -> Result<()>
    where
        T: CustomDataTrait + 'static,
    {
        let data_type = DataType::new(type_name, None, Some(identifier));
        let custom = CustomData::new(Arc::new(event), data_type);
        self.catalog.write_custom_data_batch(
            vec![custom],
            None,
            None,
            Some(self.replace_existing),
        )?;
        Ok(())
    }
}

fn validate_market_selection_result_facts(facts: &BoltV3MarketSelectionResultFacts) -> Result<()> {
    match facts.market_selection_outcome.as_str() {
        "current" | "next" => {
            if facts.market_selection_failure_reason.is_some() {
                bail!(
                    "market_selection_failure_reason must be null when market_selection_outcome is current or next"
                );
            }
        }
        "failed" => {
            let Some(reason) = facts.market_selection_failure_reason.as_deref() else {
                bail!(
                    "market_selection_failure_reason must be non-null when market_selection_outcome is failed"
                );
            };
            validate_bolt_v3_market_selection_failure_reason(reason)?;
        }
        value => bail!("unsupported market_selection_outcome `{value}`"),
    }

    if facts.market_selection_type == "rotating_market" {
        require_some(
            facts.rotating_market_family.as_ref(),
            "rotating_market_family",
        )?;
        require_some(facts.underlying_asset.as_ref(), "underlying_asset")?;
        require_some(facts.cadence_seconds.as_ref(), "cadence_seconds")?;
        require_some(
            facts.market_selection_rule.as_ref(),
            "market_selection_rule",
        )?;
        require_some(
            facts.retry_interval_seconds.as_ref(),
            "retry_interval_seconds",
        )?;
        require_some(
            facts.blocked_after_seconds.as_ref(),
            "blocked_after_seconds",
        )?;

        if matches!(facts.market_selection_outcome.as_str(), "current" | "next") {
            require_some(
                facts.polymarket_condition_id.as_ref(),
                "polymarket_condition_id",
            )?;
            require_some(
                facts.polymarket_market_slug.as_ref(),
                "polymarket_market_slug",
            )?;
            require_some(
                facts.polymarket_question_id.as_ref(),
                "polymarket_question_id",
            )?;
            require_some(facts.up_instrument_id.as_ref(), "up_instrument_id")?;
            require_some(facts.down_instrument_id.as_ref(), "down_instrument_id")?;
            require_some(
                facts.selected_market_observed_timestamp.as_ref(),
                "selected_market_observed_timestamp",
            )?;
            require_some(
                facts
                    .polymarket_market_start_timestamp_milliseconds
                    .as_ref(),
                "polymarket_market_start_timestamp_milliseconds",
            )?;
            require_some(
                facts.polymarket_market_end_timestamp_milliseconds.as_ref(),
                "polymarket_market_end_timestamp_milliseconds",
            )?;
            require_some(facts.price_to_beat_value.as_ref(), "price_to_beat_value")?;
            require_some(
                facts.price_to_beat_observed_timestamp.as_ref(),
                "price_to_beat_observed_timestamp",
            )?;
            require_some(facts.price_to_beat_source.as_ref(), "price_to_beat_source")?;
        }
    }

    Ok(())
}

fn require_some<T>(value: Option<&T>, field_name: &str) -> Result<()> {
    if value.is_none() {
        bail!("{field_name} must be non-null");
    }
    Ok(())
}

fn market_selection_result_facts_to_params(facts: BoltV3MarketSelectionResultFacts) -> Params {
    let mut params = Params::new();
    params.insert(
        "market_selection_type".to_string(),
        Value::String(facts.market_selection_type),
    );
    params.insert(
        "market_selection_timestamp_milliseconds".to_string(),
        Value::from(facts.market_selection_timestamp_milliseconds),
    );
    params.insert(
        "market_selection_outcome".to_string(),
        Value::String(facts.market_selection_outcome),
    );
    params.insert(
        "market_selection_failure_reason".to_string(),
        facts
            .market_selection_failure_reason
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "rotating_market_family".to_string(),
        facts
            .rotating_market_family
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "underlying_asset".to_string(),
        facts
            .underlying_asset
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "cadence_seconds".to_string(),
        facts
            .cadence_seconds
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "market_selection_rule".to_string(),
        facts
            .market_selection_rule
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "retry_interval_seconds".to_string(),
        facts
            .retry_interval_seconds
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "blocked_after_seconds".to_string(),
        facts
            .blocked_after_seconds
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "polymarket_condition_id".to_string(),
        facts
            .polymarket_condition_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "polymarket_market_slug".to_string(),
        facts
            .polymarket_market_slug
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "polymarket_question_id".to_string(),
        facts
            .polymarket_question_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "up_instrument_id".to_string(),
        facts
            .up_instrument_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "down_instrument_id".to_string(),
        facts
            .down_instrument_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "selected_market_observed_timestamp".to_string(),
        facts
            .selected_market_observed_timestamp
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "polymarket_market_start_timestamp_milliseconds".to_string(),
        facts
            .polymarket_market_start_timestamp_milliseconds
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "polymarket_market_end_timestamp_milliseconds".to_string(),
        facts
            .polymarket_market_end_timestamp_milliseconds
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "price_to_beat_value".to_string(),
        facts
            .price_to_beat_value
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "price_to_beat_observed_timestamp".to_string(),
        facts
            .price_to_beat_observed_timestamp
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "price_to_beat_source".to_string(),
        facts
            .price_to_beat_source
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params
}

fn validate_entry_evaluation_facts(facts: &BoltV3EntryEvaluationFacts) -> Result<()> {
    match facts.entry_decision.as_str() {
        "enter" => {
            if facts.entry_no_action_reason.is_some() {
                bail!("entry_no_action_reason must be null when entry_decision is enter");
            }
            match facts.updown_side.as_deref() {
                Some("up" | "down") => {}
                Some(value) => bail!("unsupported updown_side `{value}`"),
                None => bail!("updown_side must be non-null when entry_decision is enter"),
            }
            if facts.updown_market_mechanical_outcome != "accepted" {
                bail!("entry_decision enter requires updown_market_mechanical_outcome accepted");
            }
        }
        "no_action" => {
            if facts.entry_no_action_reason.is_none() {
                bail!("entry_no_action_reason must be non-null when entry_decision is no_action");
            }
            if facts.updown_side.is_some() {
                bail!("updown_side must be null when entry_decision is no_action");
            }
        }
        value => bail!("unsupported entry_decision `{value}`"),
    }

    match facts.updown_market_mechanical_outcome.as_str() {
        "accepted" => {
            if facts.updown_market_mechanical_rejection_reason.is_some() {
                bail!(
                    "updown_market_mechanical_rejection_reason must be null when updown_market_mechanical_outcome is accepted"
                );
            }
            if facts.has_selected_market_open_orders {
                bail!(
                    "has_selected_market_open_orders must be false when updown_market_mechanical_outcome is accepted"
                );
            }
        }
        "rejected" => {
            match facts.updown_market_mechanical_rejection_reason.as_deref() {
                Some(
                    "market_not_started" | "market_ended" | "selected_market_open_orders_present",
                ) => {}
                Some(value) => {
                    bail!("unsupported updown_market_mechanical_rejection_reason `{value}`");
                }
                None => {
                    bail!(
                        "updown_market_mechanical_rejection_reason must be non-null when updown_market_mechanical_outcome is rejected"
                    );
                }
            }
            if facts.entry_decision != "no_action" {
                bail!(
                    "updown_market_mechanical_outcome rejected requires entry_decision no_action"
                );
            }
        }
        value => bail!("unsupported updown_market_mechanical_outcome `{value}`"),
    }

    match facts.entry_no_action_reason.as_deref() {
        Some("updown_market_mechanical_rejection") => {
            if facts.updown_market_mechanical_outcome != "rejected"
                || facts.updown_market_mechanical_rejection_reason.is_none()
            {
                bail!(
                    "entry_no_action_reason updown_market_mechanical_rejection requires rejected mechanical outcome and non-null rejection reason"
                );
            }
        }
        Some(
            "missing_reference_quote"
            | "stale_reference_quote"
            | "fee_rate_unavailable"
            | "fair_probability_unavailable"
            | "insufficient_edge"
            | "market_cooling_down"
            | "recovery_mode"
            | "position_limit_reached",
        ) => {
            if facts.updown_market_mechanical_outcome != "accepted"
                || facts.updown_market_mechanical_rejection_reason.is_some()
            {
                bail!(
                    "non-mechanical entry_no_action_reason requires accepted mechanical outcome and null rejection reason"
                );
            }
        }
        Some(value) => bail!("unsupported entry_no_action_reason `{value}`"),
        None => {}
    }

    if facts.updown_market_mechanical_rejection_reason.as_deref()
        == Some("selected_market_open_orders_present")
        && !facts.has_selected_market_open_orders
    {
        bail!(
            "has_selected_market_open_orders must be true when updown_market_mechanical_rejection_reason is selected_market_open_orders_present"
        );
    }

    if facts.entry_no_action_reason.as_deref() == Some("position_limit_reached")
        && facts.strategy_remaining_entry_capacity > 0.0
    {
        bail!(
            "strategy_remaining_entry_capacity must be <= 0 when entry_no_action_reason is position_limit_reached"
        );
    }

    if !facts.archetype_metrics.is_object() {
        bail!("archetype_metrics must be an object");
    }

    Ok(())
}

fn entry_evaluation_facts_to_params(facts: BoltV3EntryEvaluationFacts) -> Params {
    let mut params = Params::new();
    params.insert(
        "updown_side".to_string(),
        optional_string_to_value(facts.updown_side),
    );
    params.insert(
        "entry_decision".to_string(),
        Value::String(facts.entry_decision),
    );
    params.insert(
        "entry_no_action_reason".to_string(),
        optional_string_to_value(facts.entry_no_action_reason),
    );
    params.insert(
        "seconds_to_market_end".to_string(),
        Value::from(facts.seconds_to_market_end),
    );
    params.insert(
        "has_selected_market_open_orders".to_string(),
        Value::from(facts.has_selected_market_open_orders),
    );
    params.insert(
        "updown_market_mechanical_outcome".to_string(),
        Value::String(facts.updown_market_mechanical_outcome),
    );
    params.insert(
        "updown_market_mechanical_rejection_reason".to_string(),
        optional_string_to_value(facts.updown_market_mechanical_rejection_reason),
    );
    params.insert(
        "entry_filled_notional".to_string(),
        Value::from(facts.entry_filled_notional),
    );
    params.insert(
        "open_entry_notional".to_string(),
        Value::from(facts.open_entry_notional),
    );
    params.insert(
        "strategy_remaining_entry_capacity".to_string(),
        Value::from(facts.strategy_remaining_entry_capacity),
    );
    params.insert("archetype_metrics".to_string(), facts.archetype_metrics);
    params
}

fn order_submission_facts_to_params(facts: BoltV3OrderSubmissionFacts) -> Params {
    let mut params = Params::new();
    params.insert("order_type".to_string(), Value::String(facts.order_type));
    params.insert(
        "time_in_force".to_string(),
        Value::String(facts.time_in_force),
    );
    params.insert(
        "instrument_id".to_string(),
        Value::String(facts.instrument_id),
    );
    params.insert("side".to_string(), Value::String(facts.side));
    params.insert("price".to_string(), Value::from(facts.price));
    params.insert("quantity".to_string(), Value::from(facts.quantity));
    params.insert(
        "is_quote_quantity".to_string(),
        Value::from(facts.is_quote_quantity),
    );
    params.insert("is_post_only".to_string(), Value::from(facts.is_post_only));
    params.insert(
        "is_reduce_only".to_string(),
        Value::from(facts.is_reduce_only),
    );
    params.insert(
        "client_order_id".to_string(),
        facts
            .client_order_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params
}

fn rejected_order_facts_to_params(facts: BoltV3RejectedOrderFacts) -> Params {
    let mut params = Params::new();
    params.insert(
        "order_type".to_string(),
        optional_string_to_value(facts.order_type),
    );
    params.insert(
        "time_in_force".to_string(),
        optional_string_to_value(facts.time_in_force),
    );
    params.insert(
        "instrument_id".to_string(),
        optional_string_to_value(facts.instrument_id),
    );
    params.insert("side".to_string(), optional_string_to_value(facts.side));
    params.insert("price".to_string(), optional_f64_to_value(facts.price));
    params.insert(
        "quantity".to_string(),
        optional_f64_to_value(facts.quantity),
    );
    params.insert(
        "is_quote_quantity".to_string(),
        optional_bool_to_value(facts.is_quote_quantity),
    );
    params.insert(
        "is_post_only".to_string(),
        optional_bool_to_value(facts.is_post_only),
    );
    params.insert(
        "is_reduce_only".to_string(),
        optional_bool_to_value(facts.is_reduce_only),
    );
    params.insert(
        "client_order_id".to_string(),
        optional_string_to_value(facts.client_order_id),
    );
    params
}

fn validate_exit_evaluation_facts(facts: &BoltV3ExitEvaluationFacts) -> Result<()> {
    match facts.exit_order_mechanical_outcome.as_str() {
        "accepted" => {
            if facts.exit_order_mechanical_rejection_reason.is_some() {
                bail!(
                    "exit_order_mechanical_rejection_reason must be null when exit_order_mechanical_outcome is accepted"
                );
            }
            match (
                facts.exit_decision.as_str(),
                facts.exit_decision_reason.as_str(),
            ) {
                ("hold", "active_exit_not_defined") => {}
                ("exit", "forced_flat" | "ev_hysteresis" | "fail_closed") => {}
                _ => {
                    bail!(
                        "accepted exit_order_mechanical_outcome requires exit_decision hold/active_exit_not_defined or exit with supported reason"
                    );
                }
            }
        }
        "rejected" => {
            match facts.exit_order_mechanical_rejection_reason.as_deref() {
                Some(
                    "position_quantity_unconfirmed"
                    | "open_exit_order_quantity_unconfirmed"
                    | "open_exit_order_quantity_covers_position"
                    | "sellable_quantity_unconfirmed"
                    | "sellable_quantity_zero"
                    | "exit_bid_unavailable"
                    | "exit_quantity_invalid"
                    | "exit_price_invalid",
                ) => {}
                Some(value) => {
                    bail!("unsupported exit_order_mechanical_rejection_reason `{value}`");
                }
                None => {
                    bail!(
                        "exit_order_mechanical_rejection_reason must be non-null when exit_order_mechanical_outcome is rejected"
                    );
                }
            }
            if facts.exit_decision != "hold"
                || facts.exit_decision_reason != "exit_order_mechanical_rejection"
            {
                bail!(
                    "rejected exit_order_mechanical_outcome requires exit_decision hold and exit_decision_reason exit_order_mechanical_rejection"
                );
            }
        }
        value => bail!("unsupported exit_order_mechanical_outcome `{value}`"),
    }

    if !facts.archetype_metrics.is_object() {
        bail!("archetype_metrics must be an object");
    }

    Ok(())
}

fn exit_evaluation_facts_to_params(facts: BoltV3ExitEvaluationFacts) -> Params {
    let mut params = Params::new();
    params.insert(
        "authoritative_position_quantity".to_string(),
        optional_f64_to_value(facts.authoritative_position_quantity),
    );
    params.insert(
        "authoritative_sellable_quantity".to_string(),
        optional_f64_to_value(facts.authoritative_sellable_quantity),
    );
    params.insert(
        "open_exit_order_quantity".to_string(),
        optional_f64_to_value(facts.open_exit_order_quantity),
    );
    params.insert(
        "uncovered_position_quantity".to_string(),
        optional_f64_to_value(facts.uncovered_position_quantity),
    );
    params.insert(
        "exit_order_mechanical_outcome".to_string(),
        Value::String(facts.exit_order_mechanical_outcome),
    );
    params.insert(
        "exit_order_mechanical_rejection_reason".to_string(),
        optional_string_to_value(facts.exit_order_mechanical_rejection_reason),
    );
    params.insert(
        "exit_decision".to_string(),
        Value::String(facts.exit_decision),
    );
    params.insert(
        "exit_decision_reason".to_string(),
        Value::String(facts.exit_decision_reason),
    );
    params.insert("archetype_metrics".to_string(), facts.archetype_metrics);
    params
}

fn pre_submit_rejection_facts_to_params(
    facts: BoltV3PreSubmitRejectionFacts,
    rejection_reason_key: &str,
    include_exit_position_facts: bool,
) -> Params {
    let mut params = rejected_order_facts_to_params(facts.order);
    params.insert(
        rejection_reason_key.to_string(),
        Value::String(facts.rejection_reason),
    );
    if include_exit_position_facts {
        params.insert(
            "authoritative_position_quantity".to_string(),
            optional_f64_to_value(facts.authoritative_position_quantity),
        );
        params.insert(
            "authoritative_sellable_quantity".to_string(),
            optional_f64_to_value(facts.authoritative_sellable_quantity),
        );
        params.insert(
            "open_exit_order_quantity".to_string(),
            optional_f64_to_value(facts.open_exit_order_quantity),
        );
        params.insert(
            "uncovered_position_quantity".to_string(),
            optional_f64_to_value(facts.uncovered_position_quantity),
        );
    }
    params
}

fn optional_string_to_value(value: Option<String>) -> Value {
    value.map(Value::String).unwrap_or(Value::Null)
}

fn optional_f64_to_value(value: Option<f64>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}

fn optional_bool_to_value(value: Option<bool>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}
