use nautilus_live::node::LiveNode;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_market_families::{
        BoltV3MarketIdentityError, MarketIdentityPlan, plan_market_identity,
    },
    bolt_v3_provider_family_bindings,
    bolt_v3_providers::{
        ProviderInstrumentReadinessContext, ProviderInstrumentReadinessFact,
        ProviderInstrumentReadinessStatus,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3InstrumentReadinessReport {
    pub facts: Vec<BoltV3InstrumentReadinessFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3InstrumentReadinessFact {
    pub client_id_key: String,
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub status: BoltV3InstrumentReadinessStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3InstrumentReadinessStatus {
    Ready,
    Blocked,
}

impl BoltV3InstrumentReadinessReport {
    pub fn is_ready(&self) -> bool {
        self.facts
            .iter()
            .all(|fact| fact.status == BoltV3InstrumentReadinessStatus::Ready)
    }
}

pub fn check_bolt_v3_instrument_readiness_for_start(
    node: &LiveNode,
    loaded: &LoadedBoltV3Config,
    market_selection_timestamp_milliseconds: i64,
) -> Result<BoltV3InstrumentReadinessReport, BoltV3MarketIdentityError> {
    let plan = plan_market_identity(loaded)?;
    check_plan_instrument_readiness_for_start(
        node,
        loaded,
        &plan,
        market_selection_timestamp_milliseconds,
    )
}

fn check_plan_instrument_readiness_for_start(
    node: &LiveNode,
    loaded: &LoadedBoltV3Config,
    plan: &MarketIdentityPlan,
    market_selection_timestamp_milliseconds: i64,
) -> Result<BoltV3InstrumentReadinessReport, BoltV3MarketIdentityError> {
    let cache = node.kernel().cache();
    let cache = cache.borrow();
    let mut facts = Vec::new();
    for (client_id_key, client) in &loaded.root.clients {
        if !plan.has_client_targets(client_id_key) {
            continue;
        }
        let provider_facts = bolt_v3_provider_family_bindings::check_instrument_readiness(
            ProviderInstrumentReadinessContext {
                client_id_key,
                venue_key: client.venue.as_str(),
                plan,
                cache: &cache,
                market_selection_timestamp_milliseconds,
            },
        )?;
        facts.extend(provider_facts.into_iter().map(from_provider_fact));
    }
    Ok(BoltV3InstrumentReadinessReport { facts })
}

fn from_provider_fact(fact: ProviderInstrumentReadinessFact) -> BoltV3InstrumentReadinessFact {
    BoltV3InstrumentReadinessFact {
        client_id_key: fact.client_id_key,
        strategy_instance_id: fact.strategy_instance_id,
        configured_target_id: fact.configured_target_id,
        status: match fact.status {
            ProviderInstrumentReadinessStatus::Ready => BoltV3InstrumentReadinessStatus::Ready,
            ProviderInstrumentReadinessStatus::Blocked => BoltV3InstrumentReadinessStatus::Blocked,
        },
        detail: fact.detail,
    }
}
