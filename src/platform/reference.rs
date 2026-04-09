use std::collections::BTreeMap;

use crate::config::ReferenceVenueEntry;

#[derive(Debug, Clone, PartialEq)]
pub enum VenueHealth {
    Healthy,
    Disabled { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceObservation {
    Orderbook {
        venue_name: String,
        instrument_id: String,
        bid: f64,
        ask: f64,
        ts_ms: u64,
    },
    Oracle {
        venue_name: String,
        instrument_id: String,
        price: f64,
        ts_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveVenueState {
    pub venue_name: String,
    pub base_weight: f64,
    pub effective_weight: f64,
    pub stale: bool,
    pub health: VenueHealth,
    pub observed_price: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceSnapshot {
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

        let observation = latest.get(&venue.name);
        let observed_price = observation.map(ReferenceObservation::observed_price);
        let stale = observation
            .map(|observation| now_ms.saturating_sub(observation.ts_ms()) > venue.stale_after_ms)
            .unwrap_or(false);

        let health = match disabled.get(&venue.name) {
            Some(reason) => VenueHealth::Disabled {
                reason: reason.clone(),
            },
            None => VenueHealth::Healthy,
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
            observed_price,
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
        topic: topic.to_string(),
        fair_value,
        confidence,
        venues,
    }
}

impl ReferenceObservation {
    fn observed_price(&self) -> f64 {
        match self {
            Self::Orderbook { bid, ask, .. } => (bid + ask) / 2.0,
            Self::Oracle { price, .. } => *price,
        }
    }

    fn ts_ms(&self) -> u64 {
        match self {
            Self::Orderbook { ts_ms, .. } | Self::Oracle { ts_ms, .. } => *ts_ms,
        }
    }
}
