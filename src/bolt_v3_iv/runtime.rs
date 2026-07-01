use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    config::{IvProfile, IvRootConfig, IvSourceNtProvenance},
    error::IvRejectReason,
    health::{IvSourceHealth, IvSourceHealthState},
    ingest::{
        IvAggregateGreeksPayload, IvAggregateIvValue, IvBasisValue, IvCustomIvPayload,
        IvGreekValues, IvIngestEvent, IvOptionChainQuotePayload, IvOptionChainSlicePayload,
        IvOptionChainStrikePayload, IvOptionGreeksPayload, IvRawEvent, IvRawPayload,
    },
    query::{IvQueryState, IvQueryStateHandle},
    selector::IvSelector,
    store::{IvRetentionPolicy, IvStore, IvStoreError},
    subscription::{IvRuntimeOperation, IvSubscriptionPlan},
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};
use nautilus_model::data::{CustomData, HasTsInit};

const INITIAL_SUBSCRIPTION_GENERATION: u64 = 0;
const CARGO_LOCK_TEXT: &str = include_str!("../../Cargo.lock");
static CARGO_PINNED_NT_REVISION: OnceLock<String> = OnceLock::new();

pub fn cargo_pinned_nt_revision() -> &'static str {
    CARGO_PINNED_NT_REVISION
        .get_or_init(|| {
            cargo_pinned_nt_revision_from_lock(CARGO_LOCK_TEXT)
                .expect("Cargo.lock must contain the pinned NautilusTrader revision")
        })
        .as_str()
}

pub trait IvRuntimeBindingAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvRuntimeBindingError {
    pub profile_id: String,
    pub source_id: String,
    pub operation: IvRuntimeOperation,
    pub reason: IvRejectReason,
    pub message: String,
}

impl IvRuntimeBindingError {
    pub fn subscription_failed(plan: &IvSubscriptionPlan, message: String) -> Self {
        Self {
            profile_id: plan.profile_id.clone(),
            source_id: plan.source_id.clone(),
            operation: plan.operation,
            reason: IvRejectReason::SubscriptionFailed,
            message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvRuntimePlanOutcome {
    pub plan: IvSubscriptionPlan,
    pub source_health: IvSourceHealth,
    pub error: Option<IvRuntimeBindingError>,
}

#[derive(Debug, Clone)]
pub struct IvRuntimeEngine {
    inner: Arc<RwLock<IvRuntimeEngineState>>,
}

#[derive(Debug)]
struct IvRuntimeEngineState {
    profile_states: BTreeMap<String, IvQueryStateHandle>,
    retention_policies: BTreeMap<String, IvRetentionPolicy>,
    profile_sources: BTreeMap<String, BTreeMap<String, IvRuntimeSourceConfig>>,
}

#[derive(Debug, Clone, PartialEq)]
struct IvRuntimeSourceConfig {
    source_kind: IvSourceKind,
    selector_fingerprint: String,
    subscription_generation: u64,
    max_source_event_age_ns: Option<u64>,
    max_source_event_future_skew_ns: u64,
    accepted_conventions: BTreeSet<String>,
    nt_provenance: IvSourceNtProvenance,
    selector: IvSelector,
}

#[derive(Debug, Clone, Copy)]
struct IvRuntimeRejectionRecord<'a> {
    profile_id: &'a str,
    source_id: &'a str,
    subscription_generation: u64,
    ts_event_ns: UnixNanos,
    reason: IvRejectReason,
    mark_rejected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvRuntimeEngineError {
    DuplicateProfileId {
        profile_id: String,
    },
    UnknownProfileId {
        profile_id: String,
    },
    IngestRejected {
        profile_id: String,
        source_id: String,
        reason: IvRejectReason,
    },
    SubscriptionPlanFailed {
        profile_id: String,
        source_id: String,
        reason: IvRejectReason,
    },
    Store(IvStoreError),
}

impl IvRuntimeEngine {
    pub fn from_iv_root(root: &IvRootConfig) -> Result<Self, IvRuntimeEngineError> {
        let mut profile_states = BTreeMap::new();
        let mut retention_policies = BTreeMap::new();
        let mut profile_sources = BTreeMap::new();
        for profile in &root.profiles {
            let state = query_state_from_profile(profile);
            if profile_states
                .insert(profile.profile_id.clone(), IvQueryStateHandle::new(state))
                .is_some()
            {
                return Err(IvRuntimeEngineError::DuplicateProfileId {
                    profile_id: profile.profile_id.clone(),
                });
            }
            retention_policies.insert(
                profile.profile_id.clone(),
                retention_policy_from_profile(profile),
            );
            profile_sources.insert(
                profile.profile_id.clone(),
                runtime_sources_from_profile(profile),
            );
        }

        Ok(Self {
            inner: Arc::new(RwLock::new(IvRuntimeEngineState {
                profile_states,
                retention_policies,
                profile_sources,
            })),
        })
    }

    fn read_inner(&self) -> RwLockReadGuard<'_, IvRuntimeEngineState> {
        self.inner
            .read()
            .expect("IV runtime engine state lock poisoned")
    }

    fn write_inner(&self) -> RwLockWriteGuard<'_, IvRuntimeEngineState> {
        self.inner
            .write()
            .expect("IV runtime engine state lock poisoned")
    }

    pub fn apply_iv_root_reload(
        &mut self,
        root: &IvRootConfig,
    ) -> Result<(), IvRuntimeEngineError> {
        let mut next_profile_ids = BTreeSet::new();
        for profile in &root.profiles {
            if !next_profile_ids.insert(profile.profile_id.clone()) {
                return Err(IvRuntimeEngineError::DuplicateProfileId {
                    profile_id: profile.profile_id.clone(),
                });
            }
        }

        let mut inner = self.write_inner();
        let previous_profile_sources = inner.profile_sources.clone();
        for profile in &root.profiles {
            let next_sources = runtime_sources_from_profile(profile);
            if let Some(state) = inner.profile_states.get(&profile.profile_id) {
                if let Some(previous_sources) = previous_profile_sources.get(&profile.profile_id) {
                    state.mark_sources_removed(
                        &profile.profile_id,
                        &removed_source_generations(previous_sources, &next_sources),
                    );
                }
                state.set_projection_policies(profile.projection_policies.clone());
                state.set_input_bounds(profile.input_bounds.clone());
                state.set_interpolation_policies(profile.interpolation_policies.clone());
                state.set_fallback_policies(profile.fallback_policies.clone());
                state.set_quorum_policies(profile.quorum_policies.clone());
                state.set_helper_policies(profile.helper_policies.clone());
                state.set_derived_input_policies(profile.derived_input_policies.clone());
                state.set_derived_inputs(runtime_derived_inputs_from_profile(profile));
                state.set_current_subscription_generations(current_generations_from_profile(
                    profile,
                ));
            } else {
                inner.profile_states.insert(
                    profile.profile_id.clone(),
                    IvQueryStateHandle::new(query_state_from_profile(profile)),
                );
            }

            inner.retention_policies.insert(
                profile.profile_id.clone(),
                retention_policy_from_profile(profile),
            );
            inner
                .profile_sources
                .insert(profile.profile_id.clone(), next_sources);
        }

        for (profile_id, previous_sources) in &previous_profile_sources {
            if next_profile_ids.contains(profile_id) {
                continue;
            }
            if let Some(state) = inner.profile_states.get(profile_id) {
                state.mark_sources_removed(
                    profile_id,
                    &source_generations_from_runtime_sources(previous_sources),
                );
                state.set_current_subscription_generations(BTreeMap::new());
            }
        }

        inner
            .retention_policies
            .retain(|profile_id, _| next_profile_ids.contains(profile_id));
        inner
            .profile_sources
            .retain(|profile_id, _| next_profile_ids.contains(profile_id));
        inner
            .profile_states
            .retain(|profile_id, _| next_profile_ids.contains(profile_id));

        Ok(())
    }

    pub fn state_for_profile(&self, profile_id: &str) -> Option<IvQueryStateHandle> {
        self.read_inner().profile_states.get(profile_id).cloned()
    }

    pub fn source_health(&self, profile_id: &str, source_id: &str) -> Option<IvSourceHealth> {
        self.state_for_profile(profile_id)
            .and_then(|state| state.source_health_for(profile_id, source_id))
    }

    pub fn source_nt_provenance(
        &self,
        profile_id: &str,
        source_id: &str,
    ) -> Option<IvSourceNtProvenance> {
        self.read_inner()
            .profile_sources
            .get(profile_id)
            .and_then(|sources| sources.get(source_id))
            .map(|source| source.nt_provenance.clone())
    }

    pub fn ingest_nt_option_greeks(
        &self,
        profile_id: &str,
        source_id: &str,
        option_greeks: &nautilus_model::data::OptionGreeks,
        received_ts_ns: UnixNanos,
    ) -> Result<IvRawEvent, IvRuntimeEngineError> {
        let ts_event_ns = UnixNanos::new(option_greeks.ts_event.as_u64());
        let source = &self.runtime_source_config(profile_id, source_id, ts_event_ns)?;
        if source.source_kind != IvSourceKind::OptionGreeks
            || !source.selector_matches_option_greeks(&option_greeks.instrument_id.to_string())
        {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        }
        if !source.accepts_nt_greeks_convention(option_greeks.convention) {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::UnsupportedConvention,
                false,
            ));
        }
        self.ingest_event(IvIngestEvent {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            source_kind: source.source_kind,
            selector_fingerprint: source.selector_fingerprint.clone(),
            nt_revision: source.nt_provenance.nt_revision.clone(),
            nt_evidence_path: source.nt_provenance.nt_evidence_path.clone(),
            nt_symbol: source.nt_provenance.nt_symbol.clone(),
            ts_event_ns,
            ts_init_ns: Some(UnixNanos::new(option_greeks.ts_init.as_u64())),
            received_ts_ns,
            subscription_generation: source.subscription_generation,
            source_health_state: IvSourceHealthState::Active,
            payload: IvRawPayload::OptionGreeks(IvOptionGreeksPayload {
                instrument_id: option_greeks.instrument_id.to_string(),
                convention: IvConvention::Named(nt_greeks_convention_name(
                    option_greeks.convention,
                )),
                basis_values: option_greeks_basis_values(option_greeks),
                greeks: IvGreekValues {
                    delta: Some(option_greeks.greeks.delta),
                    gamma: Some(option_greeks.greeks.gamma),
                    vega: Some(option_greeks.greeks.vega),
                    theta: Some(option_greeks.greeks.theta),
                    rho: Some(option_greeks.greeks.rho),
                },
                underlying_price: option_greeks.underlying_price,
                open_interest: option_greeks.open_interest,
            }),
        })
    }

    pub fn ingest_nt_option_chain_slice(
        &self,
        profile_id: &str,
        source_id: &str,
        option_chain: &nautilus_model::data::OptionChainSlice,
        received_ts_ns: UnixNanos,
    ) -> Result<IvRawEvent, IvRuntimeEngineError> {
        let ts_event_ns = UnixNanos::new(option_chain.ts_event.as_u64());
        let source = &self.runtime_source_config(profile_id, source_id, ts_event_ns)?;
        if source.source_kind != IvSourceKind::OptionChain
            || !source.selector_matches_option_chain(&option_chain.series_id.to_string())
        {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        }
        if !option_chain_conventions_supported(source, option_chain) {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::UnsupportedConvention,
                false,
            ));
        }
        self.ingest_event(IvIngestEvent {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            source_kind: source.source_kind,
            selector_fingerprint: source.selector_fingerprint.clone(),
            nt_revision: source.nt_provenance.nt_revision.clone(),
            nt_evidence_path: source.nt_provenance.nt_evidence_path.clone(),
            nt_symbol: source.nt_provenance.nt_symbol.clone(),
            ts_event_ns,
            ts_init_ns: Some(UnixNanos::new(option_chain.ts_init.as_u64())),
            received_ts_ns,
            subscription_generation: source.subscription_generation,
            source_health_state: IvSourceHealthState::Active,
            payload: IvRawPayload::OptionChainSlice(IvOptionChainSlicePayload {
                series_id: option_chain.series_id.to_string(),
                surface_selector: source.selector_fingerprint.clone(),
                atm_strike: option_chain.atm_strike.map(|strike| strike.as_f64()),
                calls: option_chain
                    .calls
                    .iter()
                    .map(|(strike, data)| option_chain_strike_payload(strike.as_f64(), data))
                    .collect(),
                puts: option_chain
                    .puts
                    .iter()
                    .map(|(strike, data)| option_chain_strike_payload(strike.as_f64(), data))
                    .collect(),
            }),
        })
    }

    pub fn ingest_nt_aggregate_greeks_custom_data(
        &self,
        profile_id: &str,
        source_id: &str,
        custom_data: &CustomData,
        received_ts_ns: UnixNanos,
    ) -> Result<IvRawEvent, IvRuntimeEngineError> {
        let ts_event_ns = UnixNanos::new(custom_data.data.ts_event().as_u64());
        let source = &self.runtime_source_config(profile_id, source_id, ts_event_ns)?;
        let IvSelector::SourceAggregateGreeks {
            aggregate_key,
            underlying_selectors,
            delta_field,
            gamma_field,
            vega_field,
            theta_field,
            rho_field,
            iv_field,
            iv_basis,
            iv_convention,
            ..
        } = &source.selector
        else {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        };
        if source.source_kind != IvSourceKind::AggregateGreeks
            || custom_data.data_type.type_name() != aggregate_key
        {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        }
        let payload = custom_data_json_value(custom_data).map_err(|reason| {
            self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                reason,
                false,
            )
        })?;
        let greeks = IvGreekValues {
            delta: Some(
                custom_data_field_f64(&payload, delta_field).map_err(|reason| {
                    self.reject_runtime_source_event(
                        profile_id,
                        source_id,
                        source,
                        ts_event_ns,
                        reason,
                        false,
                    )
                })?,
            ),
            gamma: Some(
                custom_data_field_f64(&payload, gamma_field).map_err(|reason| {
                    self.reject_runtime_source_event(
                        profile_id,
                        source_id,
                        source,
                        ts_event_ns,
                        reason,
                        false,
                    )
                })?,
            ),
            vega: Some(
                custom_data_field_f64(&payload, vega_field).map_err(|reason| {
                    self.reject_runtime_source_event(
                        profile_id,
                        source_id,
                        source,
                        ts_event_ns,
                        reason,
                        false,
                    )
                })?,
            ),
            theta: Some(
                custom_data_field_f64(&payload, theta_field).map_err(|reason| {
                    self.reject_runtime_source_event(
                        profile_id,
                        source_id,
                        source,
                        ts_event_ns,
                        reason,
                        false,
                    )
                })?,
            ),
            rho: Some(
                custom_data_field_f64(&payload, rho_field).map_err(|reason| {
                    self.reject_runtime_source_event(
                        profile_id,
                        source_id,
                        source,
                        ts_event_ns,
                        reason,
                        false,
                    )
                })?,
            ),
        };
        let aggregate_iv = match (iv_field, iv_basis, iv_convention) {
            (Some(iv_field), Some(iv_basis), Some(iv_convention)) => Some(IvAggregateIvValue {
                basis: *iv_basis,
                value: custom_data_field_f64(&payload, iv_field).map_err(|reason| {
                    self.reject_runtime_source_event(
                        profile_id,
                        source_id,
                        source,
                        ts_event_ns,
                        reason,
                        false,
                    )
                })?,
                convention: iv_convention.clone(),
            }),
            _ => None,
        };

        self.ingest_event(IvIngestEvent {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            source_kind: source.source_kind,
            selector_fingerprint: source.selector_fingerprint.clone(),
            nt_revision: source.nt_provenance.nt_revision.clone(),
            nt_evidence_path: source.nt_provenance.nt_evidence_path.clone(),
            nt_symbol: source.nt_provenance.nt_symbol.clone(),
            ts_event_ns,
            ts_init_ns: Some(UnixNanos::new(custom_data.ts_init().as_u64())),
            received_ts_ns,
            subscription_generation: source.subscription_generation,
            source_health_state: IvSourceHealthState::Active,
            payload: IvRawPayload::AggregateGreeks(IvAggregateGreeksPayload {
                aggregate_key: aggregate_key.clone(),
                underlying_selectors: underlying_selectors.clone(),
                greeks,
                aggregate_iv,
                nt_custom_data_json: Some(payload),
            }),
        })
    }

    pub fn ingest_nt_custom_iv_data(
        &self,
        profile_id: &str,
        source_id: &str,
        custom_data: &CustomData,
        received_ts_ns: UnixNanos,
    ) -> Result<IvRawEvent, IvRuntimeEngineError> {
        let ts_event_ns = UnixNanos::new(custom_data.data.ts_event().as_u64());
        let source = &self.runtime_source_config(profile_id, source_id, ts_event_ns)?;
        let IvSelector::SourceCustomImpliedVolatility {
            custom_iv_data_type,
            custom_iv_data_fields,
            ..
        } = &source.selector
        else {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        };
        if source.source_kind != IvSourceKind::CustomImpliedVolatility
            || custom_data.data_type.type_name() != custom_iv_data_type
        {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        }
        let Some(value_field) = custom_iv_data_fields.first() else {
            return Err(self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                IvRejectReason::InvalidIvValue,
                false,
            ));
        };
        let payload = custom_data_json_value(custom_data).map_err(|reason| {
            self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                reason,
                false,
            )
        })?;
        let value = custom_data_field_f64(&payload, value_field).map_err(|reason| {
            self.reject_runtime_source_event(
                profile_id,
                source_id,
                source,
                ts_event_ns,
                reason,
                false,
            )
        })?;

        self.ingest_event(IvIngestEvent {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            source_kind: source.source_kind,
            selector_fingerprint: source.selector_fingerprint.clone(),
            nt_revision: source.nt_provenance.nt_revision.clone(),
            nt_evidence_path: source.nt_provenance.nt_evidence_path.clone(),
            nt_symbol: source.nt_provenance.nt_symbol.clone(),
            ts_event_ns,
            ts_init_ns: Some(UnixNanos::new(custom_data.ts_init().as_u64())),
            received_ts_ns,
            subscription_generation: source.subscription_generation,
            source_health_state: IvSourceHealthState::Active,
            payload: IvRawPayload::CustomImpliedVolatility(IvCustomIvPayload {
                iv_evidence_kind: custom_iv_data_type.clone(),
                value,
                nt_custom_data_json: Some(payload),
            }),
        })
    }

    pub fn ingest_event(&self, event: IvIngestEvent) -> Result<IvRawEvent, IvRuntimeEngineError> {
        let profile_id = event.profile_id.clone();
        let state = self.state_for_profile(&profile_id).ok_or_else(|| {
            IvRuntimeEngineError::UnknownProfileId {
                profile_id: profile_id.clone(),
            }
        })?;
        self.validate_ingest_event(&event)?;
        let event_for_error = event.clone();
        let ingest_result = state.ingest_event(event);
        let retention_policy = self
            .read_inner()
            .retention_policies
            .get(&profile_id)
            .copied();
        if let Some(policy) = retention_policy {
            state.enforce_retention(&policy);
        }
        match ingest_result {
            Ok(raw_event) => {
                state.upsert_source_health(active_source_health_for_event(&event_for_error));
                Ok(raw_event)
            }
            Err(error) => {
                let reason = error.reject_reason();
                Err(self.reject_ingest_event(
                    &event_for_error,
                    event_for_error.subscription_generation,
                    reason,
                    false,
                ))
            }
        }
    }

    fn validate_ingest_event(&self, event: &IvIngestEvent) -> Result<(), IvRuntimeEngineError> {
        let source = {
            let inner = self.read_inner();
            let Some(profile_sources) = inner.profile_sources.get(&event.profile_id) else {
                return Err(IvRuntimeEngineError::UnknownProfileId {
                    profile_id: event.profile_id.clone(),
                });
            };
            profile_sources.get(&event.source_id).cloned()
        };
        let Some(source) = source else {
            return Err(self.reject_ingest_event(
                event,
                event.subscription_generation,
                IvRejectReason::SourceNotConfigured,
                true,
            ));
        };
        if source.source_kind != event.source_kind {
            return Err(self.reject_ingest_event(
                event,
                source.subscription_generation,
                IvRejectReason::UnsupportedSourceKind,
                false,
            ));
        }
        if source.selector_fingerprint != event.selector_fingerprint {
            return Err(self.reject_ingest_event(
                event,
                source.subscription_generation,
                IvRejectReason::SelectorProductMismatch,
                false,
            ));
        }
        if source.subscription_generation != event.subscription_generation {
            return Err(self.reject_stale_generation_event(event, source.subscription_generation));
        }
        if event
            .ts_event_ns
            .get()
            .saturating_sub(event.received_ts_ns.get())
            > source.max_source_event_future_skew_ns
        {
            return Err(self.reject_ingest_event(
                event,
                source.subscription_generation,
                IvRejectReason::ClockSkew,
                false,
            ));
        }
        if source.max_source_event_age_ns.is_some_and(|max_age_ns| {
            event
                .received_ts_ns
                .get()
                .saturating_sub(event.ts_event_ns.get())
                > max_age_ns
        }) {
            return Err(self.reject_ingest_event(
                event,
                source.subscription_generation,
                IvRejectReason::StaleData,
                false,
            ));
        }
        Ok(())
    }

    fn runtime_source_config(
        &self,
        profile_id: &str,
        source_id: &str,
        ts_event_ns: UnixNanos,
    ) -> Result<IvRuntimeSourceConfig, IvRuntimeEngineError> {
        let source = {
            let inner = self.read_inner();
            let profile_sources = inner.profile_sources.get(profile_id).ok_or_else(|| {
                IvRuntimeEngineError::UnknownProfileId {
                    profile_id: profile_id.to_string(),
                }
            })?;
            profile_sources.get(source_id).cloned()
        };
        source.ok_or_else(|| {
            self.reject_profile_source_event(
                profile_id,
                source_id,
                INITIAL_SUBSCRIPTION_GENERATION,
                ts_event_ns,
                IvRejectReason::SourceNotConfigured,
                true,
            )
        })
    }

    fn reject_ingest_event(
        &self,
        event: &IvIngestEvent,
        health_generation: u64,
        reason: IvRejectReason,
        mark_rejected: bool,
    ) -> IvRuntimeEngineError {
        if let Some(state) = self.state_for_profile(&event.profile_id) {
            self.record_source_rejection_with_retention(
                &state,
                IvRuntimeRejectionRecord {
                    profile_id: &event.profile_id,
                    source_id: &event.source_id,
                    subscription_generation: health_generation,
                    ts_event_ns: event.ts_event_ns,
                    reason,
                    mark_rejected,
                },
            );
        }
        IvRuntimeEngineError::IngestRejected {
            profile_id: event.profile_id.clone(),
            source_id: event.source_id.clone(),
            reason,
        }
    }

    fn reject_stale_generation_event(
        &self,
        event: &IvIngestEvent,
        health_generation: u64,
    ) -> IvRuntimeEngineError {
        if let Some(state) = self.state_for_profile(&event.profile_id) {
            self.record_source_rejection_diagnostic_with_retention(
                &state,
                IvRuntimeRejectionRecord {
                    profile_id: &event.profile_id,
                    source_id: &event.source_id,
                    subscription_generation: health_generation,
                    ts_event_ns: event.ts_event_ns,
                    reason: IvRejectReason::StaleData,
                    mark_rejected: false,
                },
            );
        }
        IvRuntimeEngineError::IngestRejected {
            profile_id: event.profile_id.clone(),
            source_id: event.source_id.clone(),
            reason: IvRejectReason::StaleData,
        }
    }

    fn reject_runtime_source_event(
        &self,
        profile_id: &str,
        source_id: &str,
        source: &IvRuntimeSourceConfig,
        ts_event_ns: UnixNanos,
        reason: IvRejectReason,
        mark_rejected: bool,
    ) -> IvRuntimeEngineError {
        if let Some(state) = self.state_for_profile(profile_id) {
            self.record_source_rejection_with_retention(
                &state,
                IvRuntimeRejectionRecord {
                    profile_id,
                    source_id,
                    subscription_generation: source.subscription_generation,
                    ts_event_ns,
                    reason,
                    mark_rejected,
                },
            );
        }
        IvRuntimeEngineError::IngestRejected {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            reason,
        }
    }

    fn reject_profile_source_event(
        &self,
        profile_id: &str,
        source_id: &str,
        subscription_generation: u64,
        ts_event_ns: UnixNanos,
        reason: IvRejectReason,
        mark_rejected: bool,
    ) -> IvRuntimeEngineError {
        if let Some(state) = self.state_for_profile(profile_id) {
            self.record_source_rejection_with_retention(
                &state,
                IvRuntimeRejectionRecord {
                    profile_id,
                    source_id,
                    subscription_generation,
                    ts_event_ns,
                    reason,
                    mark_rejected,
                },
            );
        }
        IvRuntimeEngineError::IngestRejected {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            reason,
        }
    }

    fn record_source_rejection_with_retention(
        &self,
        state: &IvQueryStateHandle,
        rejection: IvRuntimeRejectionRecord<'_>,
    ) {
        state.record_source_rejection(
            rejection.profile_id.to_string(),
            rejection.source_id.to_string(),
            rejection.subscription_generation,
            rejection.ts_event_ns,
            rejection.reason,
            rejection.mark_rejected,
        );
        if let Some(policy) = self.retention_policy(rejection.profile_id) {
            state.enforce_retention(&policy);
        }
    }

    fn record_source_rejection_diagnostic_with_retention(
        &self,
        state: &IvQueryStateHandle,
        rejection: IvRuntimeRejectionRecord<'_>,
    ) {
        state.record_source_rejection_diagnostic(
            rejection.profile_id.to_string(),
            rejection.source_id.to_string(),
            rejection.subscription_generation,
            rejection.ts_event_ns,
            rejection.reason,
        );
        if let Some(policy) = self.retention_policy(rejection.profile_id) {
            state.enforce_retention(&policy);
        }
    }

    fn retention_policy(&self, profile_id: &str) -> Option<IvRetentionPolicy> {
        self.read_inner()
            .retention_policies
            .get(profile_id)
            .copied()
    }

    pub fn apply_plan_outcomes(
        &self,
        outcomes: &[IvRuntimePlanOutcome],
    ) -> Result<(), IvRuntimeEngineError> {
        let mut first_error = None;
        for outcome in outcomes {
            if first_error.is_none()
                && let Some(error) = &outcome.error
            {
                first_error = Some(IvRuntimeEngineError::SubscriptionPlanFailed {
                    profile_id: error.profile_id.clone(),
                    source_id: error.source_id.clone(),
                    reason: error.reason,
                });
            }
            let Some((state, retention_policy)) = ({
                let inner = self.read_inner();
                inner
                    .profile_states
                    .get(&outcome.plan.profile_id)
                    .cloned()
                    .map(|state| {
                        (
                            state,
                            inner
                                .retention_policies
                                .get(&outcome.plan.profile_id)
                                .copied(),
                        )
                    })
            }) else {
                continue;
            };
            state.upsert_source_health(outcome.source_health.clone());
            if let Some(policy) = retention_policy {
                state.enforce_retention(&policy);
            }
        }

        first_error.map_or(Ok(()), Err)
    }
}

fn query_state_from_profile(profile: &IvProfile) -> IvQueryState {
    IvQueryState::new(IvStore::with_input_bounds(profile.input_bounds.clone()))
        .with_projection_policies(profile.projection_policies.clone())
        .with_interpolation_policies(profile.interpolation_policies.clone())
        .with_fallback_policies(profile.fallback_policies.clone())
        .with_quorum_policies(profile.quorum_policies.clone())
        .with_helper_policies(profile.helper_policies.clone())
        .with_derived_input_policies(profile.derived_input_policies.clone())
        .with_derived_inputs(runtime_derived_inputs_from_profile(profile))
        .with_current_subscription_generations(current_generations_from_profile(profile))
}

pub(crate) fn runtime_derived_inputs_from_profile(
    profile: &IvProfile,
) -> Vec<super::derive::IvDerivedInputSet> {
    profile
        .derived_inputs
        .iter()
        .cloned()
        .map(|mut inputs| {
            inputs.nt_revision = cargo_pinned_nt_revision().to_string();
            inputs
        })
        .collect()
}

fn current_generations_from_profile(profile: &IvProfile) -> BTreeMap<String, u64> {
    profile
        .sources
        .iter()
        .map(|source| (source.source_id.clone(), source.subscription_generation))
        .collect()
}

fn retention_policy_from_profile(profile: &IvProfile) -> IvRetentionPolicy {
    IvRetentionPolicy {
        max_raw_events: profile.max_raw_events,
        max_indexed_points: profile.max_indexed_points,
        max_smiles: profile.max_smiles,
        max_surfaces: profile.max_surfaces,
        max_derived_points: profile.max_derived_points,
        max_source_health_events: profile.max_source_health_events,
    }
}

fn runtime_sources_from_profile(profile: &IvProfile) -> BTreeMap<String, IvRuntimeSourceConfig> {
    profile
        .sources
        .iter()
        .map(|source| {
            let mut nt_provenance = source.nt_provenance.clone();
            nt_provenance.nt_revision = cargo_pinned_nt_revision().to_string();
            (
                source.source_id.clone(),
                IvRuntimeSourceConfig {
                    source_kind: source.source_kind,
                    selector_fingerprint: source.selector_fingerprint.clone(),
                    subscription_generation: source.subscription_generation,
                    max_source_event_age_ns: profile.max_source_event_age_ns,
                    max_source_event_future_skew_ns: profile.max_source_event_future_skew_ns,
                    accepted_conventions: source.accepted_conventions.clone(),
                    nt_provenance,
                    selector: source.selector.clone(),
                },
            )
        })
        .collect()
}

fn source_generations_from_runtime_sources(
    sources: &BTreeMap<String, IvRuntimeSourceConfig>,
) -> BTreeMap<String, u64> {
    sources
        .iter()
        .map(|(source_id, source)| (source_id.clone(), source.subscription_generation))
        .collect()
}

fn cargo_pinned_nt_revision_from_lock(lock_text: &str) -> Option<String> {
    let lock = toml::from_str::<toml::Value>(lock_text).ok()?;
    lock.get("package")?
        .as_array()?
        .iter()
        .filter_map(|package| package.get("source")?.as_str())
        .find_map(nt_source_revision)
}

fn nt_source_revision(source: &str) -> Option<String> {
    if !source.contains("nautilus_trader") {
        return None;
    }

    if let Some((_, revision)) = source.rsplit_once('#')
        && !revision.is_empty()
    {
        return Some(revision.to_string());
    }

    source.find("rev=").and_then(|start| {
        let revision_start = start + "rev=".len();
        let revision = source[revision_start..]
            .split(['&', '#'])
            .next()
            .unwrap_or("");

        (!revision.is_empty()).then(|| revision.to_string())
    })
}

fn removed_source_generations(
    previous_sources: &BTreeMap<String, IvRuntimeSourceConfig>,
    next_sources: &BTreeMap<String, IvRuntimeSourceConfig>,
) -> BTreeMap<String, u64> {
    previous_sources
        .iter()
        .filter(|(source_id, _)| !next_sources.contains_key(*source_id))
        .map(|(source_id, source)| (source_id.clone(), source.subscription_generation))
        .collect()
}

impl IvRuntimeSourceConfig {
    fn accepts_nt_greeks_convention(
        &self,
        convention: nautilus_model::enums::GreeksConvention,
    ) -> bool {
        self.accepted_conventions
            .contains(&nt_greeks_convention_name(convention))
    }

    fn selector_matches_option_greeks(&self, instrument_id: &str) -> bool {
        match &self.selector {
            IvSelector::SourceOptionGreeks { instrument_ids, .. } => instrument_ids
                .iter()
                .any(|configured| configured == instrument_id),
            _ => false,
        }
    }

    fn selector_matches_option_chain(&self, series_id: &str) -> bool {
        match &self.selector {
            IvSelector::SourceOptionChain { series_ids, .. } => {
                series_ids.iter().any(|configured| configured == series_id)
            }
            _ => false,
        }
    }
}

fn option_chain_conventions_supported(
    source: &IvRuntimeSourceConfig,
    option_chain: &nautilus_model::data::OptionChainSlice,
) -> bool {
    option_chain
        .calls
        .values()
        .chain(option_chain.puts.values())
        .filter_map(|strike| strike.greeks.as_ref())
        .all(|greeks| source.accepts_nt_greeks_convention(greeks.convention))
}

fn custom_data_json_value(custom_data: &CustomData) -> Result<Value, IvRejectReason> {
    let text = custom_data
        .data
        .to_json()
        .map_err(|_| IvRejectReason::InvalidIvValue)?;
    serde_json::from_str::<Value>(&text).map_err(|_| IvRejectReason::InvalidIvValue)
}

fn custom_data_field_f64(payload: &Value, field_name: &str) -> Result<f64, IvRejectReason> {
    let Some(value) = payload.get(field_name).and_then(Value::as_f64) else {
        return Err(IvRejectReason::InvalidIvValue);
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err(IvRejectReason::InvalidIvValue)
    }
}

fn option_greeks_basis_values(
    option_greeks: &nautilus_model::data::OptionGreeks,
) -> Vec<IvBasisValue> {
    let mut basis_values = Vec::new();
    if let Some(iv) = option_greeks.mark_iv {
        basis_values.push(IvBasisValue {
            basis: IvBasis::Mark,
            iv,
        });
    }
    if let Some(iv) = option_greeks.bid_iv {
        basis_values.push(IvBasisValue {
            basis: IvBasis::Bid,
            iv,
        });
    }
    if let Some(iv) = option_greeks.ask_iv {
        basis_values.push(IvBasisValue {
            basis: IvBasis::Ask,
            iv,
        });
    }
    basis_values
}

fn nt_greeks_convention_name(convention: nautilus_model::enums::GreeksConvention) -> String {
    convention.to_string()
}

fn option_chain_strike_payload(
    strike: f64,
    data: &nautilus_model::data::OptionStrikeData,
) -> IvOptionChainStrikePayload {
    IvOptionChainStrikePayload {
        strike,
        quote: IvOptionChainQuotePayload {
            instrument_id: data.quote.instrument_id.to_string(),
            bid_price: Some(data.quote.bid_price.as_f64()),
            ask_price: Some(data.quote.ask_price.as_f64()),
            bid_size: Some(data.quote.bid_size.as_f64()),
            ask_size: Some(data.quote.ask_size.as_f64()),
            ts_event_ns: UnixNanos::new(data.quote.ts_event.as_u64()),
            ts_init_ns: Some(UnixNanos::new(data.quote.ts_init.as_u64())),
        },
        greeks: data.greeks.map(|greeks| IvOptionGreeksPayload {
            instrument_id: greeks.instrument_id.to_string(),
            convention: IvConvention::Named(nt_greeks_convention_name(greeks.convention)),
            basis_values: option_greeks_basis_values(&greeks),
            greeks: IvGreekValues {
                delta: Some(greeks.greeks.delta),
                gamma: Some(greeks.greeks.gamma),
                vega: Some(greeks.greeks.vega),
                theta: Some(greeks.greeks.theta),
                rho: Some(greeks.greeks.rho),
            },
            underlying_price: greeks.underlying_price,
            open_interest: greeks.open_interest,
        }),
    }
}

pub fn apply_subscription_plans<A: IvRuntimeBindingAdapter>(
    adapter: &mut A,
    plans: &[IvSubscriptionPlan],
) -> Vec<IvRuntimePlanOutcome> {
    plans
        .iter()
        .map(|plan| match adapter.apply_subscription_plan(plan) {
            Ok(()) => IvRuntimePlanOutcome {
                plan: plan.clone(),
                source_health: source_health(plan, success_state(plan.operation), None),
                error: None,
            },
            Err(error) => IvRuntimePlanOutcome {
                plan: plan.clone(),
                source_health: source_health(
                    plan,
                    IvSourceHealthState::SubscriptionFailed,
                    Some(error.reason),
                ),
                error: Some(error),
            },
        })
        .collect()
}

fn success_state(operation: IvRuntimeOperation) -> IvSourceHealthState {
    match operation {
        IvRuntimeOperation::SubscribeOptionGreeks
        | IvRuntimeOperation::SubscribeOptionChain
        | IvRuntimeOperation::SubscribeAggregateGreeks
        | IvRuntimeOperation::SubscribeCustomData => IvSourceHealthState::Subscribing,
        IvRuntimeOperation::UnsubscribeOptionGreeks
        | IvRuntimeOperation::UnsubscribeOptionChain
        | IvRuntimeOperation::UnsubscribeAggregateGreeks
        | IvRuntimeOperation::UnsubscribeCustomData => IvSourceHealthState::Unsubscribing,
        IvRuntimeOperation::RemoveSource => IvSourceHealthState::Removed,
    }
}

fn source_health(
    plan: &IvSubscriptionPlan,
    subscription_state: IvSourceHealthState,
    reject_reason: Option<IvRejectReason>,
) -> IvSourceHealth {
    let mut reject_counts = BTreeMap::new();
    if let Some(reason) = reject_reason {
        reject_counts.insert(reason, 1);
    }

    IvSourceHealth {
        profile_id: plan.profile_id.clone(),
        source_id: plan.source_id.clone(),
        subscription_state,
        last_event_ts_ns: None,
        last_reject_reason: reject_reason,
        reject_counts,
        stale_state: false,
        retention_state: false,
        subscription_generation: plan.subscription_generation,
    }
}

fn active_source_health_for_event(event: &IvIngestEvent) -> IvSourceHealth {
    IvSourceHealth {
        profile_id: event.profile_id.clone(),
        source_id: event.source_id.clone(),
        subscription_state: IvSourceHealthState::Active,
        last_event_ts_ns: Some(event.ts_event_ns),
        last_reject_reason: None,
        reject_counts: BTreeMap::new(),
        stale_state: false,
        retention_state: false,
        subscription_generation: event.subscription_generation,
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::super::config::SUPPORTED_IV_SCHEMA_VERSION;
    use super::*;

    #[test]
    #[should_panic(expected = "IV runtime engine state lock poisoned")]
    fn runtime_engine_read_panics_on_poisoned_lock() {
        let engine = IvRuntimeEngine::from_iv_root(&IvRootConfig {
            schema_version: SUPPORTED_IV_SCHEMA_VERSION,
            profiles: Vec::new(),
        })
        .unwrap();
        let inner = engine.inner.clone();
        let poison_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = inner.write().unwrap();
            panic!("poison runtime engine lock");
        }));
        assert!(poison_result.is_err());
        assert!(inner.read().is_err());

        engine.state_for_profile("missing_profile");
    }

    #[test]
    #[should_panic(expected = "IV runtime engine state lock poisoned")]
    fn runtime_engine_write_panics_on_poisoned_lock() {
        let mut engine = IvRuntimeEngine::from_iv_root(&IvRootConfig {
            schema_version: SUPPORTED_IV_SCHEMA_VERSION,
            profiles: Vec::new(),
        })
        .unwrap();
        let inner = engine.inner.clone();
        let poison_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = inner.write().unwrap();
            panic!("poison runtime engine lock");
        }));
        assert!(poison_result.is_err());
        assert!(inner.write().is_err());

        let _ = engine.apply_iv_root_reload(&IvRootConfig {
            schema_version: SUPPORTED_IV_SCHEMA_VERSION,
            profiles: Vec::new(),
        });
    }
}
