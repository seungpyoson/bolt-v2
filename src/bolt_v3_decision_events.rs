#![allow(unexpected_cfgs)]

use std::{path::Path, sync::Arc};

use anyhow::{Result, bail};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::data::{CustomData, DataType};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_persistence_macros::custom_data;
use nautilus_serialization::ensure_custom_data_registered;
use serde_json::Value;

use crate::bolt_v3_config::{CatalogFsProtocol, PersistenceBlock, RotationKind};

pub const BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE: &str = "BoltV3MarketSelectionDecisionEvent";
const MARKET_SELECTION_RESULT: &str = "market_selection_result";

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

pub fn register_bolt_v3_decision_event_types() {
    ensure_custom_data_registered::<BoltV3MarketSelectionDecisionEvent>();
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
        let identifier = event.configured_target_id.clone();
        let data_type = DataType::new(
            BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE,
            None,
            Some(identifier),
        );
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
