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
pub const BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE: &str =
    "BoltV3EntryOrderSubmissionDecisionEvent";
pub const BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE: &str =
    "BoltV3EntryPreSubmitRejectionDecisionEvent";
pub const BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE: &str =
    "BoltV3ExitOrderSubmissionDecisionEvent";
pub const BOLT_V3_EXIT_PRE_SUBMIT_REJECTION_DECISION_EVENT_TYPE: &str =
    "BoltV3ExitPreSubmitRejectionDecisionEvent";
const MARKET_SELECTION_RESULT: &str = "market_selection_result";
const ENTRY_ORDER_SUBMISSION: &str = "entry_order_submission";
const ENTRY_PRE_SUBMIT_REJECTION: &str = "entry_pre_submit_rejection";
const EXIT_ORDER_SUBMISSION: &str = "exit_order_submission";
const EXIT_PRE_SUBMIT_REJECTION: &str = "exit_pre_submit_rejection";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3MarketSelectionResultFacts {
    pub market_selection_type: String,
    pub market_selection_timestamp_milliseconds: u64,
    pub market_selection_outcome: String,
    pub market_selection_failure_reason: Option<String>,
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
pub struct BoltV3PreSubmitRejectionFacts {
    pub order: BoltV3OrderSubmissionFacts,
    pub rejection_reason: String,
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
            ),
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
            ),
            ts_event,
            ts_init,
        })
    }
}

pub fn register_bolt_v3_decision_event_types() {
    ensure_custom_data_registered::<BoltV3MarketSelectionDecisionEvent>();
    ensure_custom_data_registered::<BoltV3EntryOrderSubmissionDecisionEvent>();
    ensure_custom_data_registered::<BoltV3EntryPreSubmitRejectionDecisionEvent>();
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
            if facts.market_selection_failure_reason.is_none() {
                bail!(
                    "market_selection_failure_reason must be non-null when market_selection_outcome is failed"
                );
            }
        }
        value => bail!("unsupported market_selection_outcome `{value}`"),
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

fn pre_submit_rejection_facts_to_params(
    facts: BoltV3PreSubmitRejectionFacts,
    rejection_reason_key: &str,
) -> Params {
    let mut params = order_submission_facts_to_params(facts.order);
    params.insert(
        rejection_reason_key.to_string(),
        Value::String(facts.rejection_reason),
    );
    params
}
