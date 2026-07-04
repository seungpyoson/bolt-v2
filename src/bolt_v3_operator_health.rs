use serde::Serialize;

use crate::{
    bolt_v3_capital_admission_state::NtDerivedCapitalAdmissionState,
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchHaltTriggerKind, KillSwitchState},
    bolt_v3_order_reject_observer_feed::BoltV3OrderRejectObserverHealthSnapshot,
    bolt_v3_reference_price::reference_price_source_is_runtime_available,
    bolt_v3_reference_price_health::ReferenceCurrentPriceHealthReport,
    bolt_v3_strategy_registration::BoltV3StrategyRegistrationSummary,
    bolt_v3_submit_admission::VENUE_TRUTH_CAPTURE_FAILURE_RESERVATION_SOURCE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OperatorHealthStatus {
    Nominal,
    Degraded,
    Halted,
    MissingInput,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3RejectObserverHealth {
    pub status: BoltV3OperatorHealthStatus,
    pub configured: bool,
    pub active_episode_count: usize,
    pub total_retry_count: u32,
    pub oldest_episode_first_ns: Option<u64>,
    pub latest_client_order_id: Option<String>,
}

impl BoltV3RejectObserverHealth {
    pub fn not_configured() -> Self {
        Self {
            status: BoltV3OperatorHealthStatus::NotConfigured,
            configured: false,
            active_episode_count: 0,
            total_retry_count: 0,
            oldest_episode_first_ns: None,
            latest_client_order_id: None,
        }
    }

    pub fn from_snapshot(snapshot: &BoltV3OrderRejectObserverHealthSnapshot) -> Self {
        Self {
            status: if snapshot.active_episode_count == 0 {
                BoltV3OperatorHealthStatus::Nominal
            } else {
                BoltV3OperatorHealthStatus::Degraded
            },
            configured: true,
            active_episode_count: snapshot.active_episode_count,
            total_retry_count: snapshot.total_retry_count,
            oldest_episode_first_ns: snapshot.oldest_episode_first_ns,
            latest_client_order_id: snapshot.latest_client_order_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3VenueTruthDivergenceHealth {
    pub source: String,
    pub source_timestamp_unix_nanos: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3VenueTruthHealth {
    pub status: BoltV3OperatorHealthStatus,
    pub kill_switch_state: String,
    pub divergence: Option<BoltV3VenueTruthDivergenceHealth>,
    pub capital_state_source: Option<String>,
    pub venue_truth_capture_suspended: bool,
}

impl BoltV3VenueTruthHealth {
    pub fn not_configured() -> Self {
        Self {
            status: BoltV3OperatorHealthStatus::NotConfigured,
            kill_switch_state: format!("{:?}", BoltV3OperatorHealthStatus::NotConfigured),
            divergence: None,
            capital_state_source: None,
            venue_truth_capture_suspended: false,
        }
    }

    pub fn from_kill_switch_and_capital_state(
        kill_switch_state: &KillSwitchState,
        capital_state: Option<&NtDerivedCapitalAdmissionState>,
    ) -> Self {
        let divergence = venue_truth_divergence_trigger(kill_switch_state).map(|trigger| {
            BoltV3VenueTruthDivergenceHealth {
                source: trigger.source.clone(),
                source_timestamp_unix_nanos: trigger.source_timestamp_unix_nanos,
                reason: trigger.reason.clone(),
            }
        });
        let capital_state_source = capital_state.map(|state| state.source.clone());
        let venue_truth_capture_suspended = capital_state_source
            .as_deref()
            .is_some_and(|source| source == VENUE_TRUTH_CAPTURE_FAILURE_RESERVATION_SOURCE);
        let status = if divergence.is_some() || !matches!(kill_switch_state, KillSwitchState::Armed)
        {
            BoltV3OperatorHealthStatus::Halted
        } else if venue_truth_capture_suspended {
            BoltV3OperatorHealthStatus::Degraded
        } else {
            BoltV3OperatorHealthStatus::Nominal
        };
        Self {
            status,
            kill_switch_state: kill_switch_state_kind_label(kill_switch_state),
            divergence,
            capital_state_source,
            venue_truth_capture_suspended,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3MissingInputSource {
    pub strategy_instance_id: String,
    pub source_id: String,
    pub asset: String,
    pub provider: String,
    pub provider_instrument: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3InputHealth {
    pub status: BoltV3OperatorHealthStatus,
    pub configured_source_count: usize,
    pub observed_source_count: usize,
    pub missing_sources: Vec<BoltV3MissingInputSource>,
}

impl BoltV3InputHealth {
    pub fn not_configured() -> Self {
        Self {
            status: BoltV3OperatorHealthStatus::NotConfigured,
            configured_source_count: 0,
            observed_source_count: 0,
            missing_sources: Vec::new(),
        }
    }

    pub fn from_reference_current_price_report(report: &ReferenceCurrentPriceHealthReport) -> Self {
        let configured_source_count = report.source_update_observations.len();
        let observed_source_count = report
            .source_update_observations
            .iter()
            .filter(|observation| observation.is_observed())
            .count();
        let missing_sources = report
            .source_update_observations
            .iter()
            .filter(|observation| !observation.is_observed())
            .map(|observation| BoltV3MissingInputSource {
                strategy_instance_id: observation.strategy_instance_id.clone(),
                source_id: observation.source_id.clone(),
                asset: observation.asset.clone(),
                provider: observation.provider.clone(),
                provider_instrument: observation.provider_instrument.clone(),
                reason: observation.reason.clone(),
            })
            .collect::<Vec<_>>();
        let status = if configured_source_count == 0 {
            BoltV3OperatorHealthStatus::NotConfigured
        } else if missing_sources.is_empty() {
            BoltV3OperatorHealthStatus::Nominal
        } else {
            BoltV3OperatorHealthStatus::MissingInput
        };
        Self {
            status,
            configured_source_count,
            observed_source_count,
            missing_sources,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3OperatorHealthSurface {
    pub reject_observer: BoltV3RejectObserverHealth,
    pub venue_truth: BoltV3VenueTruthHealth,
    pub input_health: BoltV3InputHealth,
}

impl BoltV3OperatorHealthSurface {
    pub fn not_configured() -> Self {
        Self {
            reject_observer: BoltV3RejectObserverHealth::not_configured(),
            venue_truth: BoltV3VenueTruthHealth::not_configured(),
            input_health: BoltV3InputHealth::not_configured(),
        }
    }

    pub fn from_parts(
        reject_observer: Option<&BoltV3OrderRejectObserverHealthSnapshot>,
        kill_switch_state: &KillSwitchState,
        capital_state: Option<&NtDerivedCapitalAdmissionState>,
        input_health: Option<BoltV3InputHealth>,
    ) -> Self {
        Self {
            reject_observer: reject_observer.map_or_else(
                BoltV3RejectObserverHealth::not_configured,
                BoltV3RejectObserverHealth::from_snapshot,
            ),
            venue_truth: BoltV3VenueTruthHealth::from_kill_switch_and_capital_state(
                kill_switch_state,
                capital_state,
            ),
            input_health: input_health.unwrap_or_else(BoltV3InputHealth::not_configured),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3RuntimeSourceAnnouncement {
    pub strategy_instance_id: String,
    pub strategy_archetype: String,
    pub registered_strategy_id: String,
    pub signal_sources: Vec<BoltV3SignalSourceAnnouncement>,
    pub resolution_source: Option<BoltV3DataInstrumentSourceAnnouncement>,
    pub reference_current_price_sources: Vec<BoltV3ReferencePriceSourceAnnouncement>,
    pub realized_volatility_sources: Vec<BoltV3RealizedVolatilitySourceAnnouncement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3SignalSourceAnnouncement {
    pub role: String,
    pub data_client_id: String,
    pub instrument_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3DataInstrumentSourceAnnouncement {
    pub data_client_id: String,
    pub instrument_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3ReferencePriceSourceAnnouncement {
    pub source_id: String,
    pub provider: String,
    pub client_id: String,
    pub provider_instrument: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3RealizedVolatilitySourceAnnouncement {
    pub surface_id: String,
    pub source_id: String,
    pub data_client_id: String,
    pub instrument_id: String,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
}

pub fn runtime_source_announcements(
    loaded: &LoadedBoltV3Config,
    summary: &BoltV3StrategyRegistrationSummary,
) -> Result<Vec<BoltV3RuntimeSourceAnnouncement>, String> {
    summary
        .registered
        .iter()
        .map(|registered| {
            let strategy = loaded
                .strategies
                .iter()
                .find(|strategy| {
                    strategy.config.strategy_instance_id == registered.strategy_instance_id
                })
                .ok_or_else(|| {
                    format!(
                        "registered strategy `{}` is missing from loaded config",
                        registered.strategy_instance_id
                    )
                })?;
            let signal_sources = strategy
                .config
                .signal_data
                .iter()
                .map(|(role, source)| BoltV3SignalSourceAnnouncement {
                    role: role.clone(),
                    data_client_id: source.data_client_id.to_string(),
                    instrument_id: source.instrument_id.to_string(),
                })
                .collect();
            let resolution_source = strategy.config.resolution_data.as_ref().map(|source| {
                BoltV3DataInstrumentSourceAnnouncement {
                    data_client_id: source.data_client_id.to_string(),
                    instrument_id: source.instrument_id.to_string(),
                }
            });
            let reference_current_price_sources = strategy
                .config
                .reference_current_price
                .as_ref()
                .map(|reference| {
                    reference
                        .source_order
                        .iter()
                        .map(|source_id| {
                            let source = reference.sources.get(source_id).ok_or_else(|| {
                                format!(
                                    "strategy `{}` lists reference source `{source_id}` without a source block",
                                    strategy.config.strategy_instance_id
                                )
                            })?;
                            if !reference_price_source_is_runtime_available(reference, source) {
                                return Ok(None);
                            }
                            Ok(Some(BoltV3ReferencePriceSourceAnnouncement {
                                source_id: source_id.clone(),
                                provider: source.provider.as_str().to_string(),
                                client_id: source.client_id.to_string(),
                                provider_instrument: source
                                    .instrument_id
                                    .clone()
                                    .or_else(|| source.symbol.clone()),
                                required: source.required,
                            }))
                        })
                        .filter_map(|source| match source {
                            Ok(Some(announcement)) => Some(Ok(announcement)),
                            Ok(None) => None,
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
                .transpose()?
                .unwrap_or_else(Vec::new);
            let realized_volatility_sources =
                realized_volatility_source_announcements(loaded, strategy)?;
            Ok(BoltV3RuntimeSourceAnnouncement {
                strategy_instance_id: registered.strategy_instance_id.clone(),
                strategy_archetype: registered.strategy_archetype.as_str().to_string(),
                registered_strategy_id: registered.registered_strategy_id.clone(),
                signal_sources,
                resolution_source,
                reference_current_price_sources,
                realized_volatility_sources,
            })
        })
        .collect()
}

fn realized_volatility_source_announcements(
    loaded: &LoadedBoltV3Config,
    strategy: &crate::bolt_v3_config::LoadedStrategy,
) -> Result<Vec<BoltV3RealizedVolatilitySourceAnnouncement>, String> {
    let Some(surface_id) = strategy.config.realized_volatility_surface_id.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(surfaces) = loaded.root.realized_volatility_surfaces.as_ref() else {
        return Err(format!(
            "strategy `{}` declares realized-volatility surface `{surface_id}` but root has no surfaces",
            strategy.config.strategy_instance_id
        ));
    };
    let Some(surface) = surfaces.get(surface_id) else {
        return Err(format!(
            "strategy `{}` declares unknown realized-volatility surface `{surface_id}`",
            strategy.config.strategy_instance_id
        ));
    };
    Ok(surface
        .sources
        .iter()
        .map(|source| BoltV3RealizedVolatilitySourceAnnouncement {
            surface_id: surface_id.clone(),
            source_id: source.source_id.clone(),
            data_client_id: source.data_client_id.to_string(),
            instrument_id: source.instrument_id.to_string(),
            enabled: source.enabled,
            counts_toward_quorum: source.counts_toward_quorum,
        })
        .collect())
}

fn venue_truth_divergence_trigger(state: &KillSwitchState) -> Option<&KillSwitchHaltTrigger> {
    match state {
        KillSwitchState::Halting { trigger, .. } | KillSwitchState::Halted { trigger, .. }
            if trigger.kind == KillSwitchHaltTriggerKind::VenueTruthDivergence =>
        {
            Some(trigger)
        }
        _ => None,
    }
}

fn kill_switch_state_kind_label(state: &KillSwitchState) -> String {
    format!("{:?}", state.kind())
}
