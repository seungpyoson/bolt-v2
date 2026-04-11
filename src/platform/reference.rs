use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::ReferenceVenueEntry;

#[derive(Debug, Clone, PartialEq)]
pub enum VenueHealth {
    Healthy,
    Disabled { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueKind {
    Orderbook,
    Oracle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceObservation {
    Orderbook {
        venue_name: String,
        instrument_id: String,
        bid: f64,
        ask: f64,
        ts_ms: u64,
        observed_ts_ms: u64,
    },
    Oracle {
        venue_name: String,
        instrument_id: String,
        price: f64,
        ts_ms: u64,
        observed_ts_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveVenueState {
    pub venue_name: String,
    pub base_weight: f64,
    pub effective_weight: f64,
    pub stale: bool,
    pub health: VenueHealth,
    pub observed_ts_ms: Option<u64>,
    pub venue_kind: VenueKind,
    pub observed_price: Option<f64>,
    pub observed_bid: Option<f64>,
    pub observed_ask: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceSnapshot {
    pub ts_ms: u64,
    pub topic: String,
    pub fair_value: Option<f64>,
    pub confidence: f64,
    pub venues: Vec<EffectiveVenueState>,
}

pub fn fuse_reference_snapshot(
    topic: &str,
    now_ms: u64,
    venue_cfgs: &[ReferenceVenueEntry],
    latest: &BTreeMap<String, ReferenceObservation>,
    disabled: &BTreeMap<String, String>,
) -> ReferenceSnapshot {
    let mut weighted_price_sum = 0.0;
    let mut total_effective_weight = 0.0;
    let mut total_base_weight = 0.0;
    let mut venues = Vec::with_capacity(venue_cfgs.len());

    for venue in venue_cfgs {
        total_base_weight += venue.base_weight;

        let observation = latest
            .get(&venue.name)
            .filter(|observation| observation.matches_identity(venue));
        let missing_observation_reason = observation.is_none().then(missing_observation_reason);
        let observed_ts_ms = observation.map(|observation| observation.observed_ts_ms());
        let venue_kind = observation
            .map(ReferenceObservation::venue_kind)
            .unwrap_or_else(|| VenueKind::from_reference_entry(venue));
        let observed_price = observation.map(ReferenceObservation::observed_price);
        let observed_bid = observation.and_then(ReferenceObservation::observed_bid);
        let observed_ask = observation.and_then(ReferenceObservation::observed_ask);
        let age_ms = observation.map(|observation| now_ms.saturating_sub(observation.ts_ms()));
        let auto_disabled_reason = age_ms
            .filter(|age_ms| *age_ms > venue.disable_after_ms)
            .map(auto_disable_reason);
        let stale = missing_observation_reason.is_some()
            || age_ms
                .map(|age_ms| age_ms > venue.stale_after_ms || auto_disabled_reason.is_some())
                .unwrap_or(false);

        let health = match disabled.get(&venue.name) {
            Some(reason) => VenueHealth::Disabled {
                reason: reason.clone(),
            },
            None => match missing_observation_reason.or(auto_disabled_reason) {
                Some(reason) => VenueHealth::Disabled { reason },
                None => VenueHealth::Healthy,
            },
        };

        let enabled = matches!(health, VenueHealth::Healthy) && !stale;
        let effective_weight = if enabled && observed_price.is_some() {
            venue.base_weight
        } else {
            0.0
        };

        if let Some(price) = observed_price {
            weighted_price_sum += price * effective_weight;
        }
        total_effective_weight += effective_weight;

        venues.push(EffectiveVenueState {
            venue_name: venue.name.clone(),
            base_weight: venue.base_weight,
            effective_weight,
            stale,
            health,
            observed_ts_ms,
            venue_kind,
            observed_price,
            observed_bid,
            observed_ask,
        });
    }

    let fair_value = if total_effective_weight > 0.0 {
        Some(weighted_price_sum / total_effective_weight)
    } else {
        None
    };
    let confidence = if total_base_weight > 0.0 {
        total_effective_weight / total_base_weight
    } else {
        0.0
    };

    ReferenceSnapshot {
        ts_ms: now_ms,
        topic: topic.to_string(),
        fair_value,
        confidence,
        venues,
    }
}

fn auto_disable_reason(age_ms: u64) -> String {
    format!("auto-disabled after {age_ms}ms without a fresh reference update")
}

fn missing_observation_reason() -> String {
    "no reference update received yet".to_string()
}

impl ReferenceObservation {
    fn matches_identity(&self, venue: &ReferenceVenueEntry) -> bool {
        match self {
            Self::Orderbook {
                venue_name,
                instrument_id,
                ..
            }
            | Self::Oracle {
                venue_name,
                instrument_id,
                ..
            } => venue_name == &venue.name && instrument_id == &venue.instrument_id,
        }
    }

    fn observed_price(&self) -> f64 {
        match self {
            Self::Orderbook { bid, ask, .. } => (bid + ask) / 2.0,
            Self::Oracle { price, .. } => *price,
        }
    }

    fn observed_bid(&self) -> Option<f64> {
        match self {
            Self::Orderbook { bid, .. } => Some(*bid),
            Self::Oracle { .. } => None,
        }
    }

    fn observed_ask(&self) -> Option<f64> {
        match self {
            Self::Orderbook { ask, .. } => Some(*ask),
            Self::Oracle { .. } => None,
        }
    }

    fn ts_ms(&self) -> u64 {
        match self {
            Self::Orderbook { ts_ms, .. } | Self::Oracle { ts_ms, .. } => *ts_ms,
        }
    }

    fn observed_ts_ms(&self) -> u64 {
        match self {
            Self::Orderbook { observed_ts_ms, .. } | Self::Oracle { observed_ts_ms, .. } => {
                *observed_ts_ms
            }
        }
    }

    fn venue_kind(&self) -> VenueKind {
        match self {
            Self::Orderbook { .. } => VenueKind::Orderbook,
            Self::Oracle { .. } => VenueKind::Oracle,
        }
    }
}

impl VenueKind {
    fn from_reference_entry(venue: &ReferenceVenueEntry) -> Self {
        match venue.kind {
            crate::config::ReferenceVenueKind::Chainlink => Self::Oracle,
            _ => Self::Orderbook,
        }
    }
}
