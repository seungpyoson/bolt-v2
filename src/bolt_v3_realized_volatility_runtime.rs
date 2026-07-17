//! Process-level realized-volatility surface runtime.

use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use nautilus_model::{
    data::{IndexPriceUpdate, QuoteTick, TradeTick},
    identifiers::{ClientId, InstrumentId},
};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, realized_volatility_engine_config},
    bolt_v3_numeric::{MIDPOINT_DIVISOR_F64, NANOS_PER_MILLI_U64, is_positive_finite},
    bolt_v3_providers::new_risk_market_data_available,
    bolt_v3_realized_volatility::{
        RealizedVolBlockReason, RealizedVolEngine, RealizedVolEngineConfig, RealizedVolObservation,
        RealizedVolSampleKind, RealizedVolSnapshot, RealizedVolSourceClass,
        RealizedVolSourceStatus,
    },
    bolt_v3_timestamp_domain::{NtStrategyClockMs, VenueEventMs},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RealizedVolSubscriptionKind {
    Quotes,
    Trades,
    IndexPrices,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RealizedVolSubscriptionRequest {
    pub instrument_id: InstrumentId,
    pub data_client_id: ClientId,
    pub kind: RealizedVolSubscriptionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EventRouteKey {
    instrument_id: InstrumentId,
    kind: RealizedVolSubscriptionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RealizedVolSourceRoute {
    surface_id: String,
    source_id: String,
    source_class: RealizedVolSourceClass,
    sample_kind: RealizedVolSampleKind,
}

#[derive(Debug, Clone, PartialEq)]
struct RealizedVolSurfaceState {
    engine: RealizedVolEngine,
    latest_snapshot: Option<RealizedVolSnapshot>,
    last_refresh_ms: Option<VenueEventMs>,
    last_strategy_refresh_ms: Option<NtStrategyClockMs>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolSurfaceRuntime {
    surfaces: BTreeMap<String, RealizedVolSurfaceState>,
    routes_by_event: BTreeMap<EventRouteKey, Vec<RealizedVolSourceRoute>>,
    // Single source of truth for RV subscriptions, keyed by `surface_id`. The global
    // `subscription_requests` accessor is only a DERIVED deduped union for audit/fanout
    // semantics; production strategy callers must use the `*_for_surface` variants so a
    // strategy only subscribes its configured surface.
    subscription_requests_by_surface: BTreeMap<String, Vec<RealizedVolSubscriptionRequest>>,
    new_risk_capability_unavailable_sources: BTreeSet<(String, String)>,
}

impl RealizedVolSurfaceRuntime {
    pub fn empty() -> Self {
        Self {
            surfaces: BTreeMap::new(),
            routes_by_event: BTreeMap::new(),
            subscription_requests_by_surface: BTreeMap::new(),
            new_risk_capability_unavailable_sources: BTreeSet::new(),
        }
    }

    pub fn from_configs(
        configs: BTreeMap<String, RealizedVolEngineConfig>,
    ) -> Result<Self, String> {
        Self::from_configs_with_unavailable_sources(configs, BTreeSet::new())
    }

    fn from_configs_with_unavailable_sources(
        configs: BTreeMap<String, RealizedVolEngineConfig>,
        new_risk_capability_unavailable_sources: BTreeSet<(String, String)>,
    ) -> Result<Self, String> {
        let mut surfaces = BTreeMap::new();
        let mut routes_by_event: BTreeMap<EventRouteKey, Vec<RealizedVolSourceRoute>> =
            BTreeMap::new();
        let mut subscription_requests_by_surface: BTreeMap<
            String,
            BTreeSet<RealizedVolSubscriptionRequest>,
        > = BTreeMap::new();

        for (surface_id, config) in configs {
            if surface_id != config.surface_id {
                return Err(format!(
                    "realized_volatility_surfaces.{surface_id} config surface_id `{}` must match map key",
                    config.surface_id
                ));
            }
            let engine = RealizedVolEngine::from_config(config.clone())?;
            for source in &config.sources {
                if source.source_class == RealizedVolSourceClass::Mark
                    || source.sample_kind == RealizedVolSampleKind::Mark
                {
                    return Err(format!(
                        "realized_volatility_surfaces.{surface_id}.sources source_id `{}` mark source routing is not supported",
                        source.source_id
                    ));
                }
                let Some(kind) = subscription_kind(source.source_class, source.sample_kind) else {
                    continue;
                };
                if new_risk_capability_unavailable_sources
                    .contains(&(surface_id.clone(), source.source_id.clone()))
                {
                    continue;
                }
                let instrument_id = InstrumentId::from_str(source.instrument_id.as_str())
                    .map_err(|error| {
                        format!(
                            "realized_volatility_surfaces.{surface_id}.sources source_id `{}` instrument_id `{}` is invalid: {error}",
                            source.source_id, source.instrument_id
                        )
                    })?;
                let data_client_id = ClientId::from(source.data_client_id.as_str());
                let route = RealizedVolSourceRoute {
                    surface_id: surface_id.clone(),
                    source_id: source.source_id.clone(),
                    source_class: source.source_class,
                    sample_kind: source.sample_kind,
                };
                let route_key = EventRouteKey {
                    instrument_id,
                    kind,
                };
                if let Some(routes) = routes_by_event.get_mut(&route_key) {
                    routes.push(route);
                } else {
                    routes_by_event.insert(route_key, vec![route]);
                }
                if source.enabled {
                    let request = RealizedVolSubscriptionRequest {
                        instrument_id,
                        data_client_id,
                        kind,
                    };
                    if let Some(requests) = subscription_requests_by_surface.get_mut(&surface_id) {
                        requests.insert(request);
                    } else {
                        let mut requests = BTreeSet::new();
                        requests.insert(request);
                        subscription_requests_by_surface.insert(surface_id.clone(), requests);
                    }
                }
            }
            surfaces.insert(
                surface_id,
                RealizedVolSurfaceState {
                    engine,
                    latest_snapshot: None,
                    last_refresh_ms: None,
                    last_strategy_refresh_ms: None,
                },
            );
        }

        let subscription_requests_by_surface = subscription_requests_by_surface
            .into_iter()
            .map(|(surface_id, requests)| (surface_id, requests.into_iter().collect()))
            .collect();

        Ok(Self {
            surfaces,
            routes_by_event,
            subscription_requests_by_surface,
            new_risk_capability_unavailable_sources,
        })
    }

    pub fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self, String> {
        let configs = loaded
            .root
            .realized_volatility_surfaces
            .as_ref()
            .into_iter()
            .flat_map(|surfaces| surfaces.iter())
            .map(|(surface_id, surface)| {
                realized_volatility_engine_config(surface_id, surface)
                    .map(|config| (surface_id.clone(), config))
                    .map_err(|error| {
                        format!(
                            "realized_volatility_surfaces.{surface_id} could not build engine config: {error}"
                        )
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let unavailable_client_ids = unavailable_realized_volatility_new_risk_client_ids(loaded)?;
        let unavailable_sources = configs
            .iter()
            .flat_map(|(surface_id, config)| {
                config
                    .sources
                    .iter()
                    .filter(|source| {
                        unavailable_client_ids.contains(source.data_client_id.as_str())
                    })
                    .map(|source| (surface_id.clone(), source.source_id.clone()))
            })
            .collect();
        Self::from_configs_with_unavailable_sources(configs, unavailable_sources)
    }

    pub fn surface_ids(&self) -> Vec<String> {
        self.surfaces.keys().cloned().collect()
    }

    /// Deduped union of every surface's subscription requests, in canonical (sorted) order.
    /// Test/audit-only derived view for fanout diagnostics (see
    /// `subscription_requests_by_surface`). A strategy must NOT subscribe from this; use
    /// `*_for_surface` with its configured surface.
    #[doc(hidden)]
    pub fn subscription_requests(&self) -> Vec<RealizedVolSubscriptionRequest> {
        let mut union: BTreeSet<RealizedVolSubscriptionRequest> = BTreeSet::new();
        for requests in self.subscription_requests_by_surface.values() {
            union.extend(requests.iter().cloned());
        }
        union.into_iter().collect()
    }

    /// Subscriptions belonging to a single configured surface. Returns an empty `Vec` for an
    /// unknown surface, which leaves pricing `RealizedVolNotReady` (fail-closed); config
    /// validation already rejects unknown configured surfaces at load time.
    pub fn subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<RealizedVolSubscriptionRequest> {
        match self.subscription_requests_by_surface.get(surface_id) {
            Some(requests) => requests.clone(),
            // Unknown surface: no subscriptions, so pricing stays `RealizedVolNotReady`
            // (fail-closed). Config validation rejects unknown configured surfaces at load.
            None => Vec::new(),
        }
    }

    pub fn surface_subscriptions_blocked_only_by_provider_capability(
        &self,
        surface_id: &str,
    ) -> bool {
        let Some(state) = self.surfaces.get(surface_id) else {
            return false;
        };
        let mut found_enabled_subscribable_source = false;
        for source in state.engine.config().sources.iter().filter(|source| {
            source.enabled && subscription_kind(source.source_class, source.sample_kind).is_some()
        }) {
            found_enabled_subscribable_source = true;
            if !self.new_risk_capability_unavailable_sources.iter().any(
                |(blocked_surface_id, blocked_source_id)| {
                    blocked_surface_id == surface_id && blocked_source_id == &source.source_id
                },
            ) {
                return false;
            }
        }
        found_enabled_subscribable_source
    }

    pub fn source_new_risk_capability_unavailable(
        &self,
        surface_id: &str,
        source_id: &str,
    ) -> bool {
        self.new_risk_capability_unavailable_sources
            .contains(&(surface_id.to_string(), source_id.to_string()))
    }

    pub fn quote_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        match self.subscription_requests_by_surface.get(surface_id) {
            Some(requests) => {
                quote_trade_index_requests(requests, RealizedVolSubscriptionKind::Quotes)
            }
            None => Vec::new(),
        }
    }

    pub fn trade_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        match self.subscription_requests_by_surface.get(surface_id) {
            Some(requests) => {
                quote_trade_index_requests(requests, RealizedVolSubscriptionKind::Trades)
            }
            None => Vec::new(),
        }
    }

    pub fn index_subscription_requests_for_surface(
        &self,
        surface_id: &str,
    ) -> Vec<(InstrumentId, Option<ClientId>)> {
        match self.subscription_requests_by_surface.get(surface_id) {
            Some(requests) => {
                quote_trade_index_requests(requests, RealizedVolSubscriptionKind::IndexPrices)
            }
            None => Vec::new(),
        }
    }

    pub fn observe(&mut self, observation: RealizedVolObservation) -> bool {
        let mut matched_configured_source = false;
        let mut observed = false;
        for (surface_id, state) in &mut self.surfaces {
            let source_belongs_to_surface = state
                .engine
                .config()
                .sources
                .iter()
                .any(|source| source.source_id == observation.source_id);
            if source_belongs_to_surface {
                matched_configured_source = true;
                if !self
                    .new_risk_capability_unavailable_sources
                    .contains(&(surface_id.clone(), observation.source_id.clone()))
                {
                    observed |= state.engine.observe(observation.clone());
                }
            }
        }
        if matched_configured_source {
            return observed;
        }
        false
    }

    pub fn observe_quote(&mut self, quote: &QuoteTick) -> Vec<RealizedVolSnapshot> {
        let bid = quote.bid_price.as_f64();
        let ask = quote.ask_price.as_f64();
        if !is_positive_finite(bid) || !is_positive_finite(ask) {
            return Vec::new();
        }
        let midpoint = (bid + ask) / MIDPOINT_DIVISOR_F64;
        if !is_positive_finite(midpoint) {
            return Vec::new();
        }
        self.observe_routed_price(
            EventRouteKey {
                instrument_id: quote.instrument_id,
                kind: RealizedVolSubscriptionKind::Quotes,
            },
            midpoint,
            quote.ts_event.as_u64() / NANOS_PER_MILLI_U64,
            quote.ts_init.as_u64() / NANOS_PER_MILLI_U64,
        )
    }

    pub fn observe_trade(&mut self, trade: &TradeTick) -> Vec<RealizedVolSnapshot> {
        let price = trade.price.as_f64();
        if !is_positive_finite(price) {
            return Vec::new();
        }
        self.observe_routed_price(
            EventRouteKey {
                instrument_id: trade.instrument_id,
                kind: RealizedVolSubscriptionKind::Trades,
            },
            price,
            trade.ts_event.as_u64() / NANOS_PER_MILLI_U64,
            trade.ts_init.as_u64() / NANOS_PER_MILLI_U64,
        )
    }

    pub fn observe_index_price(&mut self, update: &IndexPriceUpdate) -> Vec<RealizedVolSnapshot> {
        let price = update.value.as_f64();
        if !is_positive_finite(price) {
            return Vec::new();
        }
        self.observe_routed_price(
            EventRouteKey {
                instrument_id: update.instrument_id,
                kind: RealizedVolSubscriptionKind::IndexPrices,
            },
            price,
            update.ts_event.as_u64() / NANOS_PER_MILLI_U64,
            update.ts_init.as_u64() / NANOS_PER_MILLI_U64,
        )
    }

    pub fn refresh_surface_at(
        &mut self,
        surface_id: &str,
        now_ms: NtStrategyClockMs,
    ) -> Option<RealizedVolSnapshot> {
        let as_of_ms = {
            let state = self.surfaces.get_mut(surface_id)?;
            let latest_event_ms = state
                .engine
                .latest_accepted_event_ts()
                .or(state.last_refresh_ms)
                .unwrap_or_else(|| VenueEventMs::new(u64::MIN));
            if state
                .last_strategy_refresh_ms
                .is_some_and(|last_refresh_ms| {
                    now_ms <= last_refresh_ms
                        && state
                            .last_refresh_ms
                            .is_some_and(|last_event_ms| latest_event_ms <= last_event_ms)
                })
            {
                return state.latest_snapshot.clone();
            }
            state.last_strategy_refresh_ms = Some(now_ms);
            latest_event_ms
        };
        self.publish_surface_at(surface_id, as_of_ms, true)
    }

    fn publish_surface_at(
        &mut self,
        surface_id: &str,
        as_of_ms: VenueEventMs,
        allow_equal_timestamp: bool,
    ) -> Option<RealizedVolSnapshot> {
        let state = self.surfaces.get_mut(surface_id)?;
        if state.last_refresh_ms.is_some_and(|last_refresh_ms| {
            as_of_ms < last_refresh_ms || (!allow_equal_timestamp && as_of_ms == last_refresh_ms)
        }) {
            return state.latest_snapshot.clone();
        }
        let mut snapshot = state.engine.snapshot_at(as_of_ms.value());
        mark_unavailable_source_diagnostics(
            &mut snapshot,
            &self.new_risk_capability_unavailable_sources,
        );
        state.last_refresh_ms = Some(as_of_ms);
        state.latest_snapshot = Some(snapshot.clone());
        Some(snapshot)
    }

    fn publish_surface_after_routed_observation(
        &mut self,
        surface_id: &str,
    ) -> Option<RealizedVolSnapshot> {
        let as_of_ms = {
            let state = self.surfaces.get(surface_id)?;
            state
                .engine
                .latest_accepted_event_ts()
                .or(state.last_refresh_ms)
                .unwrap_or_else(|| VenueEventMs::new(u64::MIN))
        };
        self.publish_surface_at(surface_id, as_of_ms, true)
    }

    pub fn snapshot(&self, surface_id: &str) -> Option<RealizedVolSnapshot> {
        self.surfaces
            .get(surface_id)
            .and_then(|state| state.latest_snapshot.clone())
    }

    fn observe_routed_price(
        &mut self,
        key: EventRouteKey,
        price: f64,
        event_ts_ms: u64,
        recv_ts_ms: u64,
    ) -> Vec<RealizedVolSnapshot> {
        let Some(routes) = self.routes_by_event.get(&key).cloned() else {
            return Vec::new();
        };
        let mut updated_surface_ids = BTreeSet::new();
        for route in routes {
            let Some(state) = self.surfaces.get_mut(&route.surface_id) else {
                continue;
            };
            let _ = state.engine.observe(RealizedVolObservation {
                source_id: route.source_id,
                source_class: route.source_class,
                sample_kind: route.sample_kind,
                price,
                event_ts_ms,
                recv_ts_ms,
            });
            updated_surface_ids.insert(route.surface_id);
        }
        updated_surface_ids
            .into_iter()
            .filter_map(|surface_id| {
                self.publish_surface_after_routed_observation(surface_id.as_str())
            })
            .collect()
    }
}

fn unavailable_realized_volatility_new_risk_client_ids(
    loaded: &LoadedBoltV3Config,
) -> Result<BTreeSet<String>, String> {
    loaded
        .root
        .clients
        .iter()
        .map(|(client_id, client)| {
            new_risk_market_data_available(client_id, client)
                .map(|available| (!available).then(|| client_id.clone()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|client_ids| client_ids.into_iter().flatten().collect())
}

fn mark_unavailable_source_diagnostics(
    snapshot: &mut RealizedVolSnapshot,
    unavailable_sources: &BTreeSet<(String, String)>,
) {
    let mut capability_unavailable = false;
    for diagnostic in &mut snapshot.source_diagnostics {
        if unavailable_sources
            .contains(&(snapshot.surface_id.clone(), diagnostic.source_id.clone()))
        {
            capability_unavailable = true;
            diagnostic.counts_toward_quorum = false;
            diagnostic.status = RealizedVolSourceStatus::DiagnosticOnly;
            diagnostic.block_reason = Some(RealizedVolBlockReason::ProviderCapabilityUnavailable);
        }
    }
    if capability_unavailable
        && snapshot
            .blocked_reasons
            .contains(&RealizedVolBlockReason::QuorumNotReady)
    {
        snapshot
            .blocked_reasons
            .push(RealizedVolBlockReason::ProviderCapabilityUnavailable);
        snapshot.blocked_reasons.sort_unstable();
        snapshot.blocked_reasons.dedup();
    }
}

fn subscription_kind(
    source_class: RealizedVolSourceClass,
    sample_kind: RealizedVolSampleKind,
) -> Option<RealizedVolSubscriptionKind> {
    match (source_class, sample_kind) {
        (RealizedVolSourceClass::SpotQuote, RealizedVolSampleKind::Midpoint) => {
            Some(RealizedVolSubscriptionKind::Quotes)
        }
        (RealizedVolSourceClass::Trade, RealizedVolSampleKind::Trade) => {
            Some(RealizedVolSubscriptionKind::Trades)
        }
        (RealizedVolSourceClass::Index, RealizedVolSampleKind::Index) => {
            Some(RealizedVolSubscriptionKind::IndexPrices)
        }
        _ => None,
    }
}

fn quote_trade_index_requests(
    requests: &[RealizedVolSubscriptionRequest],
    kind: RealizedVolSubscriptionKind,
) -> Vec<(InstrumentId, Option<ClientId>)> {
    requests
        .iter()
        .filter(|request| request.kind == kind)
        .map(|request| (request.instrument_id, Some(request.data_client_id)))
        .collect()
}
