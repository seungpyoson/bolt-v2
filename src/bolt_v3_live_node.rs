//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or submit paths.
//!
//! Bolt-v3 LiveNode controlled-build / controlled-connect /
//! controlled-disconnect boundary. This module:
//!
//! - validates the forbidden credential env-var blocklist before
//!   constructing any NautilusTrader client
//! - resolves SSM secrets via the bolt-v3 secret resolver
//! - maps the validated bolt-v3 client blocks into provider-owned
//!   NT-native adapter configs
//! - registers the per-client NT data and execution client factories on a
//!   `nautilus_live::builder::LiveNodeBuilder` via the
//!   [`crate::bolt_v3_client_registration`] boundary
//! - calls `LiveNodeBuilder::build`, which is **not** purely passive:
//!   it constructs the NT client objects, lets provider-owned NT
//!   factories parse their credential material, and performs internal
//!   NT engine/message-bus subscriptions for venue instrument topics.
//!   None of these steps open a network connection or run the event
//!   loop.
//! - returns the resulting `nautilus_live::node::LiveNode` to the caller
//!   without entering the NT runner loop from the build path
//! - wires the existing `crate::nt_runtime_capture` from the
//!   `[persistence]` / `[persistence.streaming]` blocks
//! - installs module-level logger filters from provider-owned bindings
//!   that suppress NT credential info logs even when the root TOML log
//!   level is `INFO`
//!
//! The caller owns the `LiveNode`; the build path never opens an
//! external network connection. Opt-in controlled-connect/strategy-free
//! readiness boundaries may open adapter sockets. The production
//! trading runner entrypoint is [`run_bolt_v3_live_node`]. The strategy-free
//! readiness path builds a strategy-free node before using NT's supported
//! runner loop with handle-driven stop; its dedicated quote probes call
//! only NT quote subscribe/unsubscribe APIs for configured strategy
//! `[reference_data]` or client-owned readiness-probe instruments. This
//! module still never constructs an order or enables any submit path
//! from its own boundary code.

#[cfg(test)]
use std::cell::Cell;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ahash::AHashMap;
use anyhow::Result;
use log::LevelFilter;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle, NodeState},
};
#[cfg(test)]
use nautilus_model::{
    data::{OrderBookDeltas, QuoteTick},
    identifiers::InstrumentId,
};
use nautilus_model::{
    enums::{BarIntervalType, OrderSide, OrderType, TradingState},
    identifiers::{ClientId, StrategyId},
    orders::{Order, OrderAny},
};
use rust_decimal::Decimal;
use ustr::Ustr;
use zeroize::Zeroizing;

#[cfg(test)]
use crate::bolt_v3_config::{
    DataClientReadinessProbeBlock, DataClientReadinessProbeBookType,
    DataClientReadinessProbeMarketDataKind, DataClientReadinessProbeQuoteTargetSource,
};
use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterConfigs, BoltV3AdapterMappingError, map_bolt_v3_adapters,
        map_bolt_v3_adapters_with_runtime_approvals,
    },
    bolt_v3_capital_reservation::CapitalPoolSnapshot,
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{
        CapitalPoolBlock, CapitalPoolSizingPolicyBlock, LoadedBoltV3Config,
        resolve_root_relative_path,
    },
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3PositionSizerRebuildAuditEvidence, BoltV3StrategyInputEvidenceSnapshot,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        JsonlBoltV3DecisionEvidenceWriter, decision_evidence_path,
        read_submit_reservation_recovery_evidence,
    },
    bolt_v3_loss_governor::{LossGovernorPolicy, evaluate_loss_admission},
    bolt_v3_loss_halt_actions::{
        LossGovernorHaltActionHandler, LossGovernorHaltActionPolicy,
        LossGovernorManualRecoveryEvidence, LossGovernorManualRecoveryRequest,
        LossGovernorRecoveryMode, LossGovernorTradingStateAction,
        next_loss_governor_manual_recovery_trading_state, next_loss_governor_trading_state,
    },
    bolt_v3_loss_runtime_feed::{
        LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig,
        LossGovernorRuntimeFeedSubscription, subscribe_loss_governor_runtime_feed,
    },
    bolt_v3_position_sizer::{
        FeeSlippagePolicy, PredictionMarketSizingSnapshot, ProductKind, ProductSizingSnapshot,
        SizingPolicy,
    },
    bolt_v3_position_sizer_runtime_feed::{
        PositionSizerRuntimeFeed, PositionSizerRuntimeFeedConfig,
        PositionSizerRuntimeFeedSubscription, subscribe_position_sizer_runtime_feed,
    },
    bolt_v3_providers::{
        self, ProviderLiveSubmitApprovalContext, ProviderLiveSubmitApprovals,
        ProviderRuntimeApprovals,
    },
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars, check_no_forbidden_credential_env_vars_with,
        resolve_bolt_v3_secrets, resolve_bolt_v3_secrets_with,
    },
    bolt_v3_sizing_state::{
        VenueSpendabilityIdentity, VenueSpendabilitySnapshot, VenueSpendabilitySourceFileRequest,
        venue_spendability_snapshot_from_json_file,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, register_bolt_v3_strategies_on_node_with_bindings,
    },
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderSide, BoltV3LiveSubmitApprovalLimits, BoltV3SubmitAdmissionState,
        BoltV3SubmitPositionSizerConfig, BoltV3SubmitPositionSizingMissingNtAccountCacheBalance,
        BoltV3SubmitPositionSizingNtComponents, BoltV3SubmitPositionSizingOpenOrderEvidence,
        BoltV3SubmitPositionSizingOpenOrderSnapshot, BoltV3SubmitPositionSizingRebuildDecision,
    },
    bolt_v3_validate::parse_decimal_string,
    nt_runtime_capture::{NtRuntimeCaptureGuards, wire_nt_runtime_capture},
    secrets::SsmResolverSession,
};

pub fn current_build_head_sha() -> Option<&'static str> {
    option_env!("BOLT_V3_BUILD_HEAD_SHA").filter(|value| is_git_head_sha(value))
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    registration_summary: BoltV3RegistrationSummary,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    position_sizer_runtime_feed: Option<Arc<Mutex<PositionSizerRuntimeFeed>>>,
    position_sizer_runtime_feed_subscription: Option<PositionSizerRuntimeFeedSubscription>,
    position_sizer_venue_spendability_source:
        Option<BoltV3PositionSizerVenueSpendabilitySourceConfig>,
    submit_reservation_recovery: Option<BoltV3SubmitReservationRecoveryConfig>,
    redaction_values: Vec<Zeroizing<String>>,
}

#[derive(Debug, Clone)]
struct BoltV3PositionSizerVenueSpendabilitySourceConfig {
    path: PathBuf,
    max_bytes: u64,
    expected_sha256: String,
    venue_id: String,
    account_id: String,
    collateral_currency: String,
}

/// Startup reservation-recovery source: the decision-evidence file the
/// live-node boot driver reads to recover known submit-reservation
/// metadata after a restart, plus the byte cap from
/// [`crate::bolt_v3_config::DecisionEvidenceBlock::recovery_evidence_max_bytes`].
#[derive(Debug, Clone)]
struct BoltV3SubmitReservationRecoveryConfig {
    path: PathBuf,
    max_bytes: u64,
}

#[derive(Debug)]
struct BoltV3LiveNodeAdapterBundle {
    configs: BoltV3AdapterConfigs,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StrategyFreeReferenceCacheEvidence {
    cached_instrument_ids: Vec<String>,
}

#[cfg(test)]
mod strategy_free_probe {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub struct BoltV3StrategyFreeReferenceQuote {
        pub data_client_id: String,
        pub instrument_id: String,
        pub bid_price: f64,
        pub ask_price: f64,
        pub ts_event_unix_nanos: u64,
        pub ts_init_unix_nanos: u64,
        pub captured_at_unix_nanos: u64,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct BoltV3StrategyFreeReferenceQuoteEvidence {
        pub quotes: Vec<BoltV3StrategyFreeReferenceQuote>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BoltV3StrategyFreeBookDeltas {
        pub data_client_id: String,
        pub instrument_id: String,
        pub delta_count: u64,
        pub ts_event_unix_nanos: u64,
        pub ts_init_unix_nanos: u64,
        pub captured_at_unix_nanos: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BoltV3StrategyFreeBookDeltasEvidence {
        pub deltas: Vec<BoltV3StrategyFreeBookDeltas>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct StrategyFreeReferenceQuoteSubscription {
        pub(super) data_client_id: ClientId,
        pub(super) instrument_id: InstrumentId,
    }

    /// Live state for a trade chunk-count readiness walk. The probe subscribes one
    /// chunk of the instrument universe at a time (so it never holds more than
    /// `chunk_size` channels at once, staying below the venue's silent delivery
    /// ceiling), watches it for `chunk_observation_window_seconds`, then advances.
    /// It passes as soon as `required_live_markets` (`m`) distinct markets have
    /// traded, and fails closed once the whole universe has been walked without
    /// reaching `m`. Interior mutability mirrors the surrounding handle: the actor
    /// is single-threaded (`!Send`), so `Cell`/`RefCell` is sufficient.
    #[derive(Debug)]
    struct ChunkCountWalk {
        data_client_id: ClientId,
        chunk_size: usize,
        chunk_observation_window_seconds: u64,
        required_live_markets: usize,
        /// Universe pre-split into consecutive chunks of at most `chunk_size`,
        /// populated when the metadata response arrives.
        chunks: RefCell<Vec<Vec<InstrumentId>>>,
        /// Index of the next chunk to subscribe.
        cursor: Cell<usize>,
        /// Set once the universe has been captured and chunking has begun.
        started: Cell<bool>,
        /// Set once the walk has finished, whether by reaching `m` (pass) or by
        /// exhausting the universe (fail closed).
        complete: Cell<bool>,
    }

    #[derive(Debug, Clone)]
    pub(super) struct BoltV3StrategyFreeReferenceQuoteProbeHandle {
        pub(super) required: Rc<RefCell<Vec<StrategyFreeReferenceQuoteSubscription>>>,
        pub(super) ambiguous_instrument_ids: Rc<RefCell<BTreeSet<String>>>,
        pub(super) market_data_kind: DataClientReadinessProbeMarketDataKind,
        pub(super) metadata_response_data_client_id: Option<ClientId>,
        pub(super) metadata_response_max_quote_targets: Option<usize>,
        pub(super) metadata_response_allow_target_sampling: bool,
        pub(super) min_observed_targets: Option<usize>,
        pub(super) quote_targets_initialized: Rc<Cell<bool>>,
        pub(super) failure_reason: Rc<RefCell<Option<String>>>,
        pub(super) quotes: Rc<RefCell<Vec<BoltV3StrategyFreeReferenceQuote>>>,
        pub(super) book_deltas: Rc<RefCell<Vec<BoltV3StrategyFreeBookDeltas>>>,
        pub(super) quote_notify: Rc<tokio::sync::Notify>,
        /// Present only for a trade chunk-count probe (`market_data_kind = "trade"`
        /// with `quote_target_source = "metadata_response"`); drives the chunked
        /// walk over the instrument universe instead of a fixed sampled target set.
        chunk_walk: Option<Rc<ChunkCountWalk>>,
    }

    impl BoltV3StrategyFreeReferenceQuoteProbeHandle {
        pub(super) fn new(loaded: &LoadedBoltV3Config) -> Self {
            let (required, ambiguous_instrument_ids) =
                strategy_free_reference_quote_subscription_plan(loaded);
            Self::from_plan(
                required,
                ambiguous_instrument_ids,
                DataClientReadinessProbeMarketDataKind::Quote,
                None,
            )
        }

        pub(super) fn from_plan(
            required: Vec<StrategyFreeReferenceQuoteSubscription>,
            ambiguous_instrument_ids: BTreeSet<String>,
            market_data_kind: DataClientReadinessProbeMarketDataKind,
            min_observed_targets: Option<usize>,
        ) -> Self {
            Self {
                required: Rc::new(RefCell::new(required)),
                ambiguous_instrument_ids: Rc::new(RefCell::new(ambiguous_instrument_ids)),
                market_data_kind,
                metadata_response_data_client_id: None,
                metadata_response_max_quote_targets: None,
                metadata_response_allow_target_sampling: false,
                min_observed_targets,
                quote_targets_initialized: Rc::new(Cell::new(true)),
                failure_reason: Rc::new(RefCell::new(None)),
                quotes: Rc::new(RefCell::new(Vec::new())),
                book_deltas: Rc::new(RefCell::new(Vec::new())),
                quote_notify: Rc::new(tokio::sync::Notify::new()),
                chunk_walk: None,
            }
        }

        pub(super) fn from_metadata_response_plan(
            data_client_id: ClientId,
            max_quote_targets: usize,
            allow_target_sampling: bool,
            market_data_kind: DataClientReadinessProbeMarketDataKind,
            min_observed_targets: Option<usize>,
        ) -> Self {
            Self {
                required: Rc::new(RefCell::new(Vec::new())),
                ambiguous_instrument_ids: Rc::new(RefCell::new(BTreeSet::new())),
                market_data_kind,
                metadata_response_data_client_id: Some(data_client_id),
                metadata_response_max_quote_targets: Some(max_quote_targets),
                metadata_response_allow_target_sampling: allow_target_sampling,
                min_observed_targets,
                quote_targets_initialized: Rc::new(Cell::new(false)),
                failure_reason: Rc::new(RefCell::new(None)),
                quotes: Rc::new(RefCell::new(Vec::new())),
                book_deltas: Rc::new(RefCell::new(Vec::new())),
                quote_notify: Rc::new(tokio::sync::Notify::new()),
                chunk_walk: None,
            }
        }

        pub(super) fn from_metadata_response_chunk_count_plan(
            data_client_id: ClientId,
            chunk_size: usize,
            chunk_observation_window_seconds: u64,
            required_live_markets: usize,
            market_data_kind: DataClientReadinessProbeMarketDataKind,
        ) -> Self {
            Self {
                required: Rc::new(RefCell::new(Vec::new())),
                ambiguous_instrument_ids: Rc::new(RefCell::new(BTreeSet::new())),
                market_data_kind,
                metadata_response_data_client_id: Some(data_client_id),
                metadata_response_max_quote_targets: None,
                metadata_response_allow_target_sampling: false,
                min_observed_targets: Some(required_live_markets),
                quote_targets_initialized: Rc::new(Cell::new(false)),
                failure_reason: Rc::new(RefCell::new(None)),
                quotes: Rc::new(RefCell::new(Vec::new())),
                book_deltas: Rc::new(RefCell::new(Vec::new())),
                quote_notify: Rc::new(tokio::sync::Notify::new()),
                chunk_walk: Some(Rc::new(ChunkCountWalk {
                    data_client_id,
                    chunk_size,
                    chunk_observation_window_seconds,
                    required_live_markets,
                    chunks: RefCell::new(Vec::new()),
                    cursor: Cell::new(0),
                    started: Cell::new(false),
                    complete: Cell::new(false),
                })),
            }
        }

        pub(super) fn is_chunk_count_mode(&self) -> bool {
            self.chunk_walk.is_some()
        }

        /// Capture the metadata-response universe and split it into chunks. The
        /// universe is sorted and de-duplicated so chunk membership is
        /// deterministic; which markets ultimately certify the feed is still
        /// liveness-driven (a chunk's markets only count once they actually trade).
        pub(super) fn chunk_count_capture_universe(&self, mut instrument_ids: Vec<InstrumentId>) {
            let Some(walk) = &self.chunk_walk else {
                return;
            };
            if walk.started.get() {
                return;
            }
            instrument_ids.sort_by_key(|instrument_id| instrument_id.to_string());
            instrument_ids.dedup();
            *walk.chunks.borrow_mut() = chunk_universe(&instrument_ids, walk.chunk_size);
            walk.cursor.set(0);
            walk.started.set(true);
            self.quote_notify.notify_one();
        }

        /// Take the next chunk to subscribe, installing it as the probe's current
        /// `required` set so recorded trades match against it. Returns `None` once
        /// the universe is exhausted.
        pub(super) fn chunk_count_next_chunk(
            &self,
        ) -> Option<Vec<StrategyFreeReferenceQuoteSubscription>> {
            let walk = self.chunk_walk.as_ref()?;
            let cursor = walk.cursor.get();
            let chunk = walk.chunks.borrow().get(cursor).cloned()?;
            walk.cursor.set(cursor + 1);
            let subscriptions: Vec<StrategyFreeReferenceQuoteSubscription> = chunk
                .into_iter()
                .map(|instrument_id| StrategyFreeReferenceQuoteSubscription {
                    data_client_id: walk.data_client_id,
                    instrument_id,
                })
                .collect();
            *self.required.borrow_mut() = subscriptions.clone();
            Some(subscriptions)
        }

        /// The chunk currently subscribed, returned so the actor can unsubscribe it
        /// before advancing to the next chunk.
        pub(super) fn chunk_count_current_chunk(
            &self,
        ) -> Vec<StrategyFreeReferenceQuoteSubscription> {
            self.required.borrow().clone()
        }

        pub(super) fn chunk_count_passed(&self) -> bool {
            match &self.chunk_walk {
                Some(walk) => trade_chunk_count_probe_passed(0, walk.required_live_markets),
                None => false,
            }
        }

        pub(super) fn chunk_walk_started(&self) -> bool {
            self.chunk_walk
                .as_ref()
                .is_some_and(|walk| walk.started.get())
        }

        /// `(number_of_chunks, per_chunk_window_seconds)` for sizing the overall
        /// walk timeout once the universe is known.
        pub(super) fn chunk_walk_dims(&self) -> (usize, u64) {
            match &self.chunk_walk {
                Some(walk) => (
                    walk.chunks.borrow().len(),
                    walk.chunk_observation_window_seconds,
                ),
                None => (0, 0),
            }
        }

        #[cfg(test)]
        pub(super) fn has_all_required_quotes(&self) -> bool {
            if self.market_data_kind != DataClientReadinessProbeMarketDataKind::Quote {
                return false;
            }
            self.has_all_required_market_data()
        }

        pub(super) fn has_all_required_market_data(&self) -> bool {
            if self.failure_error().is_some() {
                return false;
            }
            if let Some(walk) = &self.chunk_walk {
                // Chunk-count probe: satisfied once the walk has concluded by
                // reaching `m` distinct firing markets. Fail-closed exhaustion sets
                // `failure_error` (handled above), so reaching here with
                // `complete` set and the pass rule unmet cannot happen.
                return walk.complete.get() && self.chunk_count_passed();
            }
            if !self.ambiguous_instrument_ids.borrow().is_empty() {
                return false;
            }
            if !self.quote_targets_initialized.get() {
                return false;
            }
            let required = self.required.borrow();
            if self.metadata_response_data_client_id.is_some() && required.is_empty() {
                return false;
            }
            let required_observations = self.required_observation_count(required.len());
            match self.market_data_kind {
                DataClientReadinessProbeMarketDataKind::Quote => {
                    let quotes = self.quotes.borrow();
                    observed_required_quote_count(&required, &quotes) >= required_observations
                }
                DataClientReadinessProbeMarketDataKind::Book => {
                    let book_deltas = self.book_deltas.borrow();
                    observed_required_book_delta_count(&required, &book_deltas)
                        >= required_observations
                }
                DataClientReadinessProbeMarketDataKind::Trade => false,
            }
        }

        /// Number of sampled targets that must be observed for the probe to pass.
        ///
        /// Defaults to every sampled target (strict, fail-closed). When
        /// `readiness_probe.min_observed_targets` is configured it lowers the bar to
        /// that value, clamped into `[1, sampled_len]` so a broad metadata universe
        /// can prove adapter data-path behaviour without requiring every illiquid or
        /// un-streamable sampled instrument to tick within the configured wait.
        fn required_observation_count(&self, sampled_len: usize) -> usize {
            match self.min_observed_targets {
                Some(min_observed) => min_observed.clamp(1, sampled_len.max(1)),
                None => sampled_len,
            }
        }

        pub(super) fn ambiguity_error(&self) -> Option<String> {
            if self.ambiguous_instrument_ids.borrow().is_empty() {
                return None;
            }
            Some(
            "reference quote probe cannot distinguish multiple data clients for the same instrument_id; QuoteTick does not carry data_client_id"
                .to_string(),
        )
        }

        pub(super) fn failure_error(&self) -> Option<String> {
            self.failure_reason.borrow().clone()
        }

        fn fail_metadata_response_probe(&self, reason: String) {
            if self.failure_reason.borrow().is_none() {
                *self.failure_reason.borrow_mut() = Some(reason);
            }
            self.required.borrow_mut().clear();
            self.ambiguous_instrument_ids.borrow_mut().clear();
            self.quote_targets_initialized.set(true);
            self.quote_notify.notify_one();
        }

        pub(super) fn evidence(&self) -> BoltV3StrategyFreeReferenceQuoteEvidence {
            BoltV3StrategyFreeReferenceQuoteEvidence {
                quotes: self.quotes.borrow().clone(),
            }
        }

        pub(super) fn book_evidence(&self) -> BoltV3StrategyFreeBookDeltasEvidence {
            BoltV3StrategyFreeBookDeltasEvidence {
                deltas: self.book_deltas.borrow().clone(),
            }
        }

        pub(super) fn install_metadata_response_instrument_ids(
            &self,
            mut instrument_ids: Vec<InstrumentId>,
        ) -> Vec<StrategyFreeReferenceQuoteSubscription> {
            let Some(data_client_id) = self.metadata_response_data_client_id else {
                return Vec::new();
            };
            if self.quote_targets_initialized.get() {
                return Vec::new();
            }
            instrument_ids.sort_by_key(|instrument_id| instrument_id.to_string());
            instrument_ids.dedup();
            let Some(max_quote_targets) = self.metadata_response_max_quote_targets else {
                self.fail_metadata_response_probe(
                "clients.<id>.readiness_probe.max_metadata_quote_targets is missing for metadata_response readiness probing".to_string(),
            );
                return Vec::new();
            };
            let metadata_quote_targets = instrument_ids.len();
            if metadata_quote_targets > max_quote_targets {
                if self.metadata_response_allow_target_sampling {
                    instrument_ids =
                        sample_metadata_response_targets(&instrument_ids, max_quote_targets);
                } else {
                    self.fail_metadata_response_probe(format!(
                    "metadata_response produced {metadata_quote_targets} source-owned quote targets, exceeding clients.<id>.readiness_probe.max_metadata_quote_targets={max_quote_targets}; tighten TOML-owned metadata filters or set clients.<id>.readiness_probe.allow_metadata_target_sampling=true before using this client for production readiness"
                ));
                    return Vec::new();
                }
            }
            let subscriptions = instrument_ids
                .into_iter()
                .map(|instrument_id| StrategyFreeReferenceQuoteSubscription {
                    data_client_id,
                    instrument_id,
                })
                .collect();
            let (required, ambiguous_instrument_ids) =
                dedupe_strategy_free_reference_quote_subscriptions(subscriptions);
            if let Some(min_observed) = self.min_observed_targets
                && min_observed > required.len()
            {
                self.fail_metadata_response_probe(format!(
                "clients.<id>.readiness_probe.min_observed_targets={min_observed} exceeds the {} source-owned metadata_response target(s) sampled this run",
                required.len()
            ));
                return Vec::new();
            }
            *self.required.borrow_mut() = required.clone();
            *self.ambiguous_instrument_ids.borrow_mut() = ambiguous_instrument_ids;
            self.quote_targets_initialized.set(true);
            self.quote_notify.notify_one();
            required
        }

        pub(super) fn record_quote(&self, quote: &QuoteTick, captured_at_unix_nanos: u64) {
            let quote_instrument_id = quote.instrument_id.to_string();
            if self
                .ambiguous_instrument_ids
                .borrow()
                .contains(&quote_instrument_id)
            {
                return;
            }
            let required = self.required.borrow().clone();
            let mut matched_required = false;
            let mut quotes = self.quotes.borrow_mut();
            for required in &required {
                if quote.instrument_id == required.instrument_id {
                    matched_required = true;
                    quotes.push(BoltV3StrategyFreeReferenceQuote {
                        data_client_id: required.data_client_id.to_string(),
                        instrument_id: required.instrument_id.to_string(),
                        bid_price: quote.bid_price.as_f64(),
                        ask_price: quote.ask_price.as_f64(),
                        ts_event_unix_nanos: quote.ts_event.as_u64(),
                        ts_init_unix_nanos: quote.ts_init.as_u64(),
                        captured_at_unix_nanos,
                    });
                }
            }
            drop(quotes);
            if matched_required && self.has_all_required_market_data() {
                self.quote_notify.notify_one();
            }
        }

        pub(super) fn record_book_deltas(
            &self,
            deltas: &OrderBookDeltas,
            captured_at_unix_nanos: u64,
        ) {
            let deltas_instrument_id = deltas.instrument_id.to_string();
            if self
                .ambiguous_instrument_ids
                .borrow()
                .contains(&deltas_instrument_id)
            {
                return;
            }
            let required = self.required.borrow().clone();
            let mut matched_required = false;
            let mut book_deltas = self.book_deltas.borrow_mut();
            for required in &required {
                if deltas.instrument_id == required.instrument_id {
                    matched_required = true;
                    book_deltas.push(BoltV3StrategyFreeBookDeltas {
                        data_client_id: required.data_client_id.to_string(),
                        instrument_id: required.instrument_id.to_string(),
                        delta_count: deltas.deltas.len() as u64,
                        ts_event_unix_nanos: deltas.ts_event.as_u64(),
                        ts_init_unix_nanos: deltas.ts_init.as_u64(),
                        captured_at_unix_nanos,
                    });
                }
            }
            drop(book_deltas);
            if matched_required && self.has_all_required_market_data() {
                self.quote_notify.notify_one();
            }
        }

        pub(super) async fn wait_for_all_required_quotes(&self) -> Result<(), String> {
            loop {
                if let Some(reason) = self.failure_error() {
                    return Err(reason);
                }
                if self.has_all_required_market_data() {
                    return Ok(());
                }
                self.quote_notify.notified().await;
            }
        }
    }

    pub(crate) fn sample_metadata_response_targets<T: Clone>(
        targets: &[T],
        max_targets: usize,
    ) -> Vec<T> {
        if max_targets == 0 {
            return Vec::new();
        }
        if targets.len() <= max_targets {
            return targets.to_vec();
        }
        if max_targets == 1 {
            return vec![targets[targets.len() / 2].clone()];
        }
        let last_index = targets.len() - 1;
        let last_sample = max_targets - 1;
        (0..max_targets)
            .map(|sample_index| targets[(sample_index * last_index) / last_sample].clone())
            .collect()
    }

    /// Split a deterministically-ordered instrument universe into consecutive
    /// chunks of at most `chunk_size`, preserving order. Returns no chunks when
    /// `chunk_size == 0` so a misconfigured probe observes nothing and fails
    /// closed rather than panicking. The trade chunk-count readiness probe walks
    /// the universe one chunk at a time so it never subscribes to more than
    /// `chunk_size` channels at once, staying below the venue's silent delivery
    /// ceiling.
    pub(crate) fn chunk_universe<T: Clone>(universe: &[T], chunk_size: usize) -> Vec<Vec<T>> {
        if chunk_size == 0 {
            return Vec::new();
        }
        universe.chunks(chunk_size).map(<[T]>::to_vec).collect()
    }

    /// Pass rule for a trade chunk-count readiness probe: at least
    /// `required_live_markets` (`m`) distinct markets must have produced a trade
    /// across the chunk walk, and `m` itself must be >= 1 (a probe that requires
    /// nothing proves nothing, so it fails closed). Single source of truth for
    /// the pass decision, shared by the live probe orchestration and the
    /// operator-artifacts materializer so both agree on what "healthy" means.
    pub(crate) fn trade_chunk_count_probe_passed(
        distinct_fired: usize,
        required_live_markets: usize,
    ) -> bool {
        required_live_markets >= 1 && distinct_fired >= required_live_markets
    }

    fn observed_required_book_delta_count(
        required: &[StrategyFreeReferenceQuoteSubscription],
        book_deltas: &[BoltV3StrategyFreeBookDeltas],
    ) -> usize {
        let mut observed = BTreeSet::new();
        for required in required {
            if book_deltas.iter().any(|deltas| {
                deltas.data_client_id == required.data_client_id.to_string()
                    && deltas.instrument_id == required.instrument_id.to_string()
            }) {
                observed.insert((
                    required.data_client_id.to_string(),
                    required.instrument_id.to_string(),
                ));
            }
        }
        observed.len()
    }

    fn observed_required_quote_count(
        required: &[StrategyFreeReferenceQuoteSubscription],
        quotes: &[BoltV3StrategyFreeReferenceQuote],
    ) -> usize {
        let mut observed = BTreeSet::new();
        for required in required {
            if quotes.iter().any(|quote| {
                quote.data_client_id == required.data_client_id.to_string()
                    && quote.instrument_id == required.instrument_id.to_string()
            }) {
                observed.insert((
                    required.data_client_id.to_string(),
                    required.instrument_id.to_string(),
                ));
            }
        }
        observed.len()
    }

    fn strategy_free_reference_quote_subscription_plan(
        loaded: &LoadedBoltV3Config,
    ) -> (
        Vec<StrategyFreeReferenceQuoteSubscription>,
        BTreeSet<String>,
    ) {
        let mut subscriptions = Vec::new();
        for strategy in &loaded.strategies {
            for reference in strategy.config.reference_data.values() {
                subscriptions.push(StrategyFreeReferenceQuoteSubscription {
                    data_client_id: reference.data_client_id,
                    instrument_id: reference.instrument_id,
                });
            }
        }
        dedupe_strategy_free_reference_quote_subscriptions(subscriptions)
    }

    pub(super) fn strategy_free_data_client_readiness_quote_subscription_plan(
        loaded: &LoadedBoltV3Config,
        client_key: &str,
    ) -> Result<
        (
            Vec<StrategyFreeReferenceQuoteSubscription>,
            BTreeSet<String>,
        ),
        BoltV3LiveNodeError,
    > {
        let client = loaded.root.clients.get(client_key).ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness quote probe client_key is not configured"
            ))
        })?;
        if client.data.is_none() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness quote probe requires the selected client to declare [data]"
                ),
            ));
        }
        let readiness_probe = client.readiness_probe.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
        ))
    })?;
        if readiness_probe.quote_target_source
            != DataClientReadinessProbeQuoteTargetSource::Configured
        {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "standalone data-client readiness quote probe requires quote_target_source = \"configured\"; metadata_response requires the combined data-client readiness probe"
                ),
            ));
        }
        let Some(quote_targets) = &readiness_probe.quote_targets else {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                ),
            ));
        };
        if quote_targets.is_empty() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                ),
            ));
        }
        let subscriptions = quote_targets
            .values()
            .map(|target| StrategyFreeReferenceQuoteSubscription {
                data_client_id: ClientId::from(client_key),
                instrument_id: target.instrument_id,
            })
            .collect();
        Ok(dedupe_strategy_free_reference_quote_subscriptions(
            subscriptions,
        ))
    }

    /// Validates the TOML-owned `readiness_probe.min_observed_targets` lower bound.
    ///
    /// `min_observed_targets` lets a broad metadata universe prove adapter data-path
    /// behaviour by observing fresh data for at least this many sampled targets,
    /// rather than requiring every sampled (and possibly illiquid or un-streamable)
    /// instrument to tick. A configured value of zero would let the probe pass with
    /// no observed data, so it is rejected here. The upper bound against the actual
    /// sampled target count is enforced where that count is known (at build time for
    /// configured targets, at metadata-response install time for sampled targets).
    fn validate_readiness_probe_min_observed_targets(
        readiness_probe: &DataClientReadinessProbeBlock,
    ) -> Result<Option<usize>, BoltV3LiveNodeError> {
        match readiness_probe.min_observed_targets {
            Some(0) => Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "clients.<id>.readiness_probe.min_observed_targets must be a positive integer when configured"
                ),
            )),
            other => Ok(other),
        }
    }

    pub(super) fn strategy_free_data_client_readiness_quote_probe_handle(
        loaded: &LoadedBoltV3Config,
        client_key: &str,
    ) -> Result<BoltV3StrategyFreeReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
        let client = loaded.root.clients.get(client_key).ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness probe client_key is not configured"
            ))
        })?;
        if client.data.is_none() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness probe requires the selected client to declare [data]"
                ),
            ));
        }
        let Some(readiness_probe) = &client.readiness_probe else {
            return Ok(BoltV3StrategyFreeReferenceQuoteProbeHandle::from_plan(
                Vec::new(),
                BTreeSet::new(),
                DataClientReadinessProbeMarketDataKind::Quote,
                None,
            ));
        };
        let min_observed_targets = validate_readiness_probe_min_observed_targets(readiness_probe)?;
        match readiness_probe.quote_target_source {
            DataClientReadinessProbeQuoteTargetSource::Configured => {
                let Some(quote_targets) = &readiness_probe.quote_targets else {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                        ),
                    ));
                };
                if quote_targets.is_empty() {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                        ),
                    ));
                }
                let subscriptions = quote_targets
                    .values()
                    .map(|target| StrategyFreeReferenceQuoteSubscription {
                        data_client_id: ClientId::from(client_key),
                        instrument_id: target.instrument_id,
                    })
                    .collect();
                let (required, ambiguous_instrument_ids) =
                    dedupe_strategy_free_reference_quote_subscriptions(subscriptions);
                if let Some(min_observed) = min_observed_targets
                    && min_observed > required.len()
                {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "clients.<id>.readiness_probe.min_observed_targets={min_observed} exceeds the {} configured readiness_probe.quote_targets",
                            required.len()
                        ),
                    ));
                }
                Ok(BoltV3StrategyFreeReferenceQuoteProbeHandle::from_plan(
                    required,
                    ambiguous_instrument_ids,
                    readiness_probe.market_data_kind,
                    min_observed_targets,
                ))
            }
            DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
                if readiness_probe.market_data_kind == DataClientReadinessProbeMarketDataKind::Trade
                    && readiness_probe.chunk_size.is_some()
                {
                    let chunk_size = match readiness_probe.chunk_size {
                        Some(chunk_size) if chunk_size > 0 => chunk_size,
                        _ => {
                            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                                anyhow::anyhow!(
                                    "trade chunk-count readiness probe requires positive clients.<id>.readiness_probe.chunk_size"
                                ),
                            ));
                        }
                    };
                    let chunk_observation_window_seconds = match readiness_probe
                        .chunk_observation_window_seconds
                    {
                        Some(window) if window > 0 => window,
                        _ => {
                            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                                anyhow::anyhow!(
                                    "trade chunk-count readiness probe requires positive clients.<id>.readiness_probe.chunk_observation_window_seconds"
                                ),
                            ));
                        }
                    };
                    let required_live_markets = match min_observed_targets {
                        Some(required_live_markets) if required_live_markets > 0 => {
                            required_live_markets
                        }
                        _ => {
                            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                                anyhow::anyhow!(
                                    "trade chunk-count readiness probe requires positive clients.<id>.readiness_probe.min_observed_targets (m)"
                                ),
                            ));
                        }
                    };
                    return Ok(
                    BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
                        ClientId::from(client_key),
                        chunk_size,
                        chunk_observation_window_seconds,
                        required_live_markets,
                        readiness_probe.market_data_kind,
                    ),
                );
                }
                let max_quote_targets = readiness_probe.max_metadata_quote_targets.ok_or_else(|| {
                BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.max_metadata_quote_targets when quote_target_source = \"metadata_response\""
                ))
            })?;
                if max_quote_targets == 0 {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "data-client readiness quote probe requires positive clients.<id>.readiness_probe.max_metadata_quote_targets"
                        ),
                    ));
                }
                let allow_target_sampling = readiness_probe
                .allow_metadata_target_sampling
                .ok_or_else(|| {
                    BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                        "data-client readiness quote probe requires clients.<id>.readiness_probe.allow_metadata_target_sampling when quote_target_source = \"metadata_response\""
                    ))
                })?;
                Ok(
                    BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_plan(
                        ClientId::from(client_key),
                        max_quote_targets,
                        allow_target_sampling,
                        readiness_probe.market_data_kind,
                        min_observed_targets,
                    ),
                )
            }
        }
    }

    fn dedupe_strategy_free_reference_quote_subscriptions(
        subscriptions: Vec<StrategyFreeReferenceQuoteSubscription>,
    ) -> (
        Vec<StrategyFreeReferenceQuoteSubscription>,
        BTreeSet<String>,
    ) {
        let mut seen = BTreeSet::new();
        let mut by_instrument: BTreeMap<String, String> = BTreeMap::new();
        let mut ambiguous_instrument_ids = BTreeSet::new();
        let mut deduped = Vec::new();
        for subscription in subscriptions {
            let data_client_id = subscription.data_client_id.to_string();
            let instrument_id = subscription.instrument_id.to_string();
            match by_instrument.get(&instrument_id) {
                Some(existing_data_client_id) if existing_data_client_id != &data_client_id => {
                    ambiguous_instrument_ids.insert(instrument_id.clone());
                }
                None => {
                    by_instrument.insert(instrument_id.clone(), data_client_id.clone());
                }
                _ => {}
            }
            let key = (data_client_id, instrument_id);
            if seen.insert(key) {
                deduped.push(subscription);
            }
        }
        (deduped, ambiguous_instrument_ids)
    }
}

#[cfg(test)]
use strategy_free_probe::*;

impl BoltV3StrategyFreeReferenceCacheEvidence {
    pub fn cached_instrument_ids(&self) -> &[String] {
        &self.cached_instrument_ids
    }
}

#[derive(Debug)]
struct NoStrategyDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for NoStrategyDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        Ok(())
    }

    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        Ok(())
    }

    fn record_admission_decision(&self, _decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        Ok(())
    }
}

struct BoltV3LiveNodeRuntimeFeeds {
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    position_sizer_runtime_feed: Option<Arc<Mutex<PositionSizerRuntimeFeed>>>,
    position_sizer_runtime_feed_subscription: Option<PositionSizerRuntimeFeedSubscription>,
    position_sizer_venue_spendability_source:
        Option<BoltV3PositionSizerVenueSpendabilitySourceConfig>,
    submit_reservation_recovery: Option<BoltV3SubmitReservationRecoveryConfig>,
}

impl BoltV3LiveNodeRuntime {
    fn new(
        node: LiveNode,
        registration_summary: BoltV3RegistrationSummary,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        feeds: BoltV3LiveNodeRuntimeFeeds,
        redaction_values: Vec<Zeroizing<String>>,
    ) -> Self {
        Self {
            node,
            registration_summary,
            submit_admission,
            loss_halt_action_policy: feeds.loss_halt_action_policy,
            loss_runtime_feed: feeds.loss_runtime_feed,
            loss_runtime_feed_subscription: feeds.loss_runtime_feed_subscription,
            position_sizer_runtime_feed: feeds.position_sizer_runtime_feed,
            position_sizer_runtime_feed_subscription: feeds
                .position_sizer_runtime_feed_subscription,
            position_sizer_venue_spendability_source: feeds
                .position_sizer_venue_spendability_source,
            submit_reservation_recovery: feeds.submit_reservation_recovery,
            redaction_values,
        }
    }

    pub fn registered_strategy_ids(&self) -> Vec<StrategyId> {
        self.node.kernel().trader().borrow().strategy_ids()
    }

    pub fn environment(&self) -> Environment {
        self.node.environment()
    }

    pub fn state(&self) -> NodeState {
        self.node.state()
    }

    pub fn registered_data_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().data_engine.borrow().registered_clients()
    }

    pub fn registration_summary(&self) -> &BoltV3RegistrationSummary {
        &self.registration_summary
    }

    pub fn registered_exec_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().exec_engine.borrow().client_ids()
    }

    pub fn cached_instrument_ids(&self) -> Vec<String> {
        self.reference_cache_evidence().cached_instrument_ids
    }

    pub fn reference_cache_evidence(&self) -> BoltV3StrategyFreeReferenceCacheEvidence {
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        let cached_instrument_ids = cache
            .instrument_ids(None)
            .into_iter()
            .map(ToString::to_string)
            .collect();
        BoltV3StrategyFreeReferenceCacheEvidence {
            cached_instrument_ids,
        }
    }

    pub fn redaction_values(&self) -> &[Zeroizing<String>] {
        &self.redaction_values
    }

    pub fn instance_id(&self) -> String {
        self.node.instance_id().to_string()
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.submit_admission.admitted_order_count()
    }

    pub fn loss_governor_configured(&self) -> bool {
        self.submit_admission.loss_governor_configured()
    }

    pub fn loss_governor_runtime_feed_configured(&self) -> bool {
        self.loss_runtime_feed.is_some() && self.loss_runtime_feed_subscription.is_some()
    }

    pub fn nt_risk_trading_state(&self) -> TradingState {
        self.node.kernel().risk_engine().borrow().trading_state()
    }

    pub fn apply_loss_governor_manual_recovery(
        &self,
        evidence: &LossGovernorManualRecoveryEvidence,
        now_ns: u64,
    ) -> Option<TradingState> {
        let loss_policy = self.submit_admission.loss_governor_policy()?;
        let action_policy = self.loss_halt_action_policy.as_ref()?;
        let snapshot = self.submit_admission.loss_snapshot();
        let decision = evaluate_loss_admission(&loss_policy, snapshot.as_ref(), now_ns);
        let current_state = self.nt_risk_trading_state();
        let target =
            next_loss_governor_manual_recovery_trading_state(LossGovernorManualRecoveryRequest {
                policy: action_policy,
                current_state,
                decision: &decision,
                snapshot: snapshot.as_ref(),
                now_ns,
                max_snapshot_age_ns: loss_policy.max_snapshot_age_ns,
                evidence: Some(evidence),
                max_evidence_path_bytes: action_policy.manual_recovery_evidence_max_path_bytes,
            })?;
        self.node
            .kernel()
            .risk_engine()
            .borrow_mut()
            .set_trading_state(target);
        Some(target)
    }

    pub fn position_sizer_configured(&self) -> bool {
        self.submit_admission.position_sizer_configured()
    }

    pub fn position_sizer_runtime_feed_configured(&self) -> bool {
        self.position_sizer_runtime_feed.is_some()
            && self.position_sizer_runtime_feed_subscription.is_some()
    }

    pub fn refresh_position_sizer_venue_spendability_from_configured_source(
        &self,
    ) -> Result<Option<BoltV3SubmitPositionSizingNtComponents>, BoltV3LiveNodeError> {
        let Some(config) = self.position_sizer_venue_spendability_source.as_ref() else {
            return Ok(None);
        };
        let Some(feed) = self.position_sizer_runtime_feed.as_ref() else {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "position sizer venue spendability source configured without runtime feed"
            )));
        };
        refresh_position_sizer_venue_spendability_from_source(feed, config)
    }

    pub fn position_sizer_reconciled(&self) -> Option<bool> {
        self.submit_admission.position_sizer_reconciled()
    }

    /// Rebuild the position sizer's capital-reservation ledger from the
    /// live NT cache at startup so a restart cannot double-allocate capital
    /// against orders/positions that already exist. Reads open orders,
    /// the configured collateral balance, and open positions for the
    /// configured account, seeds the runtime feed's portfolio/cache
    /// snapshot from that same observation, then attributes each open order
    /// to recovered reservation metadata (when configured) before handing a
    /// single coherent snapshot to submit admission. If any open order
    /// cannot be attributed, the snapshot is marked not-all-attributed so
    /// the caller fails closed rather than arming with an unreconciled
    /// ledger.
    pub fn rebuild_position_sizer_from_nt_cache(
        &self,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingRebuildDecision {
        let (account_id, binary_instrument_ids, collateral_currency) =
            match self.position_sizer_runtime_feed.as_ref() {
                Some(feed) => {
                    let feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        Some(feed.configured_account_id()),
                        feed.configured_binary_instrument_ids(),
                        Some(feed.configured_collateral_currency()),
                    )
                }
                None => (None, None, None),
            };
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        let open_order_snapshots = match account_id.as_ref() {
            Some(account_id) => cache
                .orders_open(None, None, None, Some(account_id), None)
                .into_iter()
                .map(|order| order.cloned())
                .collect::<Vec<_>>(),
            None => cache
                .orders_open(None, None, None, None, None)
                .into_iter()
                .map(|order| order.cloned())
                .collect::<Vec<_>>(),
        };
        let open_client_order_ids = open_order_snapshots
            .iter()
            .map(|order| order.client_order_id().to_string())
            .collect::<Vec<_>>();
        let cached_account_balances = match (account_id.as_ref(), collateral_currency.as_deref()) {
            (Some(account_id), Some(collateral_currency)) => {
                cache.account_owned(account_id).and_then(|account| {
                    let balances = account.balances();
                    balances
                        .values()
                        .find(|balance| balance.currency.code.as_str() == collateral_currency)
                        .map(|balance| (balance.free.as_decimal(), balance.total.as_decimal()))
                })
            }
            _ => None,
        };
        let missing_nt_account_cache_balance = match (
            account_id.as_ref(),
            collateral_currency.as_deref(),
        ) {
            (Some(account_id), Some(collateral_currency)) if cached_account_balances.is_none() => {
                let missing = BoltV3SubmitPositionSizingMissingNtAccountCacheBalance {
                    account_id: account_id.to_string(),
                    collateral_currency: collateral_currency.to_string(),
                };
                log::warn!(
                    "bolt-v3 position sizer startup rebuild could not seed account portfolio snapshot because NT cache is missing account_id={} collateral_currency={}",
                    missing.account_id,
                    missing.collateral_currency
                );
                Some(missing)
            }
            _ => None,
        };
        let (yes_position, no_position) =
            match (account_id.as_ref(), binary_instrument_ids.as_ref()) {
                (Some(account_id), Some((yes_instrument_id, no_instrument_id))) => {
                    let mut yes_position = Decimal::ZERO;
                    let mut no_position = Decimal::ZERO;
                    for position in cache.positions_open(None, None, None, Some(account_id), None) {
                        let instrument_id = position.instrument_id.to_string();
                        if instrument_id == *yes_instrument_id {
                            yes_position += position.signed_decimal_qty();
                        } else if instrument_id == *no_instrument_id {
                            no_position += position.signed_decimal_qty();
                        }
                    }
                    (yes_position, no_position)
                }
                _ => (Decimal::ZERO, Decimal::ZERO),
            };
        drop(cache);

        let account_cache_is_authoritative = cached_account_balances.is_some();
        // Seed live NT order and position state before rebuilding reservations from the same snapshot.
        if let Some(feed) = self.position_sizer_runtime_feed.as_ref() {
            let mut feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((free_collateral, total_equity)) = cached_account_balances {
                feed.seed_account_portfolio_snapshot(free_collateral, total_equity, now_ns);
            }
            if account_cache_is_authoritative || !open_client_order_ids.is_empty() {
                feed.seed_cache_snapshot(
                    open_client_order_ids.clone(),
                    yes_position,
                    no_position,
                    now_ns,
                );
            }
        }

        let recovered_reservations = if open_order_snapshots.is_empty() {
            None
        } else {
            self.submit_reservation_recovery
                .as_ref()
                .and_then(|config| {
                    match read_submit_reservation_recovery_evidence(&config.path, config.max_bytes)
                    {
                        Ok(recovery) => Some(recovery),
                        Err(error) => {
                            log::warn!(
                                "bolt-v3 submit admission could not recover Bolt reservation metadata from decision evidence: {error:#}"
                            );
                            None
                        }
                    }
                })
        };
        let mut reservations = Vec::with_capacity(open_order_snapshots.len());
        let mut all_open_orders_attributed =
            open_order_snapshots.is_empty() || recovered_reservations.is_some();
        for order in &open_order_snapshots {
            let Some(recovered_reservations) = recovered_reservations.as_ref() else {
                all_open_orders_attributed = false;
                break;
            };
            let Some(evidence) = nt_open_order_evidence_from_order(order, now_ns) else {
                all_open_orders_attributed = false;
                break;
            };
            let Some(recovered) = recovered_reservations
                .metadata_by_client_order_id
                .get(&evidence.client_order_id)
            else {
                all_open_orders_attributed = false;
                break;
            };
            let Some(reservation) = self
                .submit_admission
                .position_sizing_open_order_reservation_from_known_metadata(evidence, recovered)
            else {
                all_open_orders_attributed = false;
                break;
            };
            reservations.push(reservation);
        }
        if !all_open_orders_attributed {
            reservations.clear();
        }

        let mut rebuild = self
            .submit_admission
            .rebuild_position_sizing_open_order_snapshot(
                BoltV3SubmitPositionSizingOpenOrderSnapshot {
                    observed_at_ns: now_ns,
                    evidence_label: "nt_open_order_cache".to_string(),
                    observed_open_order_count: open_order_snapshots.len(),
                    all_open_orders_attributed,
                    reservations,
                },
                now_ns,
            );
        if let Some(missing) = missing_nt_account_cache_balance {
            rebuild = rebuild.with_missing_nt_account_cache_balance(
                missing.account_id,
                missing.collateral_currency,
            );
        }
        rebuild
    }
}

fn nt_open_order_evidence_from_order(
    order: &OrderAny,
    observed_at_ns: u64,
) -> Option<BoltV3SubmitPositionSizingOpenOrderEvidence> {
    if order.order_type() != OrderType::Limit {
        return None;
    }
    let side = match order.order_side() {
        OrderSide::Buy => BoltV3CompiledOrderSide::Buy,
        OrderSide::Sell => BoltV3CompiledOrderSide::Sell,
        _ => return None,
    };
    let limit_price = order.price()?.as_decimal();
    if !(Decimal::ZERO..=Decimal::ONE).contains(&limit_price) {
        return None;
    }
    let open_quantity = order.leaves_qty().as_decimal();
    if open_quantity <= Decimal::ZERO {
        return None;
    }
    Some(BoltV3SubmitPositionSizingOpenOrderEvidence {
        client_order_id: order.client_order_id().to_string(),
        instrument_id: order.instrument_id().to_string(),
        side,
        open_quantity,
        limit_price,
        observed_at_ns,
        evidence_label: "nt_open_order_cache".to_string(),
    })
}

impl std::fmt::Debug for BoltV3LiveNodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoltV3LiveNodeRuntime")
            .field("node", &"[redacted]")
            .field("submit_admission", &self.submit_admission)
            .field("redaction_values", &"[redacted]")
            .finish()
    }
}

#[derive(Debug)]
pub enum BoltV3LiveNodeBuilderError {
    BuilderConstruction { source: anyhow::Error },
}

impl std::fmt::Display for BoltV3LiveNodeBuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => {
                write!(f, "NT LiveNodeBuilder construction failed: {source}")
            }
        }
    }
}

impl std::error::Error for BoltV3LiveNodeBuilderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => Some(source.as_ref()),
        }
    }
}

#[derive(Debug)]
pub enum BoltV3LiveNodeError {
    ForbiddenEnv(ForbiddenEnvVarError),
    /// `SsmResolverSession::new()` failed before any client secret was
    /// read. The wrapped `SecretError` is the upstream Tokio /
    /// AWS-SDK-config setup failure. Distinct from
    /// [`SecretResolution`] (which carries a per-client `BoltV3SecretError`
    /// with client key, secret-config field name, and SSM path) because
    /// session setup happens before any client path is consulted, so an
    /// operator message that names a client or SSM path would be wrong.
    SecretResolverSetup(crate::secrets::SecretError),
    SecretResolution(BoltV3SecretError),
    AdapterMapping(BoltV3AdapterMappingError),
    BuilderConstruction(BoltV3LiveNodeBuilderError),
    ClientRegistration(BoltV3ClientRegistrationError),
    StrategyRegistration(BoltV3StrategyRegistrationError),
    RiskPolicy(anyhow::Error),
    Build(anyhow::Error),
    /// Provider-specific live-submit approval loading or consumption failed
    /// while building the adapter bundle. This is intentionally outside the
    /// live runner wrapper; production `run_bolt_v3_live_node` still enters NT
    /// without reintroducing the removed start gate.
    OperatorApprovalConsumption(anyhow::Error),
    /// The loaded root TOML configured clients beyond the selected
    /// strategy-owned transport path, but the strategy-owned
    /// execution/reference client set could not be derived or validated
    /// against `[clients]`.
    LiveTransportScope {
        reason: String,
    },
    /// NT returned an error from `LiveNode::run`.
    Run(anyhow::Error),
    /// NT runtime capture could not be wired from the validated
    /// bolt-v3 `[persistence]` config before the runner loop started.
    RuntimeCaptureWire(anyhow::Error),
    /// NT runtime capture failed during shutdown after the runner loop
    /// exited or after the capture worker asked the LiveNode to stop.
    RuntimeCaptureShutdown(anyhow::Error),
    /// NT's runner loop and runtime-capture shutdown both failed. This
    /// preserves both failure categories instead of reporting the
    /// compound case as only a capture-shutdown error.
    RunAndRuntimeCaptureShutdown {
        run_error: anyhow::Error,
        shutdown_error: anyhow::Error,
    },
    /// The bolt-v3 controlled-connect boundary
    /// ([`connect_bolt_v3_clients`]) bounds the dispatched
    /// `NautilusKernel::connect_data_clients` and
    /// `NautilusKernel::connect_exec_clients` calls by the
    /// `nautilus.timeout_connection_secs` value from the loaded
    /// bolt-v3 config. A `ConnectTimeout` is surfaced when that bound
    /// elapses before NT's engine-level connect dispatchers return,
    /// instead of the controlled-connect call hanging indefinitely.
    /// The wrapped value is the configured timeout the boundary
    /// applied (in seconds), captured so log/audit consumers can
    /// distinguish a 1-second test timeout from a 30-second
    /// production timeout without re-reading the source config.
    ConnectTimeout {
        timeout_secs: u64,
    },
    /// The bolt-v3 controlled-connect boundary dispatched both NT
    /// engine-level connect futures within the configured bound, but
    /// at least one registered NT data or execution client did not
    /// transition to `is_connected` afterwards. The pinned NT
    /// `DataEngine::connect` and `ExecutionEngine::connect`
    /// dispatchers swallow individual client `connect()` errors and
    /// only log them, so bolt-v3 consults
    /// `NautilusKernel::check_engines_connected()` after dispatch
    /// returns to keep this failure mode honest. This slice keeps the
    /// variant generic rather than synthesizing a per-client failure
    /// list. Callers should follow this with a
    /// [`disconnect_bolt_v3_clients`] call to drain any partially
    /// connected clients under the bounded controlled-disconnect
    /// boundary.
    ConnectIncomplete,
    /// The bolt-v3 controlled-disconnect boundary
    /// ([`disconnect_bolt_v3_clients`]) bounds the
    /// `NautilusKernel::disconnect_clients` future by the
    /// `nautilus.timeout_disconnection_secs` value from the loaded
    /// bolt-v3 config. A `DisconnectTimeout` is surfaced when that
    /// bound elapses before NT finishes disconnecting all data and
    /// execution clients, instead of the controlled-disconnect call
    /// hanging indefinitely. The wrapped value is the configured
    /// timeout the boundary applied (in seconds).
    DisconnectTimeout {
        timeout_secs: u64,
    },
    /// The bolt-v3 controlled-disconnect boundary dispatched
    /// `NautilusKernel::disconnect_clients` and NT returned an
    /// `Err(..)` from at least one registered client's `disconnect()`
    /// call. The wrapped `anyhow::Error` is the value NT bubbled up
    /// from its engine-level disconnect aggregator.
    DisconnectFailed(anyhow::Error),
    StrategyFreeStartTimeout {
        timeout_secs: u64,
    },
    StrategyFreeStartTimeoutOverflow,
    StrategyFreeStartIncomplete,
    StrategyFreeExecutionAccountsMissing {
        client_venues: Vec<String>,
    },
    StrategyFreeReferenceProbeSetup(anyhow::Error),
    StrategyFreeReferenceProbeFailed {
        reason: String,
    },
    StrategyFreeDataClientProbeFailed {
        reason: String,
    },
    StrategyFreeStartFailed(anyhow::Error),
    StrategyFreeStopTimeout {
        timeout_secs: u64,
    },
    StrategyFreeStopTimeoutOverflow,
    StrategyFreeStopFailed(anyhow::Error),
    /// The startup position-sizer rebuild from the NT cache could not
    /// attribute one or more pre-existing open orders to recovered
    /// submit-reservation metadata, so submit admission would arm with an
    /// unreconciled capital-reservation ledger. The live runner refuses to
    /// enter NT's loop in this state to avoid double-allocating capital
    /// against orders it cannot account for. The wrapped decision carries
    /// the attempted/rebuilt reservation counts and rejection reason
    /// captured at boot. This is intentionally outside the removed start
    /// gate: it is a fail-closed reconciliation guard, not the live-canary
    /// arm gate, so it never reintroduces a gate-report/arm sequence.
    StartupPositionSizerRebuild(BoltV3SubmitPositionSizingRebuildDecision),
}

impl std::fmt::Display for BoltV3LiveNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::SecretResolverSetup(error) => write!(
                f,
                "bolt-v3 SSM resolver session setup failed before any client \
                 secret could be read: {error}"
            ),
            BoltV3LiveNodeError::SecretResolution(error) => {
                write!(f, "bolt-v3 secret resolution failed: {error}")
            }
            BoltV3LiveNodeError::AdapterMapping(error) => {
                write!(f, "bolt-v3 adapter config mapping failed: {error}")
            }
            BoltV3LiveNodeError::BuilderConstruction(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::ClientRegistration(error) => {
                write!(f, "bolt-v3 client registration failed: {error}")
            }
            BoltV3LiveNodeError::StrategyRegistration(error) => {
                write!(f, "bolt-v3 strategy registration failed: {error}")
            }
            BoltV3LiveNodeError::RiskPolicy(error) => {
                write!(f, "bolt-v3 risk policy mapping failed: {error}")
            }
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => {
                write!(
                    f,
                    "bolt-v3 provider live-submit approval consumption failed: {error}"
                )
            }
            BoltV3LiveNodeError::LiveTransportScope { reason } => write!(
                f,
                "bolt-v3 live transport scope could not be derived from strategy-owned client bindings: {reason}"
            ),
            BoltV3LiveNodeError::Run(error) => write!(f, "LiveNode run failed: {error}"),
            BoltV3LiveNodeError::RuntimeCaptureWire(error) => {
                write!(f, "NT runtime capture wiring failed: {error}")
            }
            BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => {
                write!(f, "NT runtime capture shutdown failed: {error}")
            }
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            } => write!(
                f,
                "LiveNode run failed and NT runtime capture shutdown failed: \
                 run error: {run_error}; shutdown error: {shutdown_error}"
            ),
            BoltV3LiveNodeError::ConnectTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 controlled-connect exceeded the configured \
                 nautilus.timeout_connection_secs bound ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::ConnectIncomplete => write!(
                f,
                "bolt-v3 controlled-connect dispatched both NT engine-level connect \
                 futures within the configured bound but `kernel.check_engines_connected()` \
                 returned false; at least one registered NT data or execution client did \
                 not transition to is_connected after NT swallowed/logged its connect error"
            ),
            BoltV3LiveNodeError::DisconnectTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 controlled-disconnect exceeded the configured \
                 nautilus.timeout_disconnection_secs bound ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::DisconnectFailed(error) => write!(
                f,
                "bolt-v3 controlled-disconnect surfaced an NT engine-level disconnect \
                 aggregator error: {error}"
            ),
            BoltV3LiveNodeError::StrategyFreeStartTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 strategy-free controlled-start exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow => write!(
                f,
                "bolt-v3 strategy-free controlled-start timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::StrategyFreeStartIncomplete => write!(
                f,
                "bolt-v3 strategy-free controlled-run exited before NT reached Running \
                 with required startup evidence"
            ),
            BoltV3LiveNodeError::StrategyFreeExecutionAccountsMissing { client_venues } => write!(
                f,
                "bolt-v3 strategy-free controlled-run reached NT Running but required execution \
                 account evidence was absent from NT cache for: {}",
                client_venues.join(", ")
            ),
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(error) => write!(
                f,
                "bolt-v3 strategy-free reference quote probe setup failed: {error}"
            ),
            BoltV3LiveNodeError::StrategyFreeReferenceProbeFailed { reason } => write!(
                f,
                "bolt-v3 strategy-free controlled-run reached NT Running but live reference quote evidence was not observed; engine connectivity cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason } => write!(
                f,
                "bolt-v3 strategy-free controlled-run reached NT Running but data-client readiness evidence was not observed; data-client production readiness cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::StrategyFreeStartFailed(error) => {
                write!(f, "bolt-v3 strategy-free controlled-start failed: {error}")
            }
            BoltV3LiveNodeError::StrategyFreeStopTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 strategy-free controlled-stop exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow => write!(
                f,
                "bolt-v3 strategy-free controlled-stop timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::StrategyFreeStopFailed(error) => {
                write!(f, "bolt-v3 strategy-free controlled-stop failed: {error}")
            }
            BoltV3LiveNodeError::StartupPositionSizerRebuild(decision) => write!(
                f,
                "bolt-v3 startup position-sizer rebuild rejected runtime start: {decision:?}"
            ),
        }
    }
}

impl std::error::Error for BoltV3LiveNodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => Some(error),
            BoltV3LiveNodeError::SecretResolverSetup(error) => Some(error),
            BoltV3LiveNodeError::SecretResolution(error) => Some(error),
            BoltV3LiveNodeError::AdapterMapping(error) => Some(error),
            BoltV3LiveNodeError::BuilderConstruction(error) => Some(error),
            BoltV3LiveNodeError::ClientRegistration(error) => Some(error),
            BoltV3LiveNodeError::StrategyRegistration(error) => Some(error),
            BoltV3LiveNodeError::RiskPolicy(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::Build(error) => error.source(),
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::Run(error) => error.source(),
            BoltV3LiveNodeError::RuntimeCaptureWire(error)
            | BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => error.source(),
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown { run_error, .. } => {
                Some(run_error.as_ref())
            }
            BoltV3LiveNodeError::ConnectTimeout { .. }
            | BoltV3LiveNodeError::ConnectIncomplete
            | BoltV3LiveNodeError::DisconnectTimeout { .. }
            | BoltV3LiveNodeError::LiveTransportScope { .. }
            | BoltV3LiveNodeError::StrategyFreeStartTimeout { .. }
            | BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow
            | BoltV3LiveNodeError::StrategyFreeStartIncomplete
            | BoltV3LiveNodeError::StrategyFreeExecutionAccountsMissing { .. }
            | BoltV3LiveNodeError::StrategyFreeReferenceProbeFailed { .. }
            | BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { .. }
            | BoltV3LiveNodeError::StrategyFreeStopTimeout { .. }
            | BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow
            | BoltV3LiveNodeError::StartupPositionSizerRebuild(..) => None,
            BoltV3LiveNodeError::DisconnectFailed(error)
            | BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(error)
            | BoltV3LiveNodeError::StrategyFreeStartFailed(error)
            | BoltV3LiveNodeError::StrategyFreeStopFailed(error) => Some(error.as_ref()),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded = trade_transport_loaded_config(loaded)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    let bundle =
        live_node_adapter_bundle_with_provider_live_submit_approvals(&transport_loaded, &resolved)?;
    let (runtime, _summary) = build_live_node_with_clients_and_submit_approval_limits(
        &transport_loaded,
        &resolved,
        bundle.configs,
        bundle.live_submit_approval_limits,
    )?;
    Ok(runtime)
}

fn resolve_bolt_v3_live_node_secrets(
    loaded: &LoadedBoltV3Config,
) -> Result<ResolvedBoltV3Secrets, BoltV3LiveNodeError> {
    check_no_forbidden_credential_env_vars(&loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    // Per #252 design review: own the resolver session at the bolt-v3
    // startup boundary so a single AWS SDK config + SsmClient cache covers
    // every secret resolution in this build, and so the session lifetime is
    // visible to the caller of `resolve_bolt_v3_secrets`. Session-setup
    // failure surfaces as the dedicated `SecretResolverSetup` variant
    // (#255-2) so operator-facing messages don't pretend a venue or SSM
    // path is involved before any path has been read.
    let session = SsmResolverSession::new().map_err(BoltV3LiveNodeError::SecretResolverSetup)?;
    resolve_bolt_v3_secrets(&session, loaded).map_err(BoltV3LiveNodeError::SecretResolution)
}

fn live_node_adapter_bundle_with_provider_live_submit_approvals(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeAdapterBundle, BoltV3LiveNodeError> {
    if configured_provider_live_submit_client_count(loaded)? == 0 {
        return Ok(BoltV3LiveNodeAdapterBundle {
            configs: map_bolt_v3_adapters(loaded, resolved)
                .map_err(BoltV3LiveNodeError::AdapterMapping)?,
            live_submit_approval_limits: BTreeMap::new(),
        });
    }
    let build_head_sha = current_build_head_sha().ok_or_else(|| {
        BoltV3LiveNodeError::OperatorApprovalConsumption(anyhow::anyhow!(
            "bolt-v3 build head_sha is unavailable or invalid"
        ))
    })?;
    let now_unix_seconds = current_unix_seconds_u64()?;
    live_node_adapter_bundle_with_provider_approvals_at(
        loaded,
        resolved,
        now_unix_seconds,
        build_head_sha,
    )
}

fn live_node_adapter_bundle_with_provider_approvals_at(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    now_unix_seconds: u64,
    build_head_sha: &str,
) -> Result<BoltV3LiveNodeAdapterBundle, BoltV3LiveNodeError> {
    let approvals = load_provider_live_submit_approvals_for_live_node(
        loaded,
        resolved,
        now_unix_seconds,
        build_head_sha,
    )?;
    if approvals.is_empty() {
        return Ok(BoltV3LiveNodeAdapterBundle {
            configs: map_bolt_v3_adapters(loaded, resolved)
                .map_err(BoltV3LiveNodeError::AdapterMapping)?,
            live_submit_approval_limits: BTreeMap::new(),
        });
    }
    let configs = map_bolt_v3_adapters_with_runtime_approvals(
        loaded,
        resolved,
        ProviderRuntimeApprovals {
            live_submit: Some(&approvals),
        },
    )
    .map_err(BoltV3LiveNodeError::AdapterMapping)?;
    Ok(BoltV3LiveNodeAdapterBundle {
        configs,
        live_submit_approval_limits: live_submit_approval_limits_for_submit_admission(&approvals),
    })
}

fn live_submit_approval_limits_for_submit_admission(
    approvals: &ProviderLiveSubmitApprovals,
) -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
    approvals
        .order_limits()
        .map(|(client_key, order_limits)| {
            (
                client_key.clone(),
                BoltV3LiveSubmitApprovalLimits {
                    max_order_count: order_limits.max_order_count,
                    max_order_notional: order_limits.max_order_notional,
                },
            )
        })
        .collect()
}

fn configured_provider_live_submit_client_count(
    loaded: &LoadedBoltV3Config,
) -> Result<usize, BoltV3LiveNodeError> {
    let mut count = 0;
    for client in loaded.root.clients.values() {
        let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
        else {
            continue;
        };
        if binding.load_live_submit_approval.is_some() && client.execution.is_some() {
            count += 1;
        }
    }
    Ok(count)
}

fn load_provider_live_submit_approvals_for_live_node(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    now_unix_seconds: u64,
    build_head_sha: &str,
) -> Result<ProviderLiveSubmitApprovals, BoltV3LiveNodeError> {
    let mut approvals = ProviderLiveSubmitApprovals::empty();
    for (client_key, client) in &loaded.root.clients {
        let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
        else {
            continue;
        };
        let Some(load_live_submit_approval) = binding.load_live_submit_approval else {
            continue;
        };
        if let Some(approval) = load_live_submit_approval(ProviderLiveSubmitApprovalContext {
            loaded,
            client_key,
            client,
            resolved,
            now_unix_seconds,
            build_head_sha,
        })
        .map_err(BoltV3LiveNodeError::OperatorApprovalConsumption)?
        {
            approvals.insert(client_key.clone(), approval);
        }
    }
    Ok(approvals)
}

fn current_unix_seconds_u64() -> Result<u64, BoltV3LiveNodeError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| {
            BoltV3LiveNodeError::OperatorApprovalConsumption(anyhow::Error::new(source))
        })?
        .as_secs())
}

fn current_unix_nanos() -> Result<u64> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    u64::try_from(nanos).map_err(|_| anyhow::anyhow!("current unix nanoseconds exceed u64"))
}

pub fn build_bolt_v3_strategy_free_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded = trade_transport_loaded_config(loaded)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    let adapters = strategy_free_transport_adapter_configs(&transport_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&transport_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)?;
    Ok(runtime)
}

pub fn build_bolt_v3_strategy_free_data_client_probe_live_node(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<(BoltV3LiveNodeRuntime, LoadedBoltV3Config), BoltV3LiveNodeError> {
    let probe_loaded = data_client_probe_loaded_config(loaded, client_key)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&probe_loaded)?;
    let adapters = strategy_free_transport_adapter_configs(&probe_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&probe_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)?;
    Ok((runtime, strategy_free_loaded))
}

pub fn build_bolt_v3_all_configured_client_mapping_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let resolved = resolve_bolt_v3_live_node_secrets(loaded)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let mapping_loaded = strategy_free_transport_loaded_config(loaded);
    let (runtime, _summary) = build_live_node_with_clients(&mapping_loaded, &resolved, adapters)?;
    Ok(runtime)
}

fn strategy_free_transport_adapter_configs(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3LiveNodeError> {
    map_bolt_v3_adapters(loaded, resolved).map_err(BoltV3LiveNodeError::AdapterMapping)
}

fn trade_transport_loaded_config(
    loaded: &LoadedBoltV3Config,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    let required_clients = trade_transport_client_keys(loaded);
    if required_clients.is_empty() {
        let mut transport_loaded = loaded.clone();
        transport_loaded.root.clients.clear();
        return Ok(transport_loaded);
    }
    let missing_clients = required_clients
        .iter()
        .filter(|client_key| !loaded.root.clients.contains_key(*client_key))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_clients.is_empty() {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy references unconfigured client(s): {}",
                missing_clients.join(", ")
            ),
        });
    }

    let mut transport_loaded = loaded.clone();
    transport_loaded
        .root
        .clients
        .retain(|client_key, _| required_clients.contains(client_key));
    Ok(transport_loaded)
}

fn trade_transport_client_keys(loaded: &LoadedBoltV3Config) -> BTreeSet<String> {
    let mut client_keys = BTreeSet::new();
    for strategy in &loaded.strategies {
        client_keys.insert(strategy.config.execution_client_id.to_string());
        for reference in strategy.config.reference_data.values() {
            client_keys.insert(reference.data_client_id.to_string());
        }
        for signal in strategy.config.signal_data.values() {
            client_keys.insert(signal.data_client_id.to_string());
        }
        if let Some(resolution) = strategy.config.resolution_data.as_ref() {
            client_keys.insert(resolution.data_client_id.to_string());
        }
    }
    client_keys
}

fn data_client_probe_loaded_config(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    if client_key.trim().is_empty() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        });
    }
    let client = loaded
        .root
        .clients
        .get(client_key)
        .cloned()
        .ok_or_else(|| BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe requires the selected client to declare [data]".to_string(),
        });
    }
    let mut probe_loaded = loaded.clone();
    probe_loaded
        .root
        .clients
        .retain(|configured_key, _| configured_key == client_key);
    probe_loaded
        .strategies
        .retain(|strategy| strategy.config.execution_client_id == ClientId::from(client_key));
    Ok(probe_loaded)
}

fn strategy_free_transport_loaded_config(loaded: &LoadedBoltV3Config) -> LoadedBoltV3Config {
    let mut strategy_free_loaded = loaded.clone();
    strategy_free_loaded.strategies.clear();
    strategy_free_loaded
}

/// Single bolt-v3 entrypoint for entering NT's runner loop.
///
/// The caller builds the `LiveNode` separately, then this function enters the
/// NT runner loop through the bolt-v3 wrapper that owns runtime capture and
/// shutdown classification. Production callers must use this wrapper rather
/// than invoking the NT runner method directly.
pub async fn run_bolt_v3_live_node(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let startup_rebuild_observed_at_ns =
        current_unix_nanos().map_err(BoltV3LiveNodeError::Build)?;
    let startup_rebuild =
        runtime.rebuild_position_sizer_from_nt_cache(startup_rebuild_observed_at_ns);
    // A no-open-order startup may legitimately recover nothing: NT only
    // populates the account/portfolio cache once its runner loop performs
    // startup reconciliation, and the live runtime feed re-seeds the
    // portfolio from on_account events after entry. Pre-existing open orders
    // are different: if they cannot be attributed to recovered reservation
    // metadata, submit admission would start with an unreconciled ledger and
    // could double-allocate capital, so fail closed before entering NT's
    // loop. This is a reconciliation guard, not the removed start gate.
    if !startup_rebuild.accepted && startup_rebuild.attempted_reservation_count > 0 {
        return Err(BoltV3LiveNodeError::StartupPositionSizerRebuild(
            startup_rebuild,
        ));
    }
    let node = &mut runtime.node;
    let node_handle = node.handle();
    let mut capture_guards = wire_bolt_v3_runtime_capture(node, node_handle, loaded)
        .map_err(BoltV3LiveNodeError::RuntimeCaptureWire)?;
    let mut capture_failure_receiver = capture_guards.take_failure_receiver();

    let run_result = {
        let run_future = node.run();
        tokio::pin!(run_future);

        if let Some(receiver) = capture_failure_receiver.as_mut() {
            tokio::select! {
                result = &mut run_future => result,
                _ = receiver => {
                    log::error!("NT runtime capture failure detected, awaiting LiveNode shutdown");
                    run_future.await
                }
            }
        } else {
            run_future.await
        }
    };
    let shutdown_result = capture_guards.shutdown().await;

    classify_live_node_run_and_capture_shutdown(run_result, shutdown_result)
}

#[cfg(test)]
fn strategy_free_start_timeout_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_connection_secs
        .checked_add(loaded.root.nautilus.timeout_reconciliation_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_portfolio_secs))
        .ok_or(BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow)
}

#[cfg(test)]
fn strategy_free_stop_timeout_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .ok_or(BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow)
}

fn classify_live_node_run_and_capture_shutdown(
    run_result: Result<(), anyhow::Error>,
    shutdown_result: Result<(), anyhow::Error>,
) -> Result<(), BoltV3LiveNodeError> {
    match (run_result, shutdown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run_error), Ok(())) => Err(BoltV3LiveNodeError::Run(run_error)),
        (Ok(()), Err(shutdown_error)) => {
            Err(BoltV3LiveNodeError::RuntimeCaptureShutdown(shutdown_error))
        }
        (Err(run_error), Err(shutdown_error)) => {
            log::error!("Live node run error during NT runtime capture shutdown: {run_error}");
            Err(BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            })
        }
    }
}

/// Test-friendly variant of [`build_bolt_v3_live_node`] which lets the caller
/// inject the forbidden-environment predicate and the SSM resolver. Production
/// code must use [`build_bolt_v3_live_node`], which applies the real credential
/// environment guard and invokes the real Amazon Web Services Systems Manager
/// resolver.
pub fn build_bolt_v3_live_node_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let (runtime, _summary) = build_bolt_v3_live_node_with_summary(loaded, env_is_set, resolver)?;
    Ok(runtime)
}

/// Same as [`build_bolt_v3_live_node_with`] but also returns the
/// [`BoltV3RegistrationSummary`] so tests can assert which NT client
/// kinds the registration boundary added before the builder finalized
/// the node. Not intended for production code paths; production reads
/// the summary by other means if it ever needs to.
pub fn build_bolt_v3_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let transport_loaded = trade_transport_loaded_config(loaded)?;
    check_no_forbidden_credential_env_vars_with(&transport_loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(&transport_loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let bundle =
        live_node_adapter_bundle_with_provider_live_submit_approvals(&transport_loaded, &resolved)?;
    build_live_node_with_clients_and_submit_approval_limits(
        &transport_loaded,
        &resolved,
        bundle.configs,
        bundle.live_submit_approval_limits,
    )
}

pub fn build_bolt_v3_all_configured_client_mapping_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let mapping_loaded = strategy_free_transport_loaded_config(loaded);
    build_live_node_with_clients(&mapping_loaded, &resolved, adapters)
}

fn build_live_node_with_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    build_live_node_with_clients_and_submit_approval_limits(
        loaded,
        resolved,
        adapters,
        BTreeMap::new(),
    )
}

fn build_live_node_with_clients_and_submit_approval_limits(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    let loss_policy = loss_governor_policy_from_loaded(loaded)?;
    let loss_halt_action_policy = loss_governor_halt_action_policy_from_loaded(loaded)?;
    let position_sizer = position_sizer_config_from_loaded(loaded)?;
    let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> = if loaded.strategies.is_empty() {
        if loss_policy.is_none() && position_sizer.is_none() {
            Arc::new(NoStrategyDecisionEvidenceWriter)
        } else {
            Arc::new(
                JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded).map_err(|error| {
                    BoltV3LiveNodeError::StrategyRegistration(
                        BoltV3StrategyRegistrationError::Evidence {
                            message: error.to_string(),
                        },
                    )
                })?,
            )
        }
    } else {
        Arc::new(
            JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded).map_err(|error| {
                BoltV3LiveNodeError::StrategyRegistration(
                    BoltV3StrategyRegistrationError::Evidence {
                        message: error.to_string(),
                    },
                )
            })?,
        )
    };
    let startup_observed_at_ns = current_unix_nanos().map_err(BoltV3LiveNodeError::Build)?;
    let position_sizer_runtime_feed_config =
        position_sizer_runtime_feed_config_from_loaded(loaded, startup_observed_at_ns);
    let position_sizer_venue_spendability_source =
        position_sizer_venue_spendability_source_config_from_loaded(loaded)?;
    let submit_reservation_recovery = if position_sizer_runtime_feed_config.is_some() {
        submit_reservation_recovery_config_from_loaded(loaded)?
    } else {
        None
    };
    let submit_admission = Arc::new(
        BoltV3SubmitAdmissionState::new_with_live_submit_limits_and_optional_controls(
            decision_evidence.clone(),
            live_submit_approval_limits,
            loss_policy.clone(),
            position_sizer,
        ),
    );
    let (position_sizer_runtime_feed, position_sizer_runtime_feed_subscription) =
        match position_sizer_runtime_feed_config {
            Some(config) => {
                let feed = Arc::new(Mutex::new(PositionSizerRuntimeFeed::new(
                    config,
                    submit_admission.clone(),
                )));
                let subscription = subscribe_position_sizer_runtime_feed(feed.clone());
                (Some(feed), Some(subscription))
            }
            None => (None, None),
        };
    let builder =
        make_bolt_v3_live_node_builder(loaded).map_err(BoltV3LiveNodeError::BuilderConstruction)?;
    let (builder, summary) = register_bolt_v3_clients(builder, adapters)
        .map_err(BoltV3LiveNodeError::ClientRegistration)?;
    let mut node = builder.build().map_err(BoltV3LiveNodeError::Build)?;
    let strategy_summary = register_bolt_v3_strategies_on_node_with_bindings(
        &mut node,
        loaded,
        resolved,
        crate::bolt_v3_archetypes::runtime_bindings(),
        submit_admission.clone(),
        decision_evidence.clone(),
    )
    .map_err(BoltV3LiveNodeError::StrategyRegistration)?;
    for strategy in &strategy_summary.registered {
        log::info!(
            "bolt-v3 registered strategy: strategy_instance_id={} strategy_archetype={} nt_strategy_id={}",
            strategy.strategy_instance_id,
            strategy.strategy_archetype.as_str(),
            strategy.registered_strategy_id
        );
    }
    let loss_halt_action_handler =
        match (loss_policy.clone(), loss_halt_action_policy.as_ref()) {
            (Some(policy), Some(action_policy)) => Some(
                loss_governor_halt_action_handler_from_node(&node, policy, *action_policy),
            ),
            _ => None,
        };
    let (loss_runtime_feed, loss_runtime_feed_subscription) =
        match loss_governor_runtime_feed_config_from_loaded(loaded) {
            Some(config) => {
                let feed = LossGovernorRuntimeFeed::new(config, submit_admission.clone());
                let feed = match loss_halt_action_handler.as_ref() {
                    Some(handler) => feed.with_halt_action_handler(handler.clone()),
                    None => feed,
                };
                let feed = Rc::new(RefCell::new(feed));
                let subscription = subscribe_loss_governor_runtime_feed(feed.clone());
                (Some(feed), Some(subscription))
            }
            None => (None, None),
        };
    let runtime = BoltV3LiveNodeRuntime::new(
        node,
        summary.clone(),
        submit_admission,
        BoltV3LiveNodeRuntimeFeeds {
            loss_halt_action_policy,
            loss_runtime_feed,
            loss_runtime_feed_subscription,
            position_sizer_runtime_feed,
            position_sizer_runtime_feed_subscription,
            position_sizer_venue_spendability_source,
            submit_reservation_recovery,
        },
        resolved.redaction_values(),
    );
    runtime.refresh_position_sizer_venue_spendability_from_configured_source()?;
    Ok((runtime, summary))
}

fn loss_governor_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Option<LossGovernorRuntimeFeedConfig> {
    let block = loaded.root.risk.loss_governor.as_ref()?;
    block.enabled.then_some(LossGovernorRuntimeFeedConfig {
        account_id: block.account_id,
        rolling_window_ns: block.rolling_window_ns,
    })
}

fn position_sizer_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
    startup_observed_at_ns: u64,
) -> Option<PositionSizerRuntimeFeedConfig> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    let product = pool.prediction_market_binary.as_ref()?;
    Some(PositionSizerRuntimeFeedConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id,
        collateral_currency: pool.collateral_currency.clone(),
        product_state: ProductSizingSnapshot::PredictionMarketBinary(
            PredictionMarketSizingSnapshot {
                source: "bolt_configured_binary_product".to_string(),
                observed_at_ns: startup_observed_at_ns,
                yes_instrument_id: product.yes_instrument_id.to_string(),
                no_instrument_id: product.no_instrument_id.to_string(),
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::ZERO,
                conditional_token_allowance: Decimal::ZERO,
                collateral_coupled_group_id: product.collateral_coupled_group_id.clone(),
            },
        ),
        startup_observed_at_ns,
    })
}

fn position_sizer_venue_spendability_source_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3PositionSizerVenueSpendabilitySourceConfig>, BoltV3LiveNodeError> {
    let Some(pool) = loaded
        .root
        .risk
        .capital_pools
        .as_ref()
        .and_then(|pools| pools.iter().find(|pool| pool.enforce_submit_admission))
    else {
        return Ok(None);
    };
    let has_source_binding = pool.venue_spendability_source_path.is_some()
        || pool.venue_spendability_source_sha256.is_some()
        || pool.venue_spendability_source_max_bytes.is_some();
    if !has_source_binding {
        return Ok(None);
    }
    let (Some(path_value), Some(expected_sha256), Some(max_bytes)) = (
        pool.venue_spendability_source_path.as_ref(),
        pool.venue_spendability_source_sha256.as_ref(),
        pool.venue_spendability_source_max_bytes,
    ) else {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "risk.capital_pools venue_spendability_source path, sha256, and max_bytes must be configured together"
        )));
    };
    Ok(Some(BoltV3PositionSizerVenueSpendabilitySourceConfig {
        path: resolve_root_relative_path(&loaded.root_path, path_value),
        max_bytes,
        expected_sha256: expected_sha256.clone(),
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id.to_string(),
        collateral_currency: pool.collateral_currency.clone(),
    }))
}

/// Resolve the startup reservation-recovery source from the loaded config.
/// The recovery driver reads the decision-evidence file, so the path comes
/// from [`decision_evidence_path`] and the read bound from
/// `persistence.decision_evidence.recovery_evidence_max_bytes`. Returns
/// `None` (recovery disabled) when the byte cap is not configured.
fn submit_reservation_recovery_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitReservationRecoveryConfig>, BoltV3LiveNodeError> {
    let Some(max_bytes) = loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes
    else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitReservationRecoveryConfig {
        path: decision_evidence_path(loaded).map_err(BoltV3LiveNodeError::Build)?,
        max_bytes,
    }))
}

fn position_sizer_venue_spendability_snapshot_from_source_config(
    config: &BoltV3PositionSizerVenueSpendabilitySourceConfig,
) -> Result<VenueSpendabilitySnapshot, BoltV3LiveNodeError> {
    venue_spendability_snapshot_from_json_file(VenueSpendabilitySourceFileRequest {
        path: &config.path,
        max_bytes: config.max_bytes,
        expected_sha256: &config.expected_sha256,
        identity: VenueSpendabilityIdentity {
            venue_id: &config.venue_id,
            account_id: &config.account_id,
            collateral_currency: &config.collateral_currency,
        },
    })
    .map_err(|error| {
        BoltV3LiveNodeError::Build(anyhow::anyhow!(
            "position sizer venue spendability source rejected: {error:?}"
        ))
    })
}

fn refresh_position_sizer_venue_spendability_from_source(
    feed: &Arc<Mutex<PositionSizerRuntimeFeed>>,
    config: &BoltV3PositionSizerVenueSpendabilitySourceConfig,
) -> Result<Option<BoltV3SubmitPositionSizingNtComponents>, BoltV3LiveNodeError> {
    let snapshot = position_sizer_venue_spendability_snapshot_from_source_config(config)?;
    let mut feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(feed.on_venue_spendability_snapshot(snapshot))
}

fn position_sizer_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitPositionSizerConfig>, BoltV3LiveNodeError> {
    let Some(pools) = loaded.root.risk.capital_pools.as_ref() else {
        return Ok(None);
    };
    let Some(pool) = pools.iter().find(|pool| pool.enforce_submit_admission) else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitPositionSizerConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id.to_string(),
        product_kind: ProductKind::PredictionMarketBinary,
        collateral_currency: pool.collateral_currency.clone(),
        capital_pool: CapitalPoolSnapshot {
            source: pool.pool_id.clone(),
            observed_at_ns: 0,
            pool_id: pool.pool_id.clone(),
            max_pool_liability: required_pool_decimal(
                "risk.capital_pools.max_pool_liability",
                &pool.max_pool_liability,
            )?,
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: pool.max_snapshot_age_ns,
        },
        policy: sizing_policy_from_pool(pool)?,
    }))
}

fn sizing_policy_from_pool(pool: &CapitalPoolBlock) -> Result<SizingPolicy, BoltV3LiveNodeError> {
    let sizing = &pool.sizing_policy;
    Ok(SizingPolicy {
        min_remaining_pool_balance: optional_pool_decimal(
            "risk.capital_pools.sizing_policy.min_remaining_pool_balance",
            sizing.min_remaining_pool_balance.as_deref(),
        )?,
        fee_slippage_policy: Some(FeeSlippagePolicy {
            max_fee_liability: required_pool_decimal(
                "risk.capital_pools.sizing_policy.fee_slippage.max_fee_liability",
                &sizing.fee_slippage.max_fee_liability,
            )?,
            max_slippage_liability: required_pool_decimal(
                "risk.capital_pools.sizing_policy.fee_slippage.max_slippage_liability",
                &sizing.fee_slippage.max_slippage_liability,
            )?,
        }),
    })
}

fn required_pool_decimal(label: &str, value: &str) -> Result<Decimal, BoltV3LiveNodeError> {
    parse_decimal_string(value).map_err(|message| {
        BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be a decimal string: {message}"
        ))
    })
}

fn optional_pool_decimal(
    label: &str,
    value: Option<&str>,
) -> Result<Option<Decimal>, BoltV3LiveNodeError> {
    value
        .map(|value| required_pool_decimal(label, value))
        .transpose()
}

fn loss_governor_policy_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorPolicy>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorPolicy {
        max_snapshot_age_ns: block.max_snapshot_age_ns,
        max_per_trade_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_per_trade_loss",
            block.max_per_trade_loss.as_deref(),
        )?),
        max_daily_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_daily_loss",
            block.max_daily_loss.as_deref(),
        )?),
        max_rolling_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_rolling_loss",
            block.max_rolling_loss.as_deref(),
        )?),
        max_drawdown: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_drawdown",
            block.max_drawdown.as_deref(),
        )?),
    }))
}

fn loss_governor_halt_action_policy_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorHaltActionPolicy>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorHaltActionPolicy {
        on_loss_breach_trading_state: required_loss_governor_trading_state_action(
            "risk.loss_governor.on_loss_breach_trading_state",
            block.on_loss_breach_trading_state,
        )?,
        on_untrusted_snapshot_trading_state: required_loss_governor_trading_state_action(
            "risk.loss_governor.on_untrusted_snapshot_trading_state",
            block.on_untrusted_snapshot_trading_state,
        )?,
        recovery_mode: required_loss_governor_recovery_mode(
            "risk.loss_governor.recovery_mode",
            block.recovery_mode,
        )?,
        manual_recovery_evidence_max_path_bytes: required_loss_governor_usize(
            "risk.loss_governor.manual_recovery_evidence_max_path_bytes",
            block.manual_recovery_evidence_max_path_bytes,
        )?,
    }))
}

fn required_loss_governor_trading_state_action(
    label: &'static str,
    value: Option<LossGovernorTradingStateAction>,
) -> Result<LossGovernorTradingStateAction, BoltV3LiveNodeError> {
    value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))
}

fn required_loss_governor_recovery_mode(
    label: &'static str,
    value: Option<LossGovernorRecoveryMode>,
) -> Result<LossGovernorRecoveryMode, BoltV3LiveNodeError> {
    value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))
}

fn required_loss_governor_usize(
    label: &'static str,
    value: Option<usize>,
) -> Result<usize, BoltV3LiveNodeError> {
    let value =
        value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))?;
    if value == usize::MIN {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be positive"
        )));
    }
    Ok(value)
}

fn loss_governor_halt_action_handler_from_node(
    node: &LiveNode,
    loss_policy: LossGovernorPolicy,
    action_policy: LossGovernorHaltActionPolicy,
) -> LossGovernorHaltActionHandler {
    let risk_engine = node.kernel().risk_engine().clone();
    Rc::new(move |snapshot, now_ns| {
        let decision = evaluate_loss_admission(&loss_policy, snapshot, now_ns);
        let current_state = risk_engine.borrow().trading_state();
        if current_state == TradingState::Active && decision.accepted {
            return;
        }

        if let Some(target_state) =
            next_loss_governor_trading_state(&action_policy, current_state, &decision)
        {
            risk_engine.borrow_mut().set_trading_state(target_state);
        }
    })
}

fn required_loss_governor_decimal(
    label: &'static str,
    value: Option<&str>,
) -> Result<Decimal, BoltV3LiveNodeError> {
    let value =
        value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))?;
    let decimal = parse_decimal_string(value).map_err(|reason| {
        BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be a valid decimal string ({reason}): `{value}`"
        ))
    })?;
    if decimal <= Decimal::ZERO {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be positive: `{value}`"
        )));
    }
    Ok(decimal)
}

/// Translates a validated bolt-v3 config into an NT-native
/// [`LiveNodeBuilder`] with no clients added. Field translation goes
/// through [`make_live_node_config`] so the bolt-v3 → NT field mapping
/// has a single source of truth that the existing per-field tests can
/// keep exercising.
pub fn make_bolt_v3_live_node_builder(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    let cfg = make_live_node_config(loaded);
    make_bolt_v3_live_node_builder_from_config(cfg)
}

fn make_bolt_v3_live_node_builder_from_config(
    cfg: LiveNodeConfig,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    LiveNodeBuilder::from_config(cfg)
        .map_err(|source| BoltV3LiveNodeBuilderError::BuilderConstruction { source })
}

pub fn make_live_node_config(loaded: &LoadedBoltV3Config) -> LiveNodeConfig {
    let trader_id = loaded.root.trader_id;
    let environment = loaded.root.runtime.mode;
    let mut module_level: AHashMap<Ustr, LevelFilter> = AHashMap::new();
    for module_path in bolt_v3_providers::credential_log_modules() {
        module_level.insert(Ustr::from(module_path), LevelFilter::Warn);
    }
    let logging = LoggerConfig {
        stdout_level: nautilus_common::logging::map_log_level_to_filter(
            loaded.root.logging.stdout_level,
        ),
        fileout_level: nautilus_common::logging::map_log_level_to_filter(
            loaded.root.logging.fileout_level,
        ),
        component_level: AHashMap::new(),
        module_level,
        log_components_only: false,
        is_colored: true,
        print_config: false,
        use_tracing: false,
        bypass_logging: false,
        file_config: None,
        clear_log_file: false,
    };
    let nautilus = &loaded.root.nautilus;
    let data = &nautilus.data_engine;
    let data_engine = nautilus_live::config::LiveDataEngineConfig {
        time_bars_build_with_no_updates: data.time_bars_build_with_no_updates,
        time_bars_timestamp_on_close: data.time_bars_timestamp_on_close,
        time_bars_skip_first_non_full_bar: data.time_bars_skip_first_non_full_bar,
        time_bars_interval_type: bar_interval_type_from_str(&data.time_bars_interval_type),
        time_bars_build_delay: data.time_bars_build_delay,
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live data config consumes the same aggregation/nanosecond pairs as a HashMap.
        time_bars_origin_offset: data.time_bars_origins.clone().into_iter().collect(),
        validate_data_sequence: data.validate_data_sequence,
        buffer_deltas: data.buffer_deltas,
        emit_quotes_from_book: data.emit_quotes_from_book,
        emit_quotes_from_book_depths: data.emit_quotes_from_book_depths,
        external_clients: configured_external_clients(&data.external_clients),
        debug: data.debug,
        graceful_shutdown_on_error: data.graceful_shutdown_on_error,
        qsize: data.qsize,
    };
    let exec = &nautilus.exec_engine;
    let reconciliation_lookback_mins = u32_zero_as_none(exec.reconciliation_lookback_mins);
    let exec_engine = nautilus_live::config::LiveExecEngineConfig {
        load_cache: exec.load_cache,
        snapshot_orders: exec.snapshot_orders,
        snapshot_positions: exec.snapshot_positions,
        snapshot_positions_interval_secs: u64_zero_as_none_f64(
            exec.snapshot_positions_interval_secs,
        ),
        external_clients: configured_external_clients(&exec.external_clients),
        debug: exec.debug,
        reconciliation: exec.reconciliation,
        reconciliation_lookback_mins,
        // `f64` is lossless for all practical delay values (< 2^53 seconds).
        reconciliation_startup_delay_secs: exec.reconciliation_startup_delay_secs as f64,
        reconciliation_instrument_ids: non_empty_strings(&exec.reconciliation_instrument_ids),
        filter_unclaimed_external_orders: exec.filter_unclaimed_external_orders,
        filter_position_reports: exec.filter_position_reports,
        filtered_client_order_ids: non_empty_strings(&exec.filtered_client_order_ids),
        generate_missing_orders: exec.generate_missing_orders,
        inflight_check_interval_ms: exec.inflight_check_interval_ms,
        inflight_check_threshold_ms: exec.inflight_check_threshold_ms,
        inflight_check_retries: exec.inflight_check_retries,
        open_check_interval_secs: u64_zero_as_none_f64(exec.open_check_interval_secs),
        open_check_lookback_mins: u32_zero_as_none(exec.open_check_lookback_mins),
        open_check_threshold_ms: exec.open_check_threshold_ms,
        open_check_missing_retries: exec.open_check_missing_retries,
        open_check_open_only: exec.open_check_open_only,
        max_single_order_queries_per_cycle: exec.max_single_order_queries_per_cycle,
        single_order_query_delay_ms: exec.single_order_query_delay_ms,
        position_check_interval_secs: u64_zero_as_none_f64(exec.position_check_interval_secs),
        position_check_lookback_mins: exec.position_check_lookback_mins,
        position_check_threshold_ms: exec.position_check_threshold_ms,
        position_check_retries: exec.position_check_retries,
        purge_closed_orders_interval_mins: u32_zero_as_none(exec.purge_closed_orders_interval_mins),
        purge_closed_orders_buffer_mins: u32_zero_as_none(exec.purge_closed_orders_buffer_mins),
        purge_closed_positions_interval_mins: u32_zero_as_none(
            exec.purge_closed_positions_interval_mins,
        ),
        purge_closed_positions_buffer_mins: u32_zero_as_none(
            exec.purge_closed_positions_buffer_mins,
        ),
        purge_account_events_interval_mins: u32_zero_as_none(
            exec.purge_account_events_interval_mins,
        ),
        purge_account_events_lookback_mins: u32_zero_as_none(
            exec.purge_account_events_lookback_mins,
        ),
        purge_from_database: exec.purge_from_database,
        own_books_audit_interval_secs: u64_zero_as_none_f64(exec.own_books_audit_interval_secs),
        graceful_shutdown_on_error: exec.graceful_shutdown_on_error,
        qsize: exec.qsize,
        allow_overfills: exec.allow_overfills,
        manage_own_order_books: exec.manage_own_order_books,
    };
    let risk_engine = nautilus_live::config::LiveRiskEngineConfig {
        // Mandated safety invariant: the NT live risk engine must never be
        // bypassed. This is pinned in code with no config knob so no TOML edit
        // or operator override can disable pre-trade risk checks.
        bypass: false,
        max_order_submit_rate: loaded.root.risk.nautilus.max_order_submit_rate.clone(),
        max_order_modify_rate: loaded.root.risk.nautilus.max_order_modify_rate.clone(),
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live risk config consumes the same string pairs as a HashMap.
        max_notional_per_order: loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .clone()
            .into_iter()
            .collect(),
        debug: loaded.root.risk.nautilus.debug,
        graceful_shutdown_on_error: loaded.root.risk.nautilus.graceful_shutdown_on_error,
        qsize: loaded.root.risk.nautilus.qsize,
    };

    // Explicit struct literal: upstream NT `LiveNodeConfig` field additions must be
    // considered here instead of silently inherited through `Default`.
    LiveNodeConfig {
        environment,
        trader_id,
        load_state: nautilus.load_state,
        save_state: nautilus.save_state,
        logging,
        instance_id: None,
        timeout_connection: Duration::from_secs(nautilus.timeout_connection_secs),
        timeout_reconciliation: Duration::from_secs(nautilus.timeout_reconciliation_secs),
        timeout_portfolio: Duration::from_secs(nautilus.timeout_portfolio_secs),
        timeout_disconnection: Duration::from_secs(nautilus.timeout_disconnection_secs),
        delay_post_stop: Duration::from_secs(nautilus.delay_post_stop_secs),
        timeout_shutdown: Duration::from_secs(nautilus.timeout_shutdown_secs),
        cache: None,
        msgbus: None,
        portfolio: None,
        emulator: None,
        streaming: None,
        event_store: None,
        loop_debug: false,
        data_engine,
        risk_engine,
        exec_engine,
        data_clients: HashMap::new(),
        exec_clients: HashMap::new(),
        plugins: Vec::new(),
    }
}

fn u32_zero_as_none(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn u64_zero_as_none_f64(value: u64) -> Option<f64> {
    (value != 0).then_some(value as f64)
}

fn non_empty_strings(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

fn configured_external_clients(values: &[ClientId]) -> Option<Vec<ClientId>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// Caller must run root validation first so the string is a valid NT `BarIntervalType`.
fn bar_interval_type_from_str(value: &str) -> BarIntervalType {
    BarIntervalType::from_str(value).expect("root validation must accept data bar interval type")
}

pub fn wire_bolt_v3_runtime_capture(
    node: &LiveNode,
    stop_handle: LiveNodeHandle,
    loaded: &LoadedBoltV3Config,
) -> Result<NtRuntimeCaptureGuards> {
    wire_nt_runtime_capture(
        node,
        stop_handle,
        &loaded.root.persistence.catalog_directory,
        loaded.root.persistence.streaming.flush_interval_ms,
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_ms,
        None,
    )
}

/// Bolt-v3 controlled-connect boundary.
///
/// Drives the pinned NautilusTrader controlled-connect API
/// (`NautilusKernel::connect_data_clients` followed by
/// `NautilusKernel::connect_exec_clients`) on every NT data and
/// execution client that the bolt-v3 client-registration boundary added
/// to `node`, bounded by the bolt-v3
/// `nautilus.timeout_connection_secs` value from `loaded`.
///
/// This boundary is **opt-in**: `build_bolt_v3_live_node` and its
/// `_with` / `_with_summary` siblings deliberately do not invoke it.
/// A caller must explicitly call this function on a node previously
/// returned by one of those builders. In a bolt-v3-only process, NT's
/// first-wins logger is initialized by the bolt-v3 `LoggerConfig`
/// passed through `LiveNodeBuilder::build`, so the
/// provider-owned credential log module filters remain active during
/// connect.
/// The production bolt-v3 entrypoint preserves that ordering.
///
/// This boundary is **bounded**: the dispatched engine-level connect
/// futures are wrapped in `tokio::time::timeout` driven by
/// `nautilus.timeout_connection_secs`. If the bound elapses before
/// both engines finish dispatching connect to their registered clients
/// the function returns [`BoltV3LiveNodeError::ConnectTimeout`] and
/// the `LiveNode` is left in whatever partially-connected state NT
/// produced; the caller owns subsequent disconnect/teardown via
/// [`disconnect_bolt_v3_clients`].
///
/// This boundary is **dispatch + connected check**, not NT cache or
/// instrument readiness. The pinned NT `DataEngine::connect` and
/// `ExecutionEngine::connect` dispatchers swallow individual client
/// `connect()` errors and only log them, so after the dispatch
/// returns the bolt-v3 boundary consults
/// `NautilusKernel::check_engines_connected()` to ensure every
/// registered client transitioned to `is_connected`. If that check
/// returns false, the boundary returns
/// [`BoltV3LiveNodeError::ConnectIncomplete`] rather than `Ok(())`.
/// The boundary does **not** copy or reimplement NT private drain or
/// flush logic, and it does not gate on NT cache contents or
/// instrument-availability checks; that readiness is owned by a
/// future slice.
///
/// This boundary is **no-trade**: it never enters NT's runner loop
/// and never invokes NT's trader entrypoint, so no strategy actor is
/// activated, no reconciliation runs, and the runner loop is never
/// entered. `NodeState` therefore remains in whatever state the node
/// was in before the call (typically `Idle`). The boundary does not
/// register strategies, select markets, construct orders, submit
/// orders, or invoke any user-level subscription API.
///
/// Errors from individual NT client `connect()` calls are surfaced
/// via NT's logger (the engine-level dispatchers in
/// `nautilus_data::engine::DataEngine::connect` and
/// `nautilus_execution::engine::ExecutionEngine::connect` log
/// individual `Err` values rather than propagating them). The bolt-v3
/// boundary returns `Ok(())` only when both dispatchers have returned
/// within the configured bound **and**
/// `kernel.check_engines_connected()` returns true.
pub async fn connect_bolt_v3_clients(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_secs = loaded.root.nautilus.timeout_connection_secs;
    let bound = Duration::from_secs(timeout_secs);
    let connect = async {
        let kernel = node.kernel_mut();
        kernel.connect_data_clients().await;
        kernel.connect_exec_clients().await;
        kernel.check_engines_connected()
    };
    match tokio::time::timeout(bound, connect).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(BoltV3LiveNodeError::ConnectIncomplete),
        Err(_) => Err(BoltV3LiveNodeError::ConnectTimeout { timeout_secs }),
    }
}

/// Bolt-v3 controlled-disconnect boundary.
///
/// Drives the pinned NautilusTrader controlled-disconnect API
/// (`NautilusKernel::disconnect_clients`) on every NT data and
/// execution client previously added through the bolt-v3
/// client-registration boundary, bounded by the bolt-v3
/// `nautilus.timeout_disconnection_secs` value from `loaded`.
///
/// Recovery counterpart to [`connect_bolt_v3_clients`]: after a
/// `ConnectTimeout` or `ConnectIncomplete` the caller is expected to
/// invoke this function to drain whatever partially-connected NT
/// clients survive, again under a bounded timeout.
///
/// This boundary is **bounded**: NT's
/// `kernel.disconnect_clients()` future is wrapped in
/// `tokio::time::timeout`. On the bound elapsing, the function
/// returns [`BoltV3LiveNodeError::DisconnectTimeout`] with the
/// configured bound. On NT's engine-level disconnect aggregator
/// surfacing an `Err(..)`, the function returns
/// [`BoltV3LiveNodeError::DisconnectFailed`] wrapping the NT
/// `anyhow::Error`. Pinned NT disconnects data clients before
/// execution clients and can short-circuit on a data-client error; a
/// `DisconnectFailed` therefore leaves cleanup state indeterminate and
/// production recovery should rebuild a fresh `LiveNode`.
///
/// This boundary is **no-trade**: it never enters NT's runner loop,
/// never invokes NT's trader entrypoint, never registers strategies,
/// never selects markets, never constructs orders, never submits
/// orders, and never invokes any user-level subscription API. It
/// does not call `LiveNode::stop`; the bolt-v3 LiveNode remains
/// outside NT's runner-driven lifecycle. The boundary does **not**
/// copy or reimplement NT private drain or flush logic.
pub async fn disconnect_bolt_v3_clients(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_secs = loaded.root.nautilus.timeout_disconnection_secs;
    let bound = Duration::from_secs(timeout_secs);
    let disconnect = async { node.kernel_mut().disconnect_clients().await };
    match tokio::time::timeout(bound, disconnect).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(BoltV3LiveNodeError::DisconnectFailed(error)),
        Err(_) => Err(BoltV3LiveNodeError::DisconnectTimeout { timeout_secs }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_capital_reservation::ReservationRejectionReason;
    use crate::bolt_v3_config::{
        BoltV3RootConfig, DataClientReadinessProbeBlock, DataClientReadinessProbeMarketDataKind,
        DataClientReadinessProbeQuoteTargetBlock, DataClientReadinessProbeQuoteTargetSource,
        ReferenceDataBlock,
    };
    use crate::bolt_v3_loss_governor::LossSnapshot;
    use crate::bolt_v3_providers::hyperliquid::{
        ResolvedBoltV3HyperliquidSecrets, hyperliquid_live_submit_signer_fingerprint,
    };
    use crate::bolt_v3_providers::hyperliquid_artifacts::{
        HyperliquidLiveSubmitApprovalInput, HyperliquidLiveSubmitOrderLimits,
        HyperliquidProductSubmitProofBinding, write_hyperliquid_live_submit_approval_artifact,
    };
    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::data::{BookOrder, OrderBookDelta, OrderBookDeltas};
    use nautilus_model::enums::{
        AccountType, BookAction, CurrencyType, OrderSide, TimeInForce, TradingState,
    };
    use nautilus_model::events::{AccountState, OrderAccepted, OrderEventAny, OrderSubmitted};
    use nautilus_model::identifiers::{AccountId, ClientOrderId, TraderId, VenueOrderId};
    use nautilus_model::orders::{LimitOrder, MarketOrder, OrderAny};
    use nautilus_model::types::{AccountBalance, Currency, Money, Price, Quantity};
    use rust_decimal::Decimal;
    use sha2::{Digest, Sha256};

    #[test]
    fn startup_rebuild_recovers_known_submit_reservation_from_nt_cache() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let metadata = fixture_submit_reservation_metadata(
            "startup-known-client-order",
            "condition-fixture-yes.POLYMARKET",
            "buy",
            "10",
            "0.4",
            "0.3",
            "4.3",
        );
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        assert_eq!(runtime.position_sizer_reconciled(), Some(false));
        write_submit_reservation_metadata(&loaded, &metadata);
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
        seed_accepted_open_limit_order(
            &runtime,
            generic_limit_order(
                "startup-known-client-order",
                "condition-fixture-yes.POLYMARKET",
                OrderSide::Buy,
                Quantity::from(6),
                Price::from("0.40"),
            ),
            "POLYMARKET-001",
        );

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild,
            BoltV3SubmitPositionSizingRebuildDecision {
                accepted: true,
                reason: None,
                attempted_reservation_count: 1,
                rebuilt_reservation_count: 1,
                live_reserved_liability: Decimal::new(27, 1),
                missing_nt_account_cache_balance: None,
            }
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(true));
        assert_eq!(
            runtime
                .submit_admission
                .position_sizer_live_reserved_liability(),
            Some(Decimal::new(27, 1))
        );
        assert!(
            runtime
                .submit_admission
                .position_sizer_has_live_reservation("startup-known-client-order"),
            "startup rebuild must install the recovered reservation for later fill/cancel release"
        );
    }

    #[test]
    fn startup_rebuild_stays_closed_for_unknown_nt_cache_order() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
        seed_accepted_open_limit_order(
            &runtime,
            generic_limit_order(
                "startup-unknown-client-order",
                "condition-fixture-yes.POLYMARKET",
                OrderSide::Buy,
                Quantity::from(6),
                Price::from("0.40"),
            ),
            "POLYMARKET-001",
        );

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild,
            BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count: 1,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            }
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(false));
        assert!(
            !runtime
                .submit_admission
                .position_sizer_has_live_reservation("startup-unknown-client-order")
        );
    }

    #[test]
    fn startup_rebuild_reports_missing_nt_account_cache_balance() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild.missing_nt_account_cache_balance,
            Some(BoltV3SubmitPositionSizingMissingNtAccountCacheBalance {
                account_id: "POLYMARKET-001".to_string(),
                collateral_currency: "PUSD".to_string(),
            })
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(false));

        let feed = runtime
            .position_sizer_runtime_feed
            .as_ref()
            .expect("fixture should configure position-sizer runtime feed");
        let account_state = account_state_event("POLYMARKET-001", "PUSD", 100.0, 100.0, 2_100);
        assert!(
            feed.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .on_account_state(&account_state)
                .is_some(),
            "account state should still publish sizing components once collateral facts arrive"
        );
        assert_eq!(
            runtime.position_sizer_reconciled(),
            Some(false),
            "missing startup account cache means the pre-run empty order cache is not authoritative"
        );
        let state = runtime
            .submit_admission
            .position_sizer_state_snapshot()
            .expect("account state should publish an unreconciled sizing state");
        assert_eq!(state.order_lifecycle.source, "nt_order_lifecycle_seed");
        assert!(!state.order_lifecycle.all_open_orders_attributed);
    }

    #[test]
    fn live_node_build_does_not_apply_loss_halt_before_first_trusted_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        assert_eq!(runtime.nt_risk_trading_state(), TradingState::Active);
    }

    #[test]
    fn manual_recovery_evidence_clears_live_reducing_state_after_fresh_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");
        runtime
            .node
            .kernel()
            .risk_engine()
            .borrow_mut()
            .set_trading_state(TradingState::Reducing);
        runtime.submit_admission.update_loss_snapshot(LossSnapshot {
            source: "nt_loss_runtime_feed".to_string(),
            observed_at_ns: 2_000,
            per_trade_pnl: Some(Decimal::ZERO),
            daily_pnl: Some(Decimal::ZERO),
            rolling_pnl: Some(Decimal::ZERO),
            current_equity: Some(Decimal::new(100, 0)),
            peak_equity: Some(Decimal::new(100, 0)),
        });
        let evidence = LossGovernorManualRecoveryEvidence::new(
            "operator-primary",
            "loss-governor/manual-recovery.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            2_050,
            256,
        )
        .expect("bounded manual recovery evidence should validate");

        let target = runtime.apply_loss_governor_manual_recovery(&evidence, 2_100);

        assert_eq!(target, Some(TradingState::Active));
        assert_eq!(runtime.nt_risk_trading_state(), TradingState::Active);
    }

    #[test]
    fn startup_rebuild_seeds_nt_cached_free_collateral_when_balance_has_locked_amount() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        // Helper writes NT AccountBalance as (total, locked, free): total=100, locked=40, free=60.
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 60.0);
        {
            let account_id = AccountId::from("POLYMARKET-001");
            let cache = runtime.node.kernel().cache();
            let cache = cache.borrow();
            let account = cache
                .account_owned(&account_id)
                .expect("seeded account should be present in NT cache");
            let balances = account.balances();
            let balance = balances
                .values()
                .find(|balance| balance.currency.code.as_str() == "PUSD")
                .expect("seeded collateral balance should be present in NT cache");
            assert_eq!(balance.total.as_decimal(), Decimal::new(100, 0));
            assert_eq!(balance.locked.as_decimal(), Decimal::new(40, 0));
            assert_eq!(balance.free.as_decimal(), Decimal::new(60, 0));
        }

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(rebuild.missing_nt_account_cache_balance, None);
        assert_eq!(runtime.position_sizer_reconciled(), Some(true));
        let state = runtime
            .submit_admission
            .position_sizer_state_snapshot()
            .expect("startup rebuild should seed position sizing state");
        assert_eq!(state.portfolio.free_collateral, Decimal::new(60, 0));
        assert_eq!(state.portfolio.total_equity, Decimal::new(100, 0));
        assert_ne!(
            state.portfolio.free_collateral, state.portfolio.total_equity,
            "fixture must prove locked collateral is not treated as spendable"
        );
        match state.product_state {
            ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
                assert_eq!(snapshot.collateral_allowance, Decimal::new(60, 0));
            }
        }
    }

    #[test]
    fn startup_rebuild_rejects_known_metadata_when_open_quantity_exceeds_submitted() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let metadata = fixture_submit_reservation_metadata(
            "startup-overopen-client-order",
            "condition-fixture-yes.POLYMARKET",
            "buy",
            "10",
            "0.4",
            "0.3",
            "4.3",
        );
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        write_submit_reservation_metadata(&loaded, &metadata);
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
        seed_accepted_open_limit_order(
            &runtime,
            generic_limit_order(
                "startup-overopen-client-order",
                "condition-fixture-yes.POLYMARKET",
                OrderSide::Buy,
                Quantity::from(11),
                Price::from("0.40"),
            ),
            "POLYMARKET-001",
        );

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild,
            BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count: 1,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            }
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(false));
        assert!(
            !runtime
                .submit_admission
                .position_sizer_has_live_reservation("startup-overopen-client-order")
        );
    }

    #[test]
    fn nt_limit_order_snapshot_maps_to_generic_open_order_evidence() {
        let order = generic_limit_order(
            "client-order-1",
            "instrument-yes.VENUE-A",
            OrderSide::Buy,
            Quantity::from(10),
            Price::from("0.40"),
        );

        let evidence = nt_open_order_evidence_from_order(&order, 1_000)
            .expect("bounded NT limit order should produce generic open-order evidence");

        assert_eq!(evidence.client_order_id, "client-order-1");
        assert_eq!(evidence.instrument_id, "instrument-yes.VENUE-A");
        assert_eq!(evidence.side, BoltV3CompiledOrderSide::Buy);
        assert_eq!(evidence.open_quantity, Decimal::new(10, 0));
        assert_eq!(evidence.limit_price, Decimal::new(4, 1));
        assert_eq!(evidence.observed_at_ns, 1_000);
        assert_eq!(evidence.evidence_label, "nt_open_order_cache");
    }

    #[test]
    fn nt_non_limit_order_snapshot_is_not_sizing_evidence() {
        let order = generic_market_order(
            "client-order-1",
            "instrument-yes.VENUE-A",
            OrderSide::Buy,
            Quantity::from(10),
        );

        assert!(nt_open_order_evidence_from_order(&order, 1_000).is_none());
    }

    fn loaded_config_with_submit_sizer_recovery(temp_path: &std::path::Path) -> LoadedBoltV3Config {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.strategies.clear();
        loaded
            .root
            .risk
            .capital_pools
            .as_mut()
            .expect("fixture should configure capital pools")[0]
            .enforce_submit_admission = true;
        loaded.root.persistence.catalog_directory = temp_path.to_string_lossy().to_string();
        loaded
            .root
            .persistence
            .decision_evidence
            .recovery_evidence_max_bytes = Some(100_000);
        loaded
    }

    fn fixture_submit_reservation_metadata(
        client_order_id: &str,
        instrument_id: &str,
        side: &str,
        submitted_quantity: &str,
        liability_factor: &str,
        additive_liability: &str,
        reserved_liability: &str,
    ) -> BoltV3SubmitReservationMetadataEvidence {
        BoltV3SubmitReservationMetadataEvidence {
            client_order_id: client_order_id.to_string(),
            submit_reservation_id: format!("{client_order_id}#submit"),
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "PUSD".to_string(),
            capital_pool_id: "polymarket-prediction-live".to_string(),
            collateral_group_id: "condition-fixture".to_string(),
            instrument_id: instrument_id.to_string(),
            side: side.to_string(),
            submitted_quantity: submitted_quantity.to_string(),
            liability_factor: liability_factor.to_string(),
            additive_liability: additive_liability.to_string(),
            reserved_liability: reserved_liability.to_string(),
            observed_at_ns: 1_000,
            source: "submit_admission".to_string(),
        }
    }

    fn write_submit_reservation_metadata(
        loaded: &LoadedBoltV3Config,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) {
        let writer = JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded)
            .expect("decision evidence writer should open");
        writer
            .record_submit_reservation_metadata(metadata)
            .expect("submit reservation metadata should write");
    }

    fn fake_bolt_v3_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
        match path {
            "/bolt/polymarket_main/private_key" => Ok(
                "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
            ),
            "/bolt/polymarket_main/api_key" => Ok("polymarket-api-key".to_string()),
            "/bolt/polymarket_main/api_secret" => Ok("YWJj".to_string()),
            "/bolt/polymarket_main/passphrase" => Ok("polymarket-passphrase".to_string()),
            "/bolt/binance_reference/api_key" => Ok("binance-api-key".to_string()),
            "/bolt/binance_reference/api_secret" => {
                Ok("MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".to_string())
            }
            _ => Err("unexpected SSM path requested by bolt-v3 fake resolver"),
        }
    }

    fn seed_cached_account_state(
        runtime: &BoltV3LiveNodeRuntime,
        account_id: &str,
        currency_code: &str,
        total: f64,
        free: f64,
    ) {
        let account_state = account_state_event(account_id, currency_code, total, free, 1);
        runtime
            .node
            .kernel()
            .cache()
            .borrow_mut()
            .update_account_state(&account_state)
            .expect("NT cache should apply account state");
    }

    fn account_state_event(
        account_id: &str,
        currency_code: &str,
        total: f64,
        free: f64,
        timestamp_ns: u64,
    ) -> AccountState {
        let currency = test_currency(currency_code);
        AccountState::new(
            AccountId::from(account_id),
            AccountType::Cash,
            vec![AccountBalance::new(
                Money::new(total, currency),
                Money::new(total - free, currency),
                Money::new(free, currency),
            )],
            vec![],
            true,
            UUID4::default(),
            UnixNanos::from(timestamp_ns),
            UnixNanos::from(timestamp_ns),
            Some(currency),
        )
    }

    fn test_currency(currency_code: &str) -> Currency {
        if currency_code == "PUSD" {
            return Currency::new("PUSD", 2, 0, "Test PUSD", CurrencyType::Fiat);
        }
        Currency::from(currency_code)
    }

    fn seed_accepted_open_limit_order(
        runtime: &BoltV3LiveNodeRuntime,
        order: OrderAny,
        account_id: &str,
    ) {
        let cache = runtime.node.kernel().cache();
        let mut cache = cache.borrow_mut();
        cache
            .add_order(
                order.clone(),
                None,
                Some(ClientId::from("polymarket_main")),
                false,
            )
            .expect("NT cache should accept initialized order");
        cache
            .update_order(&OrderEventAny::Submitted(OrderSubmitted::new(
                order.trader_id(),
                order.strategy_id(),
                order.instrument_id(),
                order.client_order_id(),
                AccountId::from(account_id),
                UUID4::default(),
                UnixNanos::from(1),
                UnixNanos::from(1),
            )))
            .expect("NT cache should apply submitted event");
        cache
            .update_order(&OrderEventAny::Accepted(OrderAccepted::new(
                order.trader_id(),
                order.strategy_id(),
                order.instrument_id(),
                order.client_order_id(),
                VenueOrderId::from("venue-order-startup"),
                AccountId::from(account_id),
                UUID4::default(),
                UnixNanos::from(2),
                UnixNanos::from(2),
                false,
            )))
            .expect("NT cache should apply accepted event");
    }

    fn generic_limit_order(
        client_order_id: &str,
        instrument_id: &str,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
    ) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from(instrument_id),
                ClientOrderId::from(client_order_id),
                order_side,
                quantity,
                price,
                TimeInForce::Gtc,
                None,
                false,
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                UUID4::default(),
                UnixNanos::from(1),
            )
            .expect("generic limit order should be valid"),
        )
    }

    fn generic_market_order(
        client_order_id: &str,
        instrument_id: &str,
        order_side: OrderSide,
        quantity: Quantity,
    ) -> OrderAny {
        OrderAny::Market(
            MarketOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from(instrument_id),
                ClientOrderId::from(client_order_id),
                order_side,
                quantity,
                TimeInForce::Ioc,
                UUID4::default(),
                UnixNanos::from(1),
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("generic market order should be valid"),
        )
    }

    #[test]
    fn live_node_adapter_mapping_consumes_hyperliquid_live_submit_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_approval_artifact_path = "{}"
live_submit_approval_artifact_max_bytes = 16384
live_submit_max_order_count = 2
live_submit_max_order_notional = "25.00"
live_submit_product_proof_artifact_path = "{}"
live_submit_product_proof_artifact_sha256 = "{}"
live_submit_product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let bundle = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect("production live-node mapping should consume approval and map execution");

        assert!(
            bundle
                .configs
                .clients
                .get("hyperliquid_perps")
                .and_then(|client| client.execution.as_ref())
                .is_some(),
            "consumed approval should reach the execution adapter mapper"
        );
        let approval_limits = bundle
            .live_submit_approval_limits
            .get("hyperliquid_perps")
            .expect("consumed Hyperliquid approval should carry submit-admission limits");
        assert_eq!(approval_limits.max_order_count, 2);
        assert_eq!(
            approval_limits.max_order_notional,
            Decimal::from_str_exact("25.00").expect("expected decimal should parse")
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("consumed approval should still read"),
        )
        .expect("consumed approval JSON should parse");
        assert_eq!(persisted["used_at"], now);

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now + 1,
            &build_head_sha,
        )
        .expect_err("persisted consumption must prevent approval reuse");
        assert!(
            error.to_string().contains("used_at"),
            "reuse failure should identify the spent approval field: {error}"
        );
    }

    fn hyperliquid_test_product_submit_proof_bytes(order_proof_path: String) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.hyperliquid_product_submit_proof.v1",
            "provider_key": "HYPERLIQUID",
            "provider_id": "hyperliquid_perps",
            "product_surface": "standard_perps",
            "toml_checksum": "b".repeat(64),
            "order_proof": {
                "artifact_path": order_proof_path,
                "artifact_sha256": "e".repeat(64),
            },
            "fill_proof": {
                "artifact_path": "operator/hyperliquid-fill-proof.json",
                "artifact_sha256": "f".repeat(64),
            },
            "rounding_proof": {
                "artifact_path": "operator/hyperliquid-rounding-proof.json",
                "artifact_sha256": "a".repeat(64),
            },
            "fee_proof": {
                "artifact_path": "operator/hyperliquid-fee-proof.json",
                "artifact_sha256": "c".repeat(64),
            },
            "settlement_proof": null,
        }))
        .expect("test product proof JSON should encode")
    }

    fn write_hyperliquid_test_product_submit_proof(path: &std::path::Path) -> String {
        let bytes = hyperliquid_test_product_submit_proof_bytes(
            "operator/hyperliquid-order-proof.json".to_string(),
        );
        std::fs::write(path, &bytes).expect("product proof should write");
        hex::encode(Sha256::digest(&bytes))
    }

    fn write_hyperliquid_semantically_invalid_product_submit_proof(
        path: &std::path::Path,
    ) -> String {
        let bytes = br#"{"provider":"HYPERLIQUID","surface":"standard_perps"}"#;
        std::fs::write(path, bytes).expect("invalid product proof should write");
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn live_node_invalid_product_submit_proof_schema_does_not_spend_hyperliquid_approval_artifact()
    {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 =
            write_hyperliquid_semantically_invalid_product_submit_proof(&product_proof_path);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_approval_artifact_path = "{}"
live_submit_approval_artifact_max_bytes = 16384
live_submit_max_order_count = 2
live_submit_max_order_notional = "25.00"
live_submit_product_proof_artifact_path = "{}"
live_submit_product_proof_artifact_sha256 = "{}"
live_submit_product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("matching hash alone must not authorize live-submit approval consumption");

        assert!(
            error.to_string().contains("product_submit_proof"),
            "failure should identify the product proof schema: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "invalid product proof semantics must not spend one-time approval artifacts"
        );
    }

    fn write_hyperliquid_test_product_submit_proof_with_padding(
        path: &std::path::Path,
        padding_len: usize,
    ) -> String {
        let bytes = hyperliquid_test_product_submit_proof_bytes(format!(
            "operator/{}-hyperliquid-order-proof.json",
            "x".repeat(padding_len)
        ));
        std::fs::write(path, &bytes).expect("padded product proof should write");
        hex::encode(Sha256::digest(&bytes))
    }

    #[test]
    fn live_node_product_submit_proof_uses_independent_byte_cap() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 =
            write_hyperliquid_test_product_submit_proof_with_padding(&product_proof_path, 6000);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_approval_artifact_path = "{}"
live_submit_approval_artifact_max_bytes = 4096
live_submit_max_order_count = 2
live_submit_max_order_notional = "25.00"
live_submit_product_proof_artifact_path = "{}"
live_submit_product_proof_artifact_sha256 = "{}"
live_submit_product_proof_artifact_max_bytes = 8192
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect("product proof should use its own byte cap before approval consumption");

        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("consumed approval should still read"),
        )
        .expect("consumed approval JSON should parse");
        assert_eq!(persisted["used_at"], now);
    }

    #[test]
    fn live_node_static_target_surface_mismatch_does_not_spend_hyperliquid_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.strategies.truncate(1);
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        strategy.config.execution_client_id = ClientId::from("hyperliquid_perps");
        strategy.config.target = toml::toml! {
            configured_target_id = "hl-spot-btc-usdc"
            kind = "static_instrument"
            rotating_market_family = "hyperliquid_instrument"
            product_surface = "spot"
            instrument_id = "BTC/USDC.HYPERLIQUID"
            quantity_step = "0.001"
        }
        .into();
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_approval_artifact_path = "{}"
live_submit_approval_artifact_max_bytes = 16384
live_submit_max_order_count = 2
live_submit_max_order_notional = "25.00"
live_submit_product_proof_artifact_path = "operator/hyperliquid-product-submit-proof.json"
live_submit_product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit_product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display()
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: "operator/hyperliquid-product-submit-proof.json".to_string(),
                    artifact_sha256: "d".repeat(64),
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("static target surface mismatch must fail before approval consumption");

        assert!(
            error
                .to_string()
                .contains("strategy.target.product_surface"),
            "failure should identify the target surface mismatch: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "surface mismatches must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn live_node_missing_product_submit_proof_does_not_spend_hyperliquid_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let missing_product_proof_path = temp.path().join("missing-product-submit-proof.json");
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_approval_artifact_path = "{}"
live_submit_approval_artifact_max_bytes = 16384
live_submit_max_order_count = 2
live_submit_max_order_notional = "25.00"
live_submit_product_proof_artifact_path = "{}"
live_submit_product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit_product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                missing_product_proof_path.display()
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: missing_product_proof_path.display().to_string(),
                    artifact_sha256: "d".repeat(64),
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("missing product submit proof must fail before approval consumption");

        assert!(
            error.to_string().contains("product_submit_proof"),
            "failure should identify the missing product proof binding: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "missing product proof must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn live_node_mismatched_product_submit_proof_does_not_spend_hyperliquid_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let _actual_product_proof_sha256 =
            write_hyperliquid_test_product_submit_proof(&product_proof_path);
        let mismatched_product_proof_sha256 = "d".repeat(64);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_approval_artifact_path = "{}"
live_submit_approval_artifact_max_bytes = 16384
live_submit_max_order_count = 2
live_submit_max_order_notional = "25.00"
live_submit_product_proof_artifact_path = "{}"
live_submit_product_proof_artifact_sha256 = "{}"
live_submit_product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                mismatched_product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: mismatched_product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("mismatched product submit proof must fail before approval consumption");

        assert!(
            error
                .to_string()
                .contains("product_submit_proof.artifact_sha256"),
            "failure should identify the product proof checksum: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "mismatched product proof must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn chunk_universe_splits_into_consecutive_chunks_of_at_most_n() {
        let universe: Vec<u32> = (0..10).collect();
        assert_eq!(
            chunk_universe(&universe, 3),
            vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8], vec![9]],
            "chunks must be consecutive, in order, and at most chunk_size"
        );
    }

    #[test]
    fn chunk_universe_returns_single_chunk_when_universe_fits() {
        assert_eq!(chunk_universe(&["a", "b"], 5), vec![vec!["a", "b"]]);
    }

    #[test]
    fn chunk_universe_is_empty_for_empty_universe_or_zero_chunk_size() {
        assert!(chunk_universe::<u32>(&[], 4).is_empty());
        assert!(
            chunk_universe(&[1, 2, 3], 0).is_empty(),
            "chunk_size 0 must yield no chunks so the probe fails closed rather than panicking"
        );
    }

    #[test]
    fn trade_chunk_count_probe_passes_only_at_or_above_m_with_positive_m() {
        assert!(
            !trade_chunk_count_probe_passed(0, 0),
            "m=0 must fail closed: requiring nothing proves nothing"
        );
        assert!(
            !trade_chunk_count_probe_passed(5, 0),
            "m=0 must fail closed even with fires"
        );
        assert!(!trade_chunk_count_probe_passed(9, 10), "below m must fail");
        assert!(
            trade_chunk_count_probe_passed(10, 10),
            "exactly m must pass"
        );
        assert!(trade_chunk_count_probe_passed(11, 10), "above m must pass");
    }

    #[test]
    fn chunk_count_handle_chunks_universe_and_walks_in_sorted_order() {
        let handle =
            BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
                ClientId::from("okx_data"),
                2,
                45,
                3,
                DataClientReadinessProbeMarketDataKind::Trade,
            );
        assert!(handle.is_chunk_count_mode());
        assert!(!handle.chunk_walk_started());

        handle.chunk_count_capture_universe(vec![
            InstrumentId::from("C-3.OKX"),
            InstrumentId::from("A-1.OKX"),
            InstrumentId::from("B-2.OKX"),
        ]);
        assert!(handle.chunk_walk_started());
        // 3 instruments at chunk_size 2 => 2 chunks; window threads through.
        assert_eq!(handle.chunk_walk_dims(), (2, 45));

        let first: Vec<String> = handle
            .chunk_count_next_chunk()
            .expect("first chunk")
            .iter()
            .map(|subscription| subscription.instrument_id.to_string())
            .collect();
        assert_eq!(
            first,
            vec!["A-1.OKX".to_string(), "B-2.OKX".to_string()],
            "the universe is walked in deterministic sorted order"
        );
        assert_eq!(
            handle.chunk_count_current_chunk().len(),
            2,
            "the current chunk tracks what is subscribed, for unsubscribe on advance"
        );

        assert_eq!(
            handle.chunk_count_next_chunk().expect("second chunk").len(),
            1,
            "the trailing chunk holds the remainder"
        );
        assert!(
            handle.chunk_count_next_chunk().is_none(),
            "the walk is exhausted after the last chunk"
        );
        assert!(
            !handle.chunk_count_passed(),
            "with no trades recorded the pass rule fails closed"
        );
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root_text = include_str!("../tests/fixtures/bolt_v3/root.toml");
        let root: BoltV3RootConfig = toml::from_str(root_text).unwrap();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: String::new(),
            root,
            strategies: Vec::new(),
        }
    }

    fn write_venue_spendability_source(
        loaded: &mut LoadedBoltV3Config,
        temp_path: &std::path::Path,
        observed_at_ns: u64,
        spendable_collateral: &str,
        collateral_allowance: &str,
    ) {
        let path = temp_path.join("venue-spendability-source.json");
        let pool = loaded
            .root
            .risk
            .capital_pools
            .as_mut()
            .and_then(|pools| pools.first_mut())
            .expect("fixture should configure a capital pool");
        pool.enforce_submit_admission = true;
        let payload = format!(
            r#"{{
  "schema_version": {schema_version},
  "record_kind": "{record_kind}",
  "source": "operator_venue_spendability",
  "observed_at_ns": {observed_at_ns},
  "venue_id": "{venue_id}",
  "account_id": "{account_id}",
  "collateral_currency": "{collateral_currency}",
  "spendable_collateral": "{spendable_collateral}",
  "collateral_allowance": "{collateral_allowance}"
}}"#,
            schema_version = crate::bolt_v3_sizing_state::VENUE_SPENDABILITY_SOURCE_SCHEMA_VERSION,
            record_kind = crate::bolt_v3_sizing_state::VENUE_SPENDABILITY_SOURCE_RECORD_KIND,
            venue_id = pool.venue_id,
            account_id = pool.account_id,
            collateral_currency = pool.collateral_currency,
        );
        std::fs::write(&path, payload.as_bytes()).expect("spendability source should write");
        pool.venue_spendability_source_path = Some(path.to_string_lossy().to_string());
        pool.venue_spendability_source_sha256 =
            Some(hex::encode(Sha256::digest(payload.as_bytes())));
        pool.venue_spendability_source_max_bytes = Some(16_384);
    }

    fn loaded_config_with_primary_reference_data() -> LoadedBoltV3Config {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        strategy.config.reference_data.insert(
            "primary".to_string(),
            ReferenceDataBlock {
                data_client_id: ClientId::from("polymarket_main"),
                instrument_id: InstrumentId::from("REFERENCE.SOURCE"),
            },
        );
        loaded
    }

    #[test]
    fn data_client_readiness_quote_plan_uses_client_owned_probe_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include polymarket client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::Configured,
            max_metadata_quote_targets: None,
            allow_metadata_target_sampling: None,
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: Some(BTreeMap::from([(
                "configured_quote_probe".to_string(),
                DataClientReadinessProbeQuoteTargetBlock {
                    instrument_id: InstrumentId::from("REFERENCE.POLYMARKET"),
                },
            )])),
        });

        let (required, ambiguous) =
            strategy_free_data_client_readiness_quote_subscription_plan(&loaded, "polymarket_main")
                .expect("client-owned readiness quote plan should build");

        assert!(ambiguous.is_empty());
        assert_eq!(required.len(), 1);
        assert_eq!(
            required[0].data_client_id,
            ClientId::from("polymarket_main")
        );
        assert_eq!(
            required[0].instrument_id,
            InstrumentId::from("REFERENCE.POLYMARKET")
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_starts_pending_until_targets_arrive() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(2),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");

        assert!(
            !handle.has_all_required_quotes(),
            "metadata-response quote probes must not pass before same-run metadata installs targets"
        );
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
            InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
        ]);

        assert_eq!(installed.len(), 2);
        assert!(
            !handle.has_all_required_quotes(),
            "installing targets should not pass the quote probe until quotes arrive"
        );
        for subscription in installed {
            handle
                .quotes
                .borrow_mut()
                .push(BoltV3StrategyFreeReferenceQuote {
                    data_client_id: subscription.data_client_id.to_string(),
                    instrument_id: subscription.instrument_id.to_string(),
                    bid_price: 1.0,
                    ask_price: 2.0,
                    ts_event_unix_nanos: 1_000,
                    ts_init_unix_nanos: 1_100,
                    captured_at_unix_nanos: 1_200,
                });
        }

        assert!(
            handle.has_all_required_quotes(),
            "metadata-response quote probes should pass after every installed source-owned target has a quote"
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_rejects_unbounded_metadata_universe() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(2),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
            InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
            InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
        ]);

        assert!(
            installed.is_empty(),
            "metadata-response probes must not truncate a broad metadata universe into an arbitrary sample"
        );
        let failure = handle
            .failure_error()
            .expect("unbounded metadata universe should fail closed");
        assert!(
            failure.contains("max_metadata_quote_targets"),
            "failure should name the TOML-owned bound: {failure}"
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_samples_when_explicitly_configured() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(3),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-C.SOURCE"),
            InstrumentId::from("CONFIGURED-A.SOURCE"),
            InstrumentId::from("CONFIGURED-E.SOURCE"),
            InstrumentId::from("CONFIGURED-B.SOURCE"),
            InstrumentId::from("CONFIGURED-D.SOURCE"),
        ]);

        assert_eq!(installed.len(), 3);
        assert_eq!(
            installed[0].instrument_id,
            InstrumentId::from("CONFIGURED-A.SOURCE")
        );
        assert_eq!(
            installed[1].instrument_id,
            InstrumentId::from("CONFIGURED-C.SOURCE")
        );
        assert_eq!(
            installed[2].instrument_id,
            InstrumentId::from("CONFIGURED-E.SOURCE")
        );
        assert!(handle.failure_error().is_none());
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_requires_all_metadata_quote_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(3),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
            InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
            InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
        ]);

        for subscription in installed.iter().take(1) {
            handle
                .quotes
                .borrow_mut()
                .push(BoltV3StrategyFreeReferenceQuote {
                    data_client_id: subscription.data_client_id.to_string(),
                    instrument_id: subscription.instrument_id.to_string(),
                    bid_price: 1.0,
                    ask_price: 2.0,
                    ts_event_unix_nanos: 1_000,
                    ts_init_unix_nanos: 1_100,
                    captured_at_unix_nanos: 1_200,
                });
        }
        assert!(
            !handle.has_all_required_quotes(),
            "metadata-response quote probe must not pass before every same-run metadata target is observed"
        );

        let subscription = installed
            .get(1)
            .expect("second source-owned target should be installed");
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });

        assert!(
            !handle.has_all_required_quotes(),
            "metadata-response quote probe should still wait for the final same-run metadata target"
        );

        let subscription = installed
            .get(2)
            .expect("third source-owned target should be installed");
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });

        assert!(
            handle.has_all_required_quotes(),
            "metadata-response quote probe should pass after all same-run metadata targets are observed"
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_accepts_book_deltas_when_configured() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(1),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness book handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![InstrumentId::from(
            "CONFIGURED-FIRST.SOURCE",
        )]);

        assert_eq!(installed.len(), 1);
        assert!(
            !handle.has_all_required_market_data(),
            "book probes must not pass before a source-owned book-delta event arrives"
        );
        let subscription = &installed[0];
        let delta = OrderBookDelta::new(
            subscription.instrument_id,
            BookAction::Add,
            BookOrder::new(
                OrderSide::Buy,
                Price::from("1.00"),
                Quantity::from("2.00"),
                1,
            ),
            0,
            0,
            1_000.into(),
            1_100.into(),
        );
        let deltas = OrderBookDeltas::new(subscription.instrument_id, vec![delta]);

        handle.record_book_deltas(&deltas, 1_200);

        assert!(
            handle.has_all_required_market_data(),
            "metadata-response book probes should pass after every installed source-owned target has book deltas"
        );
        assert_eq!(handle.book_evidence().deltas.len(), 1);
    }

    #[test]
    fn data_client_readiness_metadata_response_book_probe_passes_at_min_observed_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(5),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: Some(2),
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness book handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-A.SOURCE"),
            InstrumentId::from("CONFIGURED-B.SOURCE"),
            InstrumentId::from("CONFIGURED-C.SOURCE"),
            InstrumentId::from("CONFIGURED-D.SOURCE"),
            InstrumentId::from("CONFIGURED-E.SOURCE"),
        ]);
        assert_eq!(installed.len(), 5);

        let record_delta = |subscription: &StrategyFreeReferenceQuoteSubscription| {
            let delta = OrderBookDelta::new(
                subscription.instrument_id,
                BookAction::Add,
                BookOrder::new(
                    OrderSide::Buy,
                    Price::from("1.00"),
                    Quantity::from("2.00"),
                    1,
                ),
                0,
                0,
                1_000.into(),
                1_100.into(),
            );
            let deltas = OrderBookDeltas::new(subscription.instrument_id, vec![delta]);
            handle.record_book_deltas(&deltas, 1_200);
        };

        assert!(
            !handle.has_all_required_market_data(),
            "book probe must not pass before any sampled target streams a delta"
        );

        record_delta(&installed[0]);
        assert!(
            !handle.has_all_required_market_data(),
            "book probe must keep waiting below min_observed_targets (1 of required 2)"
        );

        record_delta(&installed[1]);
        assert!(
            handle.has_all_required_market_data(),
            "book probe should pass once min_observed_targets sampled targets stream fresh deltas, without requiring every illiquid sampled instrument to tick"
        );
    }

    #[test]
    fn data_client_readiness_probe_rejects_zero_min_observed_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(5),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: Some(0),
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        assert!(
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .is_err(),
            "min_observed_targets=0 must fail closed: a probe that observes nothing proves nothing"
        );
    }

    #[test]
    fn data_client_readiness_probe_fails_closed_when_min_observed_exceeds_sampled() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(5),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: Some(4),
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness book handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-A.SOURCE"),
            InstrumentId::from("CONFIGURED-B.SOURCE"),
        ]);

        assert!(
            installed.is_empty(),
            "install must fail closed when min_observed_targets exceeds the sampled target count"
        );
        assert!(
            !handle.has_all_required_market_data(),
            "probe must not pass after min_observed_targets exceeds the sampled targets"
        );
    }

    #[test]
    fn runtime_redaction_value_buffers_zeroize_on_drop() {
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        fn redaction_values_field(runtime: &BoltV3LiveNodeRuntime) -> &Vec<Zeroizing<String>> {
            &runtime.redaction_values
        }

        assert_zeroize_on_drop::<Vec<Zeroizing<String>>>();
        let _ = redaction_values_field as fn(&BoltV3LiveNodeRuntime) -> &Vec<Zeroizing<String>>;
    }

    #[test]
    fn strategy_free_transport_config_preserves_identity_but_removes_strategy_instances() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        assert!(
            !loaded.strategies.is_empty(),
            "fixture must include strategy config to prove strategy-free transport strips it"
        );

        let strategy_free_loaded = strategy_free_transport_loaded_config(&loaded);

        assert!(
            strategy_free_loaded.strategies.is_empty(),
            "strategy-free transport runtime must not register strategy actors"
        );
        assert_eq!(strategy_free_loaded.root_path, loaded.root_path);
        assert_eq!(
            strategy_free_loaded.config_bundle_checksum,
            loaded.config_bundle_checksum
        );
        assert_eq!(
            strategy_free_loaded.root.strategy_files,
            loaded.root.strategy_files
        );
        assert!(
            !loaded.strategies.is_empty(),
            "helper must not mutate the caller's loaded config"
        );
    }

    #[test]
    fn trade_transport_config_keeps_only_strategy_bound_clients() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut reference_client = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        reference_client.execution = None;
        reference_client.secrets = None;
        let unrelated_client = reference_client.clone();
        loaded
            .root
            .clients
            .insert("reference_data".to_string(), reference_client);
        let mut signal_client = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        signal_client.execution = None;
        signal_client.secrets = None;
        loaded
            .root
            .clients
            .insert("signal_data".to_string(), signal_client);
        loaded
            .root
            .clients
            .insert("unrelated_data".to_string(), unrelated_client);
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        strategy.config.reference_data.insert(
            "primary".to_string(),
            ReferenceDataBlock {
                data_client_id: ClientId::from("reference_data"),
                instrument_id: InstrumentId::from("REFERENCE.SOURCE"),
            },
        );
        strategy.config.signal_data.insert(
            "primary".to_string(),
            ReferenceDataBlock {
                data_client_id: ClientId::from("signal_data"),
                instrument_id: InstrumentId::from("SIGNAL.SOURCE"),
            },
        );

        let scoped = trade_transport_loaded_config(&loaded)
            .expect("strategy-bound transport scope should be derived from config");

        assert_eq!(scoped.root.clients.len(), 3);
        assert!(scoped.root.clients.contains_key("polymarket_main"));
        assert!(scoped.root.clients.contains_key("reference_data"));
        assert!(scoped.root.clients.contains_key("signal_data"));
        assert!(
            !scoped.root.clients.contains_key("unrelated_data"),
            "unrelated configured data clients must not block the selected trade path"
        );
        assert_eq!(scoped.strategies.len(), loaded.strategies.len());
        assert!(
            loaded.root.clients.contains_key("unrelated_data"),
            "helper must not mutate the caller's full client bundle"
        );
    }

    #[test]
    fn data_client_probe_config_keeps_only_selected_data_client() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut secondary = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        secondary.execution = None;
        secondary.secrets = None;
        loaded
            .root
            .clients
            .insert("secondary_data".to_string(), secondary);

        let probe_loaded = data_client_probe_loaded_config(&loaded, "secondary_data")
            .expect("selected data client should produce a scoped probe config");

        assert!(
            probe_loaded.strategies.is_empty(),
            "adapter mapping must drop strategy targets that do not reference the selected probe client"
        );
        assert_eq!(probe_loaded.root_path, loaded.root_path);
        assert_eq!(
            probe_loaded.config_bundle_checksum,
            loaded.config_bundle_checksum
        );
        assert_eq!(probe_loaded.root.clients.len(), 1);
        assert!(probe_loaded.root.clients.contains_key("secondary_data"));
        assert!(
            loaded.root.clients.contains_key("polymarket_main"),
            "helper must not mutate the caller's full client bundle"
        );
    }

    #[test]
    fn data_client_probe_adapter_mapping_drops_unrelated_strategy_targets() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut secondary = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        secondary.execution = None;
        secondary.secrets = None;
        loaded
            .root
            .clients
            .insert("secondary_data".to_string(), secondary);

        let probe_loaded = data_client_probe_loaded_config(&loaded, "secondary_data")
            .expect("selected data client should produce a scoped probe config");

        assert!(
            probe_loaded.strategies.is_empty(),
            "probe mapping input must drop strategy targets that reference clients outside the scoped probe"
        );
        strategy_free_transport_adapter_configs(
            &probe_loaded,
            &crate::bolt_v3_secrets::ResolvedBoltV3Secrets {
                clients: Default::default(),
            },
        )
        .expect("scoped data-client adapter mapping must not fail on unrelated strategies");
    }

    #[test]
    fn data_client_probe_runtime_clears_strategies_after_adapter_mapping() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");

        let probe_loaded = data_client_probe_loaded_config(&loaded, "polymarket_main")
            .expect("selected data client should produce a scoped probe config");
        let runtime_loaded = strategy_free_transport_loaded_config(&probe_loaded);

        assert!(
            !probe_loaded.strategies.is_empty(),
            "probe adapter mapping input must keep strategies for provider-owned data filters"
        );
        assert!(
            runtime_loaded.strategies.is_empty(),
            "strategy-free data-client probes must not register strategy actors"
        );
        assert_eq!(runtime_loaded.root.clients.len(), 1);
        assert!(runtime_loaded.root.clients.contains_key("polymarket_main"));
    }

    #[test]
    fn strategy_free_adapter_mapping_preserves_strategy_derived_market_filters() {
        use crate::{
            bolt_v3_providers::{
                binance::ResolvedBoltV3BinanceSecrets, chainlink::ResolvedBoltV3ChainlinkSecrets,
                polymarket::ResolvedBoltV3PolymarketSecrets,
            },
            bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
        };
        use nautilus_polymarket::config::PolymarketDataClientConfig;
        use std::{collections::BTreeMap, sync::Arc};

        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(ResolvedBoltV3PolymarketSecrets {
                private_key: zeroize::Zeroizing::new("fixture-poly-private-key".to_string()),
                api_key: zeroize::Zeroizing::new("fixture-poly-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("fixture-poly-api-secret".to_string()),
                passphrase: zeroize::Zeroizing::new("fixture-poly-passphrase".to_string()),
            }),
        );
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(ResolvedBoltV3BinanceSecrets {
                api_key: zeroize::Zeroizing::new("fixture-binance-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("fixture-binance-api-secret".to_string()),
            }),
        );
        clients.insert(
            "chainlink_strike".to_string(),
            Arc::new(ResolvedBoltV3ChainlinkSecrets {
                api_key: zeroize::Zeroizing::new("fixture-chainlink-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("fixture-chainlink-api-secret".to_string()),
            }),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let adapters = strategy_free_transport_adapter_configs(&loaded, &resolved)
            .expect("strategy-free adapter mapping should retain market identity filters");
        let polymarket = adapters
            .clients
            .get("polymarket_main")
            .expect("polymarket_main must be mapped");
        let data = polymarket
            .data
            .as_ref()
            .expect("polymarket data config must be mapped")
            .config_as::<PolymarketDataClientConfig>()
            .expect("polymarket data config should downcast");

        assert_eq!(
            data.filters.len(),
            1,
            "strategy-free adapter mapping must keep strategy-derived provider filters"
        );
        assert_eq!(
            data.filters[0]
                .market_slugs()
                .expect("strategy-free data config must keep configured target slug filters")
                .len(),
            2
        );
    }

    #[test]
    fn reference_quote_probe_does_not_satisfy_distinct_clients_with_one_quote() {
        let mut loaded = loaded_config_with_primary_reference_data();
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        let primary = strategy
            .config
            .reference_data
            .get("primary")
            .expect("fixture should include primary reference data")
            .clone();
        strategy.config.reference_data.insert(
            "secondary".to_string(),
            ReferenceDataBlock {
                data_client_id: ClientId::from("secondary_reference"),
                instrument_id: primary.instrument_id,
            },
        );
        let handle = BoltV3StrategyFreeReferenceQuoteProbeHandle::new(&loaded);
        let ambiguity = handle
            .ambiguity_error()
            .expect("probe setup should reject ambiguous reference quote sources");
        assert!(ambiguity.contains("QuoteTick does not carry data_client_id"));
        let quote = QuoteTick::new(
            primary.instrument_id,
            Price::from("100.00"),
            Price::from("100.01"),
            Quantity::from("1"),
            Quantity::from("1"),
            1_u64.into(),
            1_u64.into(),
        );

        handle.record_quote(&quote, 2);

        assert!(
            !handle.has_all_required_quotes(),
            "one source-unattributed QuoteTick must not satisfy distinct data clients"
        );
        assert_eq!(
            handle.evidence().quotes.len(),
            0,
            "probe must not label a source-unattributed QuoteTick with any ambiguous data client"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reference_quote_probe_wait_wakes_when_required_quote_records() {
        let loaded = loaded_config_with_primary_reference_data();
        let handle = BoltV3StrategyFreeReferenceQuoteProbeHandle::new(&loaded);
        let required = handle
            .required
            .borrow()
            .first()
            .expect("fixture should require reference quote evidence")
            .clone();
        let quote = QuoteTick::new(
            required.instrument_id,
            Price::from("100.00"),
            Price::from("100.01"),
            Quantity::from("1"),
            Quantity::from("1"),
            1_u64.into(),
            1_u64.into(),
        );
        let wait = handle.wait_for_all_required_quotes();
        tokio::pin!(wait);

        tokio::select! {
            result = &mut wait => panic!("wait should not complete before required quote evidence: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(5)) => {}
        }

        handle.record_quote(&quote, 2);
        tokio::time::timeout(Duration::from_millis(100), &mut wait)
            .await
            .expect("notify must wake required-quote wait")
            .expect("required quote wait should succeed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reference_quote_probe_wait_accepts_quote_recorded_before_wait_starts() {
        let loaded = loaded_config_with_primary_reference_data();
        let handle = BoltV3StrategyFreeReferenceQuoteProbeHandle::new(&loaded);
        let required = handle
            .required
            .borrow()
            .first()
            .expect("fixture should require reference quote evidence")
            .clone();
        let quote = QuoteTick::new(
            required.instrument_id,
            Price::from("100.00"),
            Price::from("100.01"),
            Quantity::from("1"),
            Quantity::from("1"),
            1_u64.into(),
            1_u64.into(),
        );

        handle.record_quote(&quote, 2);
        tokio::time::timeout(
            Duration::from_millis(100),
            handle.wait_for_all_required_quotes(),
        )
        .await
        .expect("pre-observed quote must not be lost before wait starts")
        .expect("required quote wait should succeed");
    }

    #[test]
    fn live_node_config_maps_trader_id_and_environment_from_v3_root() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert_eq!(cfg.trader_id, TraderId::from("BOLT-001"));
        assert_eq!(cfg.environment, Environment::Live);
        assert_eq!(cfg.timeout_connection, Duration::from_secs(30));
        assert_eq!(cfg.timeout_reconciliation, Duration::from_secs(60));
        assert_eq!(cfg.timeout_portfolio, Duration::from_secs(10));
        assert_eq!(cfg.timeout_disconnection, Duration::from_secs(10));
        assert_eq!(cfg.delay_post_stop, Duration::from_secs(5));
        assert_eq!(cfg.timeout_shutdown, Duration::from_secs(10));
    }

    #[test]
    fn live_node_builder_rejects_backtest_environment_before_registration() {
        let loaded = fixture_loaded_config();
        let make_error = || {
            let mut cfg = make_live_node_config(&loaded);
            cfg.environment = Environment::Backtest;
            make_bolt_v3_live_node_builder_from_config(cfg)
                .expect_err("NT LiveNodeBuilder must reject Backtest environment")
        };

        let rendered = BoltV3LiveNodeError::BuilderConstruction(make_error()).to_string();
        assert_eq!(
            rendered
                .matches("LiveNodeBuilder construction failed")
                .count(),
            1,
            "builder-construction Display should not duplicate layer prefixes: {rendered}"
        );
        assert!(
            rendered.contains("Backtest environment"),
            "builder-construction failure should identify the invalid environment: {rendered}"
        );

        let BoltV3LiveNodeBuilderError::BuilderConstruction { source } = make_error();
        assert!(
            source.to_string().contains("Backtest environment"),
            "builder-construction failure should identify the invalid environment: {source}"
        );
    }

    #[test]
    fn combined_run_and_runtime_capture_shutdown_failure_preserves_both_error_types() {
        let error = classify_live_node_run_and_capture_shutdown(
            Err(anyhow::anyhow!("runner failed")),
            Err(anyhow::anyhow!("capture shutdown failed")),
        )
        .expect_err("combined failure must surface a bolt-v3 live-node error");

        let source = std::error::Error::source(&error)
            .expect("compound failure should expose the runner error as its source");
        assert_eq!(source.to_string(), "runner failed");

        match error {
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            } => {
                assert_eq!(run_error.to_string(), "runner failed");
                assert_eq!(shutdown_error.to_string(), "capture shutdown failed");
            }
            other => panic!(
                "combined runner/capture-shutdown failure must preserve both \
                 error categories, got {other:?}"
            ),
        }
    }

    #[test]
    fn live_node_config_top_level_residuals_are_disabled_or_empty() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert!(cfg.instance_id.is_none());
        assert!(cfg.cache.is_none());
        assert!(cfg.msgbus.is_none());
        assert!(cfg.portfolio.is_none());
        assert!(cfg.emulator.is_none());
        assert!(cfg.streaming.is_none());
        assert!(!cfg.loop_debug);
        assert!(cfg.data_clients.is_empty());
        assert!(cfg.exec_clients.is_empty());
    }

    #[test]
    fn live_node_config_maps_zero_lookback_to_unbounded_reconciliation() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);
        assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
    }

    #[test]
    fn strategy_free_timeout_sums_fail_closed_on_overflow() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.timeout_connection_secs = u64::MAX;
        loaded.root.nautilus.timeout_reconciliation_secs = 1;
        let start_error = strategy_free_start_timeout_secs(&loaded)
            .expect_err("strategy-free start timeout overflow must fail closed");
        assert!(
            matches!(
                start_error,
                BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow
            ),
            "expected start timeout overflow rejection, got {start_error:?}"
        );

        loaded.root.nautilus.timeout_disconnection_secs = u64::MAX;
        loaded.root.nautilus.delay_post_stop_secs = 1;
        let stop_error = strategy_free_stop_timeout_secs(&loaded)
            .expect_err("strategy-free stop timeout overflow must fail closed");
        assert!(
            matches!(
                stop_error,
                BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow
            ),
            "expected stop timeout overflow rejection, got {stop_error:?}"
        );
    }

    #[test]
    fn live_node_config_maps_explicit_nt_runtime_defaults_from_v3_root() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert!(cfg.data_engine.time_bars_build_with_no_updates);
        assert!(cfg.data_engine.time_bars_timestamp_on_close);
        assert!(!cfg.data_engine.time_bars_skip_first_non_full_bar);
        assert_eq!(
            cfg.data_engine.time_bars_interval_type,
            nautilus_model::enums::BarIntervalType::LeftOpen
        );
        assert_eq!(cfg.data_engine.time_bars_build_delay, 0);
        assert!(cfg.data_engine.time_bars_origin_offset.is_empty());
        assert!(!cfg.data_engine.validate_data_sequence);
        assert!(!cfg.data_engine.buffer_deltas);
        assert!(!cfg.data_engine.emit_quotes_from_book);
        assert!(!cfg.data_engine.emit_quotes_from_book_depths);
        assert_eq!(cfg.data_engine.external_clients, None);
        assert!(!cfg.data_engine.debug);
        assert!(!cfg.data_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.data_engine.qsize, 100_000);
        assert!(cfg.exec_engine.load_cache);
        assert!(!cfg.exec_engine.snapshot_orders);
        assert!(!cfg.exec_engine.snapshot_positions);
        assert_eq!(cfg.exec_engine.snapshot_positions_interval_secs, None);
        assert_eq!(cfg.exec_engine.external_clients, None);
        assert!(!cfg.exec_engine.debug);
        assert!(cfg.exec_engine.reconciliation);
        assert_eq!(cfg.exec_engine.reconciliation_startup_delay_secs, 10.0);
        assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
        assert_eq!(cfg.exec_engine.reconciliation_instrument_ids, None);
        assert!(!cfg.exec_engine.filter_unclaimed_external_orders);
        assert!(!cfg.exec_engine.filter_position_reports);
        assert_eq!(cfg.exec_engine.filtered_client_order_ids, None);
        assert!(cfg.exec_engine.generate_missing_orders);
        assert_eq!(cfg.exec_engine.inflight_check_interval_ms, 2_000);
        assert_eq!(cfg.exec_engine.inflight_check_threshold_ms, 5_000);
        assert_eq!(cfg.exec_engine.inflight_check_retries, 5);
        assert_eq!(cfg.exec_engine.open_check_interval_secs, None);
        assert_eq!(cfg.exec_engine.open_check_lookback_mins, Some(60));
        assert_eq!(cfg.exec_engine.open_check_threshold_ms, 5_000);
        assert_eq!(cfg.exec_engine.open_check_missing_retries, 5);
        assert!(cfg.exec_engine.open_check_open_only);
        assert_eq!(cfg.exec_engine.max_single_order_queries_per_cycle, 10);
        assert_eq!(cfg.exec_engine.single_order_query_delay_ms, 100);
        assert_eq!(cfg.exec_engine.position_check_interval_secs, None);
        assert_eq!(cfg.exec_engine.position_check_lookback_mins, 60);
        assert_eq!(cfg.exec_engine.position_check_threshold_ms, 5_000);
        assert_eq!(cfg.exec_engine.position_check_retries, 3);
        assert_eq!(cfg.exec_engine.purge_closed_orders_interval_mins, None);
        assert_eq!(cfg.exec_engine.purge_closed_orders_buffer_mins, None);
        assert_eq!(cfg.exec_engine.purge_closed_positions_interval_mins, None);
        assert_eq!(cfg.exec_engine.purge_closed_positions_buffer_mins, None);
        assert_eq!(cfg.exec_engine.purge_account_events_interval_mins, None);
        assert_eq!(cfg.exec_engine.purge_account_events_lookback_mins, None);
        assert!(!cfg.exec_engine.purge_from_database);
        assert_eq!(cfg.exec_engine.own_books_audit_interval_secs, None);
        assert!(!cfg.exec_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.exec_engine.qsize, 100_000);
        assert!(!cfg.exec_engine.allow_overfills);
        assert!(!cfg.exec_engine.manage_own_order_books);
        assert!(!cfg.risk_engine.bypass);
        assert_eq!(cfg.risk_engine.max_order_submit_rate, "40/00:01:00");
        assert_eq!(cfg.risk_engine.max_order_modify_rate, "40/00:01:00");
        assert!(cfg.risk_engine.max_notional_per_order.is_empty());
        assert!(!cfg.risk_engine.debug);
        assert!(!cfg.risk_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.risk_engine.qsize, 100_000);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_risk_debug_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.risk.nautilus.debug = true;

        let cfg = make_live_node_config(&loaded);

        assert!(cfg.risk_engine.debug);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_data_engine_debug_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.data_engine.debug = true;

        let cfg = make_live_node_config(&loaded);

        assert!(cfg.data_engine.debug);
    }

    #[test]
    fn live_node_config_maps_non_empty_nt_max_notional_per_order() {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .insert("REFERENCE.SOURCE".to_string(), "12345.00".to_string());
        loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .insert("SECONDARY.SOURCE".to_string(), "25000.50".to_string());
        let cfg = make_live_node_config(&loaded);

        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("REFERENCE.SOURCE"),
            Some(&"12345.00".to_string())
        );
        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("SECONDARY.SOURCE"),
            Some(&"25000.50".to_string())
        );
    }

    #[test]
    fn venue_spendability_source_config_reads_configured_capital_pool_source() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let mut loaded = fixture_loaded_config();
        write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
        let config = position_sizer_venue_spendability_source_config_from_loaded(&loaded)
            .expect("source config should build")
            .expect("fixture should configure source");

        let snapshot = position_sizer_venue_spendability_snapshot_from_source_config(&config)
            .expect("configured source should be accepted");

        assert_eq!(snapshot.source, "operator_venue_spendability");
        assert_eq!(snapshot.spendable_collateral, Decimal::from(20));
        assert_eq!(snapshot.collateral_allowance, Decimal::from(12));
    }

    #[test]
    fn venue_spendability_source_config_fails_closed_on_sha_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let mut loaded = fixture_loaded_config();
        write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
        let mut config = position_sizer_venue_spendability_source_config_from_loaded(&loaded)
            .expect("source config should build")
            .expect("fixture should configure source");
        config.expected_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        let error = position_sizer_venue_spendability_snapshot_from_source_config(&config)
            .expect_err("hash mismatch must fail closed");
        let rendered = error.to_string();

        assert!(
            rendered.contains("position sizer venue spendability source rejected")
                && rendered.contains("Sha256Mismatch"),
            "startup error should name rejected spendability evidence, got: {rendered}"
        );
    }

    #[test]
    fn live_node_config_maps_log_levels_from_uppercase_strings() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);
        assert_eq!(cfg.logging.stdout_level, log::LevelFilter::Info);
        assert_eq!(cfg.logging.fileout_level, log::LevelFilter::Info);
    }

    #[test]
    fn live_node_config_logger_literal_does_not_inherit_nt_defaults() {
        let src = include_str!("bolt_v3_live_node.rs");
        let logging_literal = src
            .split("let logging = LoggerConfig {")
            .nth(1)
            .expect("logger config literal must exist")
            .split("let nautilus =")
            .next()
            .expect("logger config literal must precede nautilus config");

        // Field-add drift is caught by Rust struct literal exhaustiveness; this
        // guards against silently re-introducing inherited NT defaults.
        assert!(
            !logging_literal.contains(concat!("..", "Default::default()")),
            "LoggerConfig must set every pinned NT field explicitly"
        );
    }

    #[test]
    fn live_node_config_maps_explicit_logger_residuals_in_builder_path() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert!(cfg.logging.component_level.is_empty());
        assert!(!cfg.logging.log_components_only);
        assert!(cfg.logging.is_colored);
        assert!(!cfg.logging.print_config);
        assert!(!cfg.logging.use_tracing);
        assert!(!cfg.logging.bypass_logging);
        assert!(cfg.logging.file_config.is_none());
        assert!(!cfg.logging.clear_log_file);
    }

    #[test]
    fn live_node_config_suppresses_nt_credential_module_logs_to_warn() {
        // Regression for the slice-7 review finding: NT's
        // `nautilus_polymarket::common::credential` and
        // `nautilus_binance::common::credential` modules log credential
        // material at info-level. Bolt-v3 forces those targets to
        // `Warn` even when the root TOML log level is `Info`, so the
        // logger filter must contain both module paths with at most
        // `Warn` regardless of the configured root level.
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        for module_path in crate::bolt_v3_providers::credential_log_modules() {
            let key = Ustr::from(module_path);
            let level = cfg
                .logging
                .module_level
                .get(&key)
                .copied()
                .unwrap_or_else(|| panic!("logger module_level missing `{module_path}`"));
            assert!(
                level <= log::LevelFilter::Warn,
                "credential module `{module_path}` filter must be Warn or stricter, got {level:?}"
            );
        }
    }

    #[test]
    fn secret_resolver_setup_variant_renders_clean_message_without_empty_client_path() {
        // Per #255-2: before this fix, session-construction failure was
        // mapped into `BoltV3SecretError` with empty `client_key` and
        // `ssm_path`, rendering as a confusing
        // an empty client key in the secret-path template. The dedicated
        // `BoltV3LiveNodeError::SecretResolverSetup(SecretError)` variant
        // gives operators a clean, accurate message that does not
        // pretend a client or SSM path is involved (none is — the
        // failure happens before any path is read).
        let inner = crate::secrets::SecretError::for_test(
            "failed to build Tokio runtime for SSM resolver session: simulated".to_string(),
        );
        let err = BoltV3LiveNodeError::SecretResolverSetup(inner);
        let rendered = format!("{err}");
        assert!(
            !rendered.contains(".secrets.ssm_resolver_session"),
            "SecretResolverSetup must not render through the client/SSM-path template"
        );
        assert!(
            !rendered.contains("ssm_path"),
            "SecretResolverSetup must not include an empty ssm_path field"
        );
        assert!(
            rendered.contains("SSM resolver session"),
            "SecretResolverSetup message must name the resolver-session setup boundary"
        );
        assert!(
            rendered.contains("simulated"),
            "SecretResolverSetup must surface the wrapped SecretError"
        );
        let source = std::error::Error::source(&err);
        assert!(
            source.is_some(),
            "SecretResolverSetup must report its wrapped SecretError via \
             std::error::Error::source"
        );
    }
}
