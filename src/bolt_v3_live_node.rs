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
//! external network connection. Opt-in controlled-connect/no-submit
//! readiness boundaries may open adapter sockets. The production
//! trading runner entrypoint is [`run_bolt_v3_live_node`], which first
//! applies the bolt-v3 live canary gate. The no-submit readiness path
//! builds a strategy-free node before using NT's supported runner loop
//! with handle-driven stop; its dedicated quote probes call only NT
//! quote subscribe/unsubscribe APIs for configured strategy
//! `[reference_data]` or client-owned readiness-probe instruments. This
//! module still never constructs an order or enables any submit path
//! from its own boundary code.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ahash::AHashMap;
use anyhow::Result;
use log::LevelFilter;
use nautilus_common::{
    actor::{DataActor, DataActorConfig, DataActorCore, registry::try_get_actor_unchecked},
    enums::Environment,
    logging::logger::LoggerConfig,
    messages::data::InstrumentsResponse,
    msgbus::ShareableMessageHandler,
    nautilus_actor,
};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle, NodeState},
};
use nautilus_model::{
    data::{OrderBookDeltas, QuoteTick},
    enums::{BarIntervalType, BookType},
    identifiers::{ActorId, ClientId, InstrumentId, StrategyId, Venue},
    instruments::Instrument,
};
use rust_decimal::Decimal;
use ustr::Ustr;
use zeroize::Zeroizing;

use crate::{
    bolt_v3_adapters::{BoltV3AdapterConfigs, BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_canary_proof_executor::register_canary_proof_executor_on_node,
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{
        DataClientReadinessProbeBookType, DataClientReadinessProbeMarketDataKind,
        DataClientReadinessProbeQuoteTargetSource, LoadedBoltV3Config,
    },
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3StrategyInputEvidenceSnapshot, JsonlBoltV3DecisionEvidenceWriter,
    },
    bolt_v3_live_canary_gate::{
        BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_pre_consumption_gate,
        current_build_head_sha,
    },
    bolt_v3_providers,
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars, check_no_forbidden_credential_env_vars_with,
        resolve_bolt_v3_secrets, resolve_bolt_v3_secrets_with,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, register_bolt_v3_strategies_on_node_with_bindings,
    },
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionState},
    bolt_v3_tiny_canary_evidence::{
        Phase8CanaryBlockReason, Phase8CanaryEvidence, Phase8CanaryEvidenceInput,
        Phase8EvidenceRef, Phase8OperatorApprovalEnvelope, Phase8RuntimeCaptureRef,
        phase8_sha256_text,
    },
    nt_runtime_capture::{NtRuntimeCaptureGuards, wire_nt_runtime_capture},
    secrets::SsmResolverSession,
};

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    registration_summary: BoltV3RegistrationSummary,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    redaction_values: Vec<Zeroizing<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitReferenceCacheEvidence {
    cached_instrument_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3NoSubmitReferenceQuote {
    pub data_client_id: String,
    pub instrument_id: String,
    pub bid_price: f64,
    pub ask_price: f64,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
    pub captured_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3NoSubmitReferenceQuoteEvidence {
    pub quotes: Vec<BoltV3NoSubmitReferenceQuote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitBookDeltas {
    pub data_client_id: String,
    pub instrument_id: String,
    pub delta_count: u64,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
    pub captured_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitBookDeltasEvidence {
    pub deltas: Vec<BoltV3NoSubmitBookDeltas>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitDataClientMetadata {
    pub data_client_id: String,
    pub venue: String,
    pub instrument_ids: Vec<String>,
    pub ts_init_unix_nanos: u64,
    pub captured_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitDataClientMetadataEvidence {
    pub responses: Vec<BoltV3NoSubmitDataClientMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3NoSubmitDataClientReadinessEvidence {
    pub metadata: BoltV3NoSubmitDataClientMetadataEvidence,
    pub quotes: BoltV3NoSubmitReferenceQuoteEvidence,
    pub books: BoltV3NoSubmitBookDeltasEvidence,
}

impl BoltV3NoSubmitReferenceQuoteEvidence {
    pub fn observed_at_unix_nanos(&self) -> Option<u64> {
        self.quotes
            .iter()
            .map(|quote| quote.captured_at_unix_nanos)
            .max()
    }
}

impl BoltV3NoSubmitDataClientMetadataEvidence {
    pub fn observed_at_unix_nanos(&self) -> Option<u64> {
        self.responses
            .iter()
            .map(|response| response.captured_at_unix_nanos)
            .max()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoSubmitReferenceQuoteSubscription {
    data_client_id: ClientId,
    instrument_id: InstrumentId,
}

#[derive(Debug, Clone)]
struct BoltV3NoSubmitReferenceQuoteProbeHandle {
    required: Rc<RefCell<Vec<NoSubmitReferenceQuoteSubscription>>>,
    ambiguous_instrument_ids: Rc<RefCell<BTreeSet<String>>>,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<DataClientReadinessProbeBookType>,
    metadata_response_data_client_id: Option<ClientId>,
    metadata_response_max_quote_targets: Option<usize>,
    metadata_response_allow_target_sampling: bool,
    quote_targets_initialized: Rc<Cell<bool>>,
    failure_reason: Rc<RefCell<Option<String>>>,
    quotes: Rc<RefCell<Vec<BoltV3NoSubmitReferenceQuote>>>,
    book_deltas: Rc<RefCell<Vec<BoltV3NoSubmitBookDeltas>>>,
    quote_notify: Rc<tokio::sync::Notify>,
}

impl BoltV3NoSubmitReferenceQuoteProbeHandle {
    fn new(loaded: &LoadedBoltV3Config) -> Self {
        let (required, ambiguous_instrument_ids) =
            no_submit_reference_quote_subscription_plan(loaded);
        Self::from_plan(
            required,
            ambiguous_instrument_ids,
            DataClientReadinessProbeMarketDataKind::Quote,
            None,
        )
    }

    fn from_plan(
        required: Vec<NoSubmitReferenceQuoteSubscription>,
        ambiguous_instrument_ids: BTreeSet<String>,
        market_data_kind: DataClientReadinessProbeMarketDataKind,
        book_type: Option<DataClientReadinessProbeBookType>,
    ) -> Self {
        Self {
            required: Rc::new(RefCell::new(required)),
            ambiguous_instrument_ids: Rc::new(RefCell::new(ambiguous_instrument_ids)),
            market_data_kind,
            book_type,
            metadata_response_data_client_id: None,
            metadata_response_max_quote_targets: None,
            metadata_response_allow_target_sampling: false,
            quote_targets_initialized: Rc::new(Cell::new(true)),
            failure_reason: Rc::new(RefCell::new(None)),
            quotes: Rc::new(RefCell::new(Vec::new())),
            book_deltas: Rc::new(RefCell::new(Vec::new())),
            quote_notify: Rc::new(tokio::sync::Notify::new()),
        }
    }

    fn from_metadata_response_plan(
        data_client_id: ClientId,
        max_quote_targets: usize,
        allow_target_sampling: bool,
        market_data_kind: DataClientReadinessProbeMarketDataKind,
        book_type: Option<DataClientReadinessProbeBookType>,
    ) -> Self {
        Self {
            required: Rc::new(RefCell::new(Vec::new())),
            ambiguous_instrument_ids: Rc::new(RefCell::new(BTreeSet::new())),
            market_data_kind,
            book_type,
            metadata_response_data_client_id: Some(data_client_id),
            metadata_response_max_quote_targets: Some(max_quote_targets),
            metadata_response_allow_target_sampling: allow_target_sampling,
            quote_targets_initialized: Rc::new(Cell::new(false)),
            failure_reason: Rc::new(RefCell::new(None)),
            quotes: Rc::new(RefCell::new(Vec::new())),
            book_deltas: Rc::new(RefCell::new(Vec::new())),
            quote_notify: Rc::new(tokio::sync::Notify::new()),
        }
    }

    #[cfg(test)]
    fn has_all_required_quotes(&self) -> bool {
        if self.market_data_kind != DataClientReadinessProbeMarketDataKind::Quote {
            return false;
        }
        self.has_all_required_market_data()
    }

    fn has_all_required_market_data(&self) -> bool {
        if self.failure_error().is_some() {
            return false;
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
        match self.market_data_kind {
            DataClientReadinessProbeMarketDataKind::Quote => {
                let quotes = self.quotes.borrow();
                observed_required_quote_count(&required, &quotes) == required.len()
            }
            DataClientReadinessProbeMarketDataKind::Book => {
                let book_deltas = self.book_deltas.borrow();
                observed_required_book_delta_count(&required, &book_deltas) == required.len()
            }
        }
    }

    fn ambiguity_error(&self) -> Option<String> {
        if self.ambiguous_instrument_ids.borrow().is_empty() {
            return None;
        }
        Some(
            "reference quote probe cannot distinguish multiple data clients for the same instrument_id; QuoteTick does not carry data_client_id"
                .to_string(),
        )
    }

    fn failure_error(&self) -> Option<String> {
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

    fn evidence(&self) -> BoltV3NoSubmitReferenceQuoteEvidence {
        BoltV3NoSubmitReferenceQuoteEvidence {
            quotes: self.quotes.borrow().clone(),
        }
    }

    fn book_evidence(&self) -> BoltV3NoSubmitBookDeltasEvidence {
        BoltV3NoSubmitBookDeltasEvidence {
            deltas: self.book_deltas.borrow().clone(),
        }
    }

    fn install_metadata_response_instrument_ids(
        &self,
        mut instrument_ids: Vec<InstrumentId>,
    ) -> Vec<NoSubmitReferenceQuoteSubscription> {
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
            .map(|instrument_id| NoSubmitReferenceQuoteSubscription {
                data_client_id,
                instrument_id,
            })
            .collect();
        let (required, ambiguous_instrument_ids) =
            dedupe_no_submit_reference_quote_subscriptions(subscriptions);
        *self.required.borrow_mut() = required.clone();
        *self.ambiguous_instrument_ids.borrow_mut() = ambiguous_instrument_ids;
        self.quote_targets_initialized.set(true);
        self.quote_notify.notify_one();
        required
    }

    fn record_quote(&self, quote: &QuoteTick, captured_at_unix_nanos: u64) {
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
                quotes.push(BoltV3NoSubmitReferenceQuote {
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

    fn record_book_deltas(&self, deltas: &OrderBookDeltas, captured_at_unix_nanos: u64) {
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
                book_deltas.push(BoltV3NoSubmitBookDeltas {
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

    async fn wait_for_all_required_quotes(&self) -> Result<(), String> {
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

fn observed_required_book_delta_count(
    required: &[NoSubmitReferenceQuoteSubscription],
    book_deltas: &[BoltV3NoSubmitBookDeltas],
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
    required: &[NoSubmitReferenceQuoteSubscription],
    quotes: &[BoltV3NoSubmitReferenceQuote],
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

#[derive(Debug, Clone)]
struct BoltV3NoSubmitDataClientMetadataProbeHandle {
    data_client_id: ClientId,
    venue: Venue,
    responses: Rc<RefCell<Vec<BoltV3NoSubmitDataClientMetadata>>>,
    metadata_notify: Rc<tokio::sync::Notify>,
}

impl BoltV3NoSubmitDataClientMetadataProbeHandle {
    fn new(loaded: &LoadedBoltV3Config, client_key: &str) -> Result<Self, BoltV3LiveNodeError> {
        let client = loaded.root.clients.get(client_key).ok_or_else(|| {
            BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
                "data-client metadata probe client_key is not configured"
            ))
        })?;
        if client.data.is_none() {
            return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client metadata probe requires the selected client to declare [data]"
                ),
            ));
        }

        Ok(Self {
            data_client_id: ClientId::from(client_key),
            venue: client.venue,
            responses: Rc::new(RefCell::new(Vec::new())),
            metadata_notify: Rc::new(tokio::sync::Notify::new()),
        })
    }

    fn has_metadata_response(&self) -> bool {
        !self.responses.borrow().is_empty()
    }

    fn evidence(&self) -> BoltV3NoSubmitDataClientMetadataEvidence {
        BoltV3NoSubmitDataClientMetadataEvidence {
            responses: self.responses.borrow().clone(),
        }
    }

    fn record_response(&self, response: &InstrumentsResponse, captured_at_unix_nanos: u64) {
        if response.client_id != self.data_client_id || response.venue != self.venue {
            return;
        }
        let mut instrument_ids: Vec<String> = response
            .data
            .iter()
            .map(|instrument| instrument.id().to_string())
            .collect();
        instrument_ids.sort();
        self.responses
            .borrow_mut()
            .push(BoltV3NoSubmitDataClientMetadata {
                data_client_id: response.client_id.to_string(),
                venue: response.venue.to_string(),
                instrument_ids,
                ts_init_unix_nanos: response.ts_init.as_u64(),
                captured_at_unix_nanos,
            });
        self.metadata_notify.notify_one();
    }

    async fn wait_for_metadata_response(&self) {
        while !self.has_metadata_response() {
            self.metadata_notify.notified().await;
        }
    }
}

#[derive(Debug)]
struct BoltV3NoSubmitReferenceQuoteProbe {
    core: DataActorCore,
    handle: BoltV3NoSubmitReferenceQuoteProbeHandle,
}

nautilus_actor!(BoltV3NoSubmitReferenceQuoteProbe);

impl BoltV3NoSubmitReferenceQuoteProbe {
    fn new(handle: BoltV3NoSubmitReferenceQuoteProbeHandle, config: DataActorConfig) -> Self {
        Self {
            core: DataActorCore::new(config),
            handle,
        }
    }
}

impl DataActor for BoltV3NoSubmitReferenceQuoteProbe {
    fn on_start(&mut self) -> anyhow::Result<()> {
        let required_subscriptions = self.handle.required.borrow().clone();
        let market_data_kind = self.handle.market_data_kind;
        let book_type = self.handle.book_type;
        for required in required_subscriptions {
            subscribe_no_submit_required_market_data(self, market_data_kind, book_type, required)?;
        }
        Ok(())
    }

    fn on_stop(&mut self) -> anyhow::Result<()> {
        let required_subscriptions = self.handle.required.borrow().clone();
        let market_data_kind = self.handle.market_data_kind;
        let book_type = self.handle.book_type;
        for required in required_subscriptions {
            unsubscribe_no_submit_required_market_data(
                self,
                market_data_kind,
                book_type,
                required,
            )?;
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        self.handle.record_quote(quote, current_unix_nanos()?);
        Ok(())
    }

    fn on_book_deltas(&mut self, deltas: &OrderBookDeltas) -> anyhow::Result<()> {
        self.handle
            .record_book_deltas(deltas, current_unix_nanos()?);
        Ok(())
    }
}

#[derive(Debug)]
struct BoltV3NoSubmitDataClientReadinessProbe {
    core: DataActorCore,
    metadata_handle: BoltV3NoSubmitDataClientMetadataProbeHandle,
    quote_handle: BoltV3NoSubmitReferenceQuoteProbeHandle,
}

nautilus_actor!(BoltV3NoSubmitDataClientReadinessProbe);

impl BoltV3NoSubmitDataClientReadinessProbe {
    fn new(
        metadata_handle: BoltV3NoSubmitDataClientMetadataProbeHandle,
        quote_handle: BoltV3NoSubmitReferenceQuoteProbeHandle,
        config: DataActorConfig,
    ) -> Self {
        Self {
            core: DataActorCore::new(config),
            metadata_handle,
            quote_handle,
        }
    }

    fn handle_instruments_metadata_response(&mut self, response: &InstrumentsResponse) {
        self.metadata_handle
            .record_response(response, self.timestamp_ns().as_u64());
        if response.client_id == self.metadata_handle.data_client_id
            && response.venue == self.metadata_handle.venue
        {
            let instrument_ids = response
                .data
                .iter()
                .map(|instrument| instrument.id())
                .collect();
            for required in self
                .quote_handle
                .install_metadata_response_instrument_ids(instrument_ids)
            {
                if let Err(error) = subscribe_no_submit_required_market_data(
                    self,
                    self.quote_handle.market_data_kind,
                    self.quote_handle.book_type,
                    required,
                ) {
                    self.quote_handle
                        .fail_metadata_response_probe(error.to_string());
                    break;
                }
            }
        }
    }
}

impl DataActor for BoltV3NoSubmitDataClientReadinessProbe {
    fn on_start(&mut self) -> anyhow::Result<()> {
        let actor_id = self.actor_id().inner();
        let handler = ShareableMessageHandler::from_typed(move |response: &InstrumentsResponse| {
            if let Some(mut actor) =
                try_get_actor_unchecked::<BoltV3NoSubmitDataClientReadinessProbe>(&actor_id)
            {
                actor.handle_instruments_metadata_response(response);
            } else {
                log::error!("Actor {actor_id} not found for data-client metadata handling");
            }
        });
        self.core.request_instruments(
            Some(self.metadata_handle.venue),
            None,
            None,
            Some(self.metadata_handle.data_client_id),
            None,
            handler,
        )?;
        let required_subscriptions = self.quote_handle.required.borrow().clone();
        let market_data_kind = self.quote_handle.market_data_kind;
        let book_type = self.quote_handle.book_type;
        for required in required_subscriptions {
            subscribe_no_submit_required_market_data(self, market_data_kind, book_type, required)?;
        }
        Ok(())
    }

    fn on_stop(&mut self) -> anyhow::Result<()> {
        let required_subscriptions = self.quote_handle.required.borrow().clone();
        let market_data_kind = self.quote_handle.market_data_kind;
        let book_type = self.quote_handle.book_type;
        for required in required_subscriptions {
            unsubscribe_no_submit_required_market_data(
                self,
                market_data_kind,
                book_type,
                required,
            )?;
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        self.quote_handle.record_quote(quote, current_unix_nanos()?);
        Ok(())
    }

    fn on_book_deltas(&mut self, deltas: &OrderBookDeltas) -> anyhow::Result<()> {
        self.quote_handle
            .record_book_deltas(deltas, current_unix_nanos()?);
        Ok(())
    }
}

fn subscribe_no_submit_required_market_data<A: DataActor + std::fmt::Debug + 'static>(
    actor: &mut A,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<DataClientReadinessProbeBookType>,
    required: NoSubmitReferenceQuoteSubscription,
) -> anyhow::Result<()> {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => {
            actor.subscribe_quotes(required.instrument_id, Some(required.data_client_id), None);
        }
        DataClientReadinessProbeMarketDataKind::Book => {
            let book_type = book_type.ok_or_else(|| {
                anyhow::anyhow!(
                    "clients.<id>.readiness_probe.book_type must be configured when market_data_kind = \"book\""
                )
            })?;
            actor.subscribe_book_deltas(
                required.instrument_id,
                data_client_readiness_probe_book_type_to_nt(book_type),
                None,
                Some(required.data_client_id),
                false,
                None,
            );
        }
    }
    Ok(())
}

fn unsubscribe_no_submit_required_market_data<A: DataActor + std::fmt::Debug + 'static>(
    actor: &mut A,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<DataClientReadinessProbeBookType>,
    required: NoSubmitReferenceQuoteSubscription,
) -> anyhow::Result<()> {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => {
            actor.unsubscribe_quotes(required.instrument_id, Some(required.data_client_id), None);
        }
        DataClientReadinessProbeMarketDataKind::Book => {
            let _ = book_type.ok_or_else(|| {
                anyhow::anyhow!(
                    "clients.<id>.readiness_probe.book_type must be configured when market_data_kind = \"book\""
                )
            })?;
            actor.unsubscribe_book_deltas(
                required.instrument_id,
                Some(required.data_client_id),
                None,
            );
        }
    }
    Ok(())
}

fn data_client_readiness_probe_book_type_to_nt(
    book_type: DataClientReadinessProbeBookType,
) -> BookType {
    match book_type {
        DataClientReadinessProbeBookType::L1Mbp => BookType::L1_MBP,
        DataClientReadinessProbeBookType::L2Mbp => BookType::L2_MBP,
        DataClientReadinessProbeBookType::L3Mbo => BookType::L3_MBO,
    }
}

fn no_submit_reference_quote_subscription_plan(
    loaded: &LoadedBoltV3Config,
) -> (Vec<NoSubmitReferenceQuoteSubscription>, BTreeSet<String>) {
    let mut subscriptions = Vec::new();
    for strategy in &loaded.strategies {
        for reference in strategy.config.reference_data.values() {
            subscriptions.push(NoSubmitReferenceQuoteSubscription {
                data_client_id: reference.data_client_id,
                instrument_id: reference.instrument_id,
            });
        }
    }
    dedupe_no_submit_reference_quote_subscriptions(subscriptions)
}

fn no_submit_data_client_readiness_quote_subscription_plan(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<(Vec<NoSubmitReferenceQuoteSubscription>, BTreeSet<String>), BoltV3LiveNodeError> {
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness quote probe client_key is not configured"
        ))
    })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "data-client readiness quote probe requires the selected client to declare [data]"
            ),
        ));
    }
    let readiness_probe = client.readiness_probe.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
        ))
    })?;
    if readiness_probe.quote_target_source != DataClientReadinessProbeQuoteTargetSource::Configured
    {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "standalone data-client readiness quote probe requires quote_target_source = \"configured\"; metadata_response requires the combined data-client readiness probe"
            ),
        ));
    }
    let Some(quote_targets) = &readiness_probe.quote_targets else {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
            ),
        ));
    };
    if quote_targets.is_empty() {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
            ),
        ));
    }
    let subscriptions = quote_targets
        .values()
        .map(|target| NoSubmitReferenceQuoteSubscription {
            data_client_id: ClientId::from(client_key),
            instrument_id: target.instrument_id,
        })
        .collect();
    Ok(dedupe_no_submit_reference_quote_subscriptions(
        subscriptions,
    ))
}

fn no_submit_data_client_readiness_quote_probe_handle(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3NoSubmitReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness probe client_key is not configured"
        ))
    })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "data-client readiness probe requires the selected client to declare [data]"
            ),
        ));
    }
    let Some(readiness_probe) = &client.readiness_probe else {
        return Ok(BoltV3NoSubmitReferenceQuoteProbeHandle::from_plan(
            Vec::new(),
            BTreeSet::new(),
            DataClientReadinessProbeMarketDataKind::Quote,
            None,
        ));
    };
    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            let Some(quote_targets) = &readiness_probe.quote_targets else {
                return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
                    anyhow::anyhow!(
                        "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                    ),
                ));
            };
            if quote_targets.is_empty() {
                return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
                    anyhow::anyhow!(
                        "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                    ),
                ));
            }
            let subscriptions = quote_targets
                .values()
                .map(|target| NoSubmitReferenceQuoteSubscription {
                    data_client_id: ClientId::from(client_key),
                    instrument_id: target.instrument_id,
                })
                .collect();
            let (required, ambiguous_instrument_ids) =
                dedupe_no_submit_reference_quote_subscriptions(subscriptions);
            Ok(BoltV3NoSubmitReferenceQuoteProbeHandle::from_plan(
                required,
                ambiguous_instrument_ids,
                readiness_probe.market_data_kind,
                readiness_probe.book_type,
            ))
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            let max_quote_targets = readiness_probe.max_metadata_quote_targets.ok_or_else(|| {
                BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.max_metadata_quote_targets when quote_target_source = \"metadata_response\""
                ))
            })?;
            if max_quote_targets == 0 {
                return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
                    anyhow::anyhow!(
                        "data-client readiness quote probe requires positive clients.<id>.readiness_probe.max_metadata_quote_targets"
                    ),
                ));
            }
            let allow_target_sampling = readiness_probe
                .allow_metadata_target_sampling
                .ok_or_else(|| {
                    BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
                        "data-client readiness quote probe requires clients.<id>.readiness_probe.allow_metadata_target_sampling when quote_target_source = \"metadata_response\""
                    ))
                })?;
            Ok(
                BoltV3NoSubmitReferenceQuoteProbeHandle::from_metadata_response_plan(
                    ClientId::from(client_key),
                    max_quote_targets,
                    allow_target_sampling,
                    readiness_probe.market_data_kind,
                    readiness_probe.book_type,
                ),
            )
        }
    }
}

fn dedupe_no_submit_reference_quote_subscriptions(
    subscriptions: Vec<NoSubmitReferenceQuoteSubscription>,
) -> (Vec<NoSubmitReferenceQuoteSubscription>, BTreeSet<String>) {
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

impl BoltV3NoSubmitReferenceCacheEvidence {
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
}

impl BoltV3LiveNodeRuntime {
    fn new(
        node: LiveNode,
        registration_summary: BoltV3RegistrationSummary,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        redaction_values: Vec<Zeroizing<String>>,
    ) -> Self {
        Self {
            node,
            registration_summary,
            submit_admission,
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

    pub fn reference_cache_evidence(&self) -> BoltV3NoSubmitReferenceCacheEvidence {
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        let cached_instrument_ids = cache
            .instrument_ids(None)
            .into_iter()
            .map(ToString::to_string)
            .collect();
        BoltV3NoSubmitReferenceCacheEvidence {
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
    Build(anyhow::Error),
    /// The live canary gate rejected entry to NT's runner loop before
    /// `LiveNode::run` was invoked. This variant wraps the specific
    /// fail-closed reason from [`BoltV3LiveCanaryGateError`].
    LiveCanaryGate(BoltV3LiveCanaryGateError),
    /// The live runner entrypoint passed the pre-consumption gate but
    /// could not atomically create the operator-approval consumption
    /// proof before arming submit admission.
    OperatorApprovalConsumption(anyhow::Error),
    /// The loaded root TOML configured clients beyond the selected
    /// strategy-owned transport path, but the strategy-owned
    /// execution/reference client set could not be derived or validated
    /// against `[clients]`.
    LiveTransportScope {
        reason: String,
    },
    /// The validated live canary gate report could not arm the shared
    /// submit-admission state before `LiveNode::run` was invoked.
    SubmitAdmission(BoltV3SubmitAdmissionError),
    /// NT returned an error from `LiveNode::run` after the live canary
    /// gate accepted the loaded config and readiness report.
    Run(anyhow::Error),
    /// NT runtime capture could not be wired from the validated
    /// bolt-v3 `[persistence]` config before the runner loop started.
    RuntimeCaptureWire(anyhow::Error),
    /// NT runtime capture failed during shutdown after the runner loop
    /// exited or after the capture worker asked the LiveNode to stop.
    RuntimeCaptureShutdown(anyhow::Error),
    /// The live runner exited without admitting a live order after the
    /// one-time approval had already been consumed. A blocked canary
    /// evidence artifact was written so the consumed approval has a
    /// source-owned terminal record instead of disappearing into logs.
    BlockedBeforeSubmit {
        canary_evidence_path: String,
    },
    /// The live runner exited without admitting a live order, but the
    /// terminal blocked canary evidence artifact could not be written.
    CanaryEvidenceWrite(anyhow::Error),
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
    NoSubmitStartTimeout {
        timeout_secs: u64,
    },
    NoSubmitStartTimeoutOverflow,
    NoSubmitStartIncomplete,
    NoSubmitExecutionAccountsMissing {
        client_venues: Vec<String>,
    },
    NoSubmitReferenceProbeSetup(anyhow::Error),
    NoSubmitReferenceProbeFailed {
        reason: String,
    },
    NoSubmitDataClientProbeFailed {
        reason: String,
    },
    NoSubmitStartFailed(anyhow::Error),
    NoSubmitStopTimeout {
        timeout_secs: u64,
    },
    NoSubmitStopTimeoutOverflow,
    NoSubmitStopFailed(anyhow::Error),
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
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
            BoltV3LiveNodeError::LiveCanaryGate(error) => {
                write!(
                    f,
                    "bolt-v3 live canary gate rejected runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => {
                write!(
                    f,
                    "bolt-v3 live canary approval consumption failed before runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::LiveTransportScope { reason } => write!(
                f,
                "bolt-v3 live transport scope could not be derived from strategy-owned client bindings: {reason}"
            ),
            BoltV3LiveNodeError::SubmitAdmission(error) => {
                write!(
                    f,
                    "bolt-v3 submit admission rejected runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::Run(error) => write!(f, "LiveNode run failed: {error}"),
            BoltV3LiveNodeError::RuntimeCaptureWire(error) => {
                write!(f, "NT runtime capture wiring failed: {error}")
            }
            BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => {
                write!(f, "NT runtime capture shutdown failed: {error}")
            }
            BoltV3LiveNodeError::BlockedBeforeSubmit {
                canary_evidence_path,
            } => write!(
                f,
                "bolt-v3 live canary blocked before submit; canary evidence written to {canary_evidence_path}"
            ),
            BoltV3LiveNodeError::CanaryEvidenceWrite(error) => {
                write!(f, "bolt-v3 live canary evidence write failed: {error}")
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
            BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 no-submit controlled-start exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow => write!(
                f,
                "bolt-v3 no-submit controlled-start timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::NoSubmitStartIncomplete => write!(
                f,
                "bolt-v3 no-submit controlled-run exited before NT reached Running \
                 with required startup evidence"
            ),
            BoltV3LiveNodeError::NoSubmitExecutionAccountsMissing { client_venues } => write!(
                f,
                "bolt-v3 no-submit controlled-run reached NT Running but required execution \
                 account evidence was absent from NT cache for: {}",
                client_venues.join(", ")
            ),
            BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(error) => write!(
                f,
                "bolt-v3 no-submit reference quote probe setup failed: {error}"
            ),
            BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { reason } => write!(
                f,
                "bolt-v3 no-submit controlled-run reached NT Running but live reference quote evidence was not observed; engine connectivity cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::NoSubmitDataClientProbeFailed { reason } => write!(
                f,
                "bolt-v3 no-submit controlled-run reached NT Running but data-client readiness evidence was not observed; data-client production readiness cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::NoSubmitStartFailed(error) => {
                write!(f, "bolt-v3 no-submit controlled-start failed: {error}")
            }
            BoltV3LiveNodeError::NoSubmitStopTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 no-submit controlled-stop exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow => write!(
                f,
                "bolt-v3 no-submit controlled-stop timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::NoSubmitStopFailed(error) => {
                write!(f, "bolt-v3 no-submit controlled-stop failed: {error}")
            }
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
            BoltV3LiveNodeError::Build(error) => error.source(),
            BoltV3LiveNodeError::LiveCanaryGate(error) => Some(error),
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::SubmitAdmission(error) => Some(error),
            BoltV3LiveNodeError::Run(error) => error.source(),
            BoltV3LiveNodeError::RuntimeCaptureWire(error)
            | BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => error.source(),
            BoltV3LiveNodeError::CanaryEvidenceWrite(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown { run_error, .. } => {
                Some(run_error.as_ref())
            }
            BoltV3LiveNodeError::ConnectTimeout { .. }
            | BoltV3LiveNodeError::ConnectIncomplete
            | BoltV3LiveNodeError::DisconnectTimeout { .. }
            | BoltV3LiveNodeError::LiveTransportScope { .. }
            | BoltV3LiveNodeError::BlockedBeforeSubmit { .. }
            | BoltV3LiveNodeError::NoSubmitStartTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow
            | BoltV3LiveNodeError::NoSubmitStartIncomplete
            | BoltV3LiveNodeError::NoSubmitExecutionAccountsMissing { .. }
            | BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { .. }
            | BoltV3LiveNodeError::NoSubmitDataClientProbeFailed { .. }
            | BoltV3LiveNodeError::NoSubmitStopTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow => None,
            BoltV3LiveNodeError::DisconnectFailed(error)
            | BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(error)
            | BoltV3LiveNodeError::NoSubmitStartFailed(error)
            | BoltV3LiveNodeError::NoSubmitStopFailed(error) => Some(error.as_ref()),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded = trade_transport_loaded_config(loaded)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    let adapters = map_bolt_v3_adapters(&transport_loaded, &resolved)
        .map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let (runtime, _summary) = build_live_node_with_clients(&transport_loaded, &resolved, adapters)?;
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

pub fn build_bolt_v3_no_submit_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded = trade_transport_loaded_config(loaded)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    let adapters = no_submit_transport_adapter_configs(&transport_loaded, &resolved)?;
    let no_submit_loaded = no_submit_transport_loaded_config(&transport_loaded);
    let (runtime, _summary) = build_live_node_with_clients(&no_submit_loaded, &resolved, adapters)?;
    Ok(runtime)
}

pub fn build_bolt_v3_no_submit_data_client_probe_live_node(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<(BoltV3LiveNodeRuntime, LoadedBoltV3Config), BoltV3LiveNodeError> {
    let probe_loaded = data_client_probe_loaded_config(loaded, client_key)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&probe_loaded)?;
    let adapters = no_submit_transport_adapter_configs(&probe_loaded, &resolved)?;
    let no_submit_loaded = no_submit_transport_loaded_config(&probe_loaded);
    let (runtime, _summary) = build_live_node_with_clients(&no_submit_loaded, &resolved, adapters)?;
    Ok((runtime, no_submit_loaded))
}

pub fn build_bolt_v3_all_configured_client_mapping_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let resolved = resolve_bolt_v3_live_node_secrets(loaded)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let mapping_loaded = no_submit_transport_loaded_config(loaded);
    let (runtime, _summary) = build_live_node_with_clients(&mapping_loaded, &resolved, adapters)?;
    Ok(runtime)
}

fn no_submit_transport_adapter_configs(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3LiveNodeError> {
    map_bolt_v3_adapters(loaded, resolved).map_err(BoltV3LiveNodeError::AdapterMapping)
}

fn trade_transport_loaded_config(
    loaded: &LoadedBoltV3Config,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    if loaded.strategies.is_empty() {
        return Ok(loaded.clone());
    }

    let required_clients = trade_transport_client_keys(loaded);
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
    }
    client_keys
}

fn data_client_probe_loaded_config(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    if client_key.trim().is_empty() {
        return Err(BoltV3LiveNodeError::NoSubmitDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        });
    }
    let client = loaded
        .root
        .clients
        .get(client_key)
        .cloned()
        .ok_or_else(|| BoltV3LiveNodeError::NoSubmitDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::NoSubmitDataClientProbeFailed {
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

fn no_submit_transport_loaded_config(loaded: &LoadedBoltV3Config) -> LoadedBoltV3Config {
    let mut no_submit_loaded = loaded.clone();
    no_submit_loaded.strategies.clear();
    no_submit_loaded
}

/// Single bolt-v3 entrypoint for entering NT's runner loop.
///
/// The caller builds the `LiveNode` separately, then this function checks
/// the loaded config's `[live_canary]` section and referenced no-submit
/// readiness report before entering the NT runner loop. Production callers
/// must use this wrapper rather than invoking the NT runner method directly.
/// If the gate rejects, NT's runner loop is never entered.
pub async fn run_bolt_v3_live_node(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let gate_report = check_bolt_v3_live_canary_pre_consumption_gate(loaded)
        .await
        .map_err(BoltV3LiveNodeError::LiveCanaryGate)?;
    consume_bolt_v3_live_runner_approval(loaded)
        .map_err(BoltV3LiveNodeError::OperatorApprovalConsumption)?;
    runtime
        .submit_admission
        .arm(gate_report)
        .map_err(BoltV3LiveNodeError::SubmitAdmission)?;
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

    let run_classification =
        classify_live_node_run_and_capture_shutdown(run_result, shutdown_result);
    if run_classification.is_ok() && runtime.admitted_order_count() == 0 {
        let canary_evidence_path =
            write_bolt_v3_blocked_before_submit_canary_evidence(loaded, &runtime.instance_id())
                .map_err(BoltV3LiveNodeError::CanaryEvidenceWrite)?;
        return Err(BoltV3LiveNodeError::BlockedBeforeSubmit {
            canary_evidence_path,
        });
    }
    run_classification
}

pub fn consume_bolt_v3_live_runner_approval(
    loaded: &LoadedBoltV3Config,
) -> Result<(), anyhow::Error> {
    let current_head_sha = current_build_head_sha()
        .ok_or_else(|| anyhow::anyhow!("bolt-v3 build head_sha is unavailable or invalid"))?;
    let (envelope, current_root_toml_sha256) = phase8_live_runner_approval_envelope(loaded)?;
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing `[live_canary]` config"))?;
    let current_unix_secs = current_unix_seconds_i64()?;

    envelope.validate_and_consume_against(
        current_head_sha,
        &current_root_toml_sha256,
        &live_canary.approval_id,
        loaded,
        current_unix_secs,
    )
}

fn write_bolt_v3_blocked_before_submit_canary_evidence(
    loaded: &LoadedBoltV3Config,
    run_id: &str,
) -> Result<String, anyhow::Error> {
    let (envelope, current_root_toml_sha256) = phase8_live_runner_approval_envelope(loaded)?;
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing `[live_canary]` config"))?;
    let max_notional_per_order = Decimal::from_str_exact(live_canary.max_notional_per_order.trim())
        .map_err(|source| {
            anyhow::anyhow!("[live_canary].max_notional_per_order is not a valid decimal: {source}")
        })?;
    let input = Phase8CanaryEvidenceInput {
        head_sha: envelope.head_sha.clone(),
        root_config_sha256: current_root_toml_sha256,
        ssm_manifest_sha256: envelope.ssm_manifest_sha256.clone(),
        ssm_manifest_ref: Phase8EvidenceRef {
            path_hash: phase8_sha256_text(&envelope.ssm_manifest_path),
            record_hash: envelope.ssm_manifest_sha256.clone(),
        },
        strategy_input_evidence_ref: Phase8EvidenceRef {
            path_hash: phase8_sha256_text(&envelope.strategy_input_evidence_path),
            record_hash: envelope.strategy_input_evidence_sha256.clone(),
        },
        approved_strategy_instance_id_hash: envelope.approved_strategy_instance_id_hash()?,
        approval_id: live_canary.approval_id.clone(),
        max_live_order_count: live_canary.max_live_order_count,
        max_notional_per_order,
        runtime_capture_ref: Phase8RuntimeCaptureRef {
            spool_root_hash: phase8_sha256_text(&loaded.root.persistence.catalog_directory),
            run_id: run_id.to_string(),
        },
    };
    let evidence = Phase8CanaryEvidence::blocked_before_submit(
        input,
        vec![Phase8CanaryBlockReason::RuntimeNoAdmittedOrder],
    );
    evidence.write_json_file(&envelope.canary_evidence_path)?;
    Ok(envelope.canary_evidence_path)
}

fn phase8_live_runner_approval_envelope(
    loaded: &LoadedBoltV3Config,
) -> Result<(Phase8OperatorApprovalEnvelope, String), anyhow::Error> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing `[live_canary]` config"))?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing `[live_canary.operator_evidence]` config"))?;
    let current_root_toml_sha256 = Phase8OperatorApprovalEnvelope::sha256_file(&loaded.root_path)?;
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: operator_evidence.head_sha.clone(),
        root_toml_path: loaded.root_path.to_string_lossy().to_string(),
        root_toml_sha256: current_root_toml_sha256.clone(),
        approval_envelope_sha256: operator_evidence.approval_envelope_sha256.clone(),
        ssm_manifest_path: operator_evidence.ssm_manifest_path.clone(),
        ssm_manifest_sha256: operator_evidence.ssm_manifest_sha256.clone(),
        strategy_input_evidence_path: operator_evidence.strategy_input_evidence_path.clone(),
        strategy_input_evidence_sha256: operator_evidence.strategy_input_evidence_sha256.clone(),
        financial_envelope_path: operator_evidence.financial_envelope_path.clone(),
        financial_envelope_sha256: operator_evidence.financial_envelope_sha256.clone(),
        pre_run_state_path: operator_evidence.pre_run_state_path.clone(),
        pre_run_state_sha256: operator_evidence.pre_run_state_sha256.clone(),
        abort_plan_path: operator_evidence.abort_plan_path.clone(),
        abort_plan_sha256: operator_evidence.abort_plan_sha256.clone(),
        operator_approval_id: live_canary.approval_id.clone(),
        approval_not_before_unix_secs: operator_evidence.approval_not_before_unix_seconds,
        approval_not_after_unix_secs: operator_evidence.approval_not_after_unix_seconds,
        approval_nonce_path: operator_evidence.approval_nonce_path.clone(),
        approval_nonce_sha256: operator_evidence.approval_nonce_sha256.clone(),
        approval_consumption_path: operator_evidence.approval_consumption_path.clone(),
        canary_evidence_path: operator_evidence.canary_evidence_path.clone(),
        strategy_cancel_path: operator_evidence.strategy_cancel_path.clone(),
    };
    Ok((envelope, current_root_toml_sha256))
}

fn current_unix_seconds_i64() -> Result<i64, anyhow::Error> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| anyhow::anyhow!("system time is before UNIX_EPOCH: {source}"))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|source| anyhow::anyhow!("current unix seconds exceeds i64: {source}"))
}

fn current_unix_nanos() -> Result<u64, anyhow::Error> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| anyhow::anyhow!("system time is before UNIX_EPOCH: {source}"))?
        .as_nanos();
    u64::try_from(nanos)
        .map_err(|source| anyhow::anyhow!("current unix nanoseconds exceeds u64: {source}"))
}

pub async fn controlled_no_submit_readiness<F>(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    mut reference_readiness: F,
) -> (
    Result<(), BoltV3LiveNodeError>,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
)
where
    F: FnMut(&BoltV3LiveNodeRuntime, &BoltV3NoSubmitReferenceQuoteEvidence) -> Result<(), String>,
{
    let (run, reference_quote_evidence, reference_quote_probe, stop) =
        run_bolt_v3_no_submit_readiness_until_observed(&mut runtime.node, loaded).await;
    let execution_accounts = match run {
        Ok(()) => no_submit_required_execution_accounts_registered(runtime, loaded),
        Err(error) => Err(error),
    };
    let connect = no_submit_controlled_connect_result(execution_accounts, &reference_quote_probe);
    let reference = if connect.is_ok() {
        reference_quote_probe.and_then(|()| reference_readiness(runtime, &reference_quote_evidence))
    } else {
        Err("controlled connect failed".to_string())
    };
    (connect, reference, stop)
}

pub async fn collect_no_submit_reference_quote_evidence(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReferenceQuoteEvidence, BoltV3LiveNodeError> {
    let (run, reference_quote_evidence, reference_quote_probe, stop) =
        run_bolt_v3_no_submit_readiness_until_observed(&mut runtime.node, loaded).await;
    run?;
    no_submit_required_execution_accounts_registered(runtime, loaded)?;
    if let Err(reason) = reference_quote_probe {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { reason });
    }
    stop?;
    Ok(reference_quote_evidence)
}

pub async fn collect_no_submit_data_client_readiness_quote_evidence(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3NoSubmitReferenceQuoteEvidence, BoltV3LiveNodeError> {
    let (run, reference_quote_evidence, reference_quote_probe, stop) =
        run_bolt_v3_no_submit_readiness_until_data_client_probe_observed(
            &mut runtime.node,
            loaded,
            client_key,
        )
        .await;
    run?;
    no_submit_required_execution_accounts_registered(runtime, loaded)?;
    if let Err(reason) = reference_quote_probe {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { reason });
    }
    stop?;
    Ok(reference_quote_evidence)
}

pub async fn collect_no_submit_data_client_readiness_evidence(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3NoSubmitDataClientReadinessEvidence, BoltV3LiveNodeError> {
    let (
        run,
        metadata_evidence,
        quote_evidence,
        book_evidence,
        metadata_probe,
        reference_quote_probe,
        stop,
    ) = run_bolt_v3_no_submit_readiness_until_data_client_readiness_observed(
        &mut runtime.node,
        loaded,
        client_key,
    )
    .await;
    run?;
    no_submit_required_execution_accounts_registered(runtime, loaded)?;
    if let Err(reason) = metadata_probe {
        return Err(BoltV3LiveNodeError::NoSubmitDataClientProbeFailed { reason });
    }
    if let Err(reason) = reference_quote_probe {
        return Err(BoltV3LiveNodeError::NoSubmitDataClientProbeFailed { reason });
    }
    stop?;
    Ok(BoltV3NoSubmitDataClientReadinessEvidence {
        metadata: metadata_evidence,
        quotes: quote_evidence,
        books: book_evidence,
    })
}

pub async fn collect_no_submit_data_client_metadata_evidence(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3NoSubmitDataClientMetadataEvidence, BoltV3LiveNodeError> {
    let (
        run,
        metadata_evidence,
        _quote_evidence,
        _book_evidence,
        metadata_probe,
        _reference_quote_probe,
        stop,
    ) = run_bolt_v3_no_submit_readiness_until_data_client_readiness_observed(
        &mut runtime.node,
        loaded,
        client_key,
    )
    .await;
    run?;
    no_submit_required_execution_accounts_registered(runtime, loaded)?;
    if let Err(reason) = metadata_probe {
        return Err(BoltV3LiveNodeError::NoSubmitDataClientProbeFailed { reason });
    }
    stop?;
    Ok(metadata_evidence)
}

fn no_submit_controlled_connect_result(
    execution_accounts: Result<(), BoltV3LiveNodeError>,
    reference_quote_probe: &Result<(), String>,
) -> Result<(), BoltV3LiveNodeError> {
    execution_accounts?;
    match reference_quote_probe {
        Ok(()) => Ok(()),
        Err(reason) => Err(BoltV3LiveNodeError::NoSubmitReferenceProbeFailed {
            reason: reason.clone(),
        }),
    }
}

async fn run_bolt_v3_no_submit_readiness_until_observed(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> (
    Result<(), BoltV3LiveNodeError>,
    BoltV3NoSubmitReferenceQuoteEvidence,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
) {
    let reference_quote_probe = install_no_submit_reference_quote_probe(node, loaded);
    run_bolt_v3_no_submit_readiness_with_reference_quote_probe(
        node,
        loaded,
        reference_quote_probe,
        "configured reference_data quotes",
    )
    .await
}

async fn run_bolt_v3_no_submit_readiness_until_data_client_probe_observed(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> (
    Result<(), BoltV3LiveNodeError>,
    BoltV3NoSubmitReferenceQuoteEvidence,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
) {
    let reference_quote_probe =
        install_no_submit_data_client_readiness_quote_probe(node, loaded, client_key);
    run_bolt_v3_no_submit_readiness_with_reference_quote_probe(
        node,
        loaded,
        reference_quote_probe,
        "configured client readiness_probe.quote_targets quotes",
    )
    .await
}

async fn run_bolt_v3_no_submit_readiness_until_data_client_readiness_observed(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> (
    Result<(), BoltV3LiveNodeError>,
    BoltV3NoSubmitDataClientMetadataEvidence,
    BoltV3NoSubmitReferenceQuoteEvidence,
    BoltV3NoSubmitBookDeltasEvidence,
    Result<(), String>,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
) {
    let probe = install_no_submit_data_client_readiness_probe(node, loaded, client_key);
    let (metadata_probe, reference_quote_probe) = match probe {
        Ok(probe) => probe,
        Err(error) => {
            return (
                Err(error),
                BoltV3NoSubmitDataClientMetadataEvidence {
                    responses: Vec::new(),
                },
                BoltV3NoSubmitReferenceQuoteEvidence { quotes: Vec::new() },
                BoltV3NoSubmitBookDeltasEvidence { deltas: Vec::new() },
                Err("data-client metadata probe setup failed".to_string()),
                Err("data-client quote probe setup failed".to_string()),
                Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                    "no-submit runner was not started because data-client readiness probe setup failed"
                ))),
            );
        }
    };
    let timeout_secs = match no_submit_start_timeout_secs(loaded) {
        Ok(timeout_secs) => timeout_secs,
        Err(error) => {
            return (
                Err(error),
                metadata_probe.evidence(),
                reference_quote_probe.evidence(),
                reference_quote_probe.book_evidence(),
                Err(
                    "data-client metadata probe was not observed because start timeout overflowed"
                        .to_string(),
                ),
                Err(
                    "data-client quote probe was not observed because start timeout overflowed"
                        .to_string(),
                ),
                Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                    "no-submit runner was not started because the configured start timeout overflowed"
                ))),
            );
        }
    };
    let stop_timeout_secs = match no_submit_stop_timeout_secs(loaded) {
        Ok(timeout_secs) => timeout_secs,
        Err(_) => {
            return (
                Err(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
                metadata_probe.evidence(),
                reference_quote_probe.evidence(),
                reference_quote_probe.book_evidence(),
                Err(
                    "data-client metadata probe was not observed because stop timeout overflowed"
                        .to_string(),
                ),
                Err(
                    "data-client quote probe was not observed because stop timeout overflowed"
                        .to_string(),
                ),
                Err(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
            );
        }
    };
    let node_handle = node.handle();
    let run_future = node.run();
    tokio::pin!(run_future);

    let connect = tokio::select! {
        result = &mut run_future => {
            let connect = match result {
                Ok(()) => Err(BoltV3LiveNodeError::NoSubmitStartIncomplete),
                Err(error) => Err(BoltV3LiveNodeError::NoSubmitStartFailed(error)),
            };
            return (
                connect,
                metadata_probe.evidence(),
                reference_quote_probe.evidence(),
                reference_quote_probe.book_evidence(),
                Err("data-client metadata probe was not observed before runner exit".to_string()),
                Err("data-client quote probe was not observed before runner exit".to_string()),
                Ok(()),
            );
        }
        result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            await_no_submit_running(&node_handle, loaded),
        ) => {
            match result {
                Ok(()) => Ok(()),
                Err(_) => {
                    return (
                        Err(BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_secs }),
                        metadata_probe.evidence(),
                        reference_quote_probe.evidence(),
                        reference_quote_probe.book_evidence(),
                        Err("data-client metadata probe was not observed because no-submit runner did not reach Running".to_string()),
                        Err("data-client quote probe was not observed because no-submit runner did not reach Running".to_string()),
                        Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                            "no-submit runner did not reach Running before the configured start timeout; NT does not observe stop signals during startup"
                        ))),
                    );
                }
            }
        }
    };

    let mut metadata_probe_result = None;
    let mut reference_probe_result = None;
    let metadata_future = await_no_submit_data_client_metadata_probe(&metadata_probe, loaded);
    let reference_future = await_no_submit_reference_quote_probe(
        &reference_quote_probe,
        loaded,
        "configured client readiness_probe.quote_targets quotes",
    );
    tokio::pin!(metadata_future);
    tokio::pin!(reference_future);
    while metadata_probe_result.is_none() || reference_probe_result.is_none() {
        tokio::select! {
            result = &mut run_future => {
                let stop = match result {
                    Ok(()) => Ok(()),
                    Err(error) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
                };
                return (
                    connect,
                    metadata_probe.evidence(),
                    reference_quote_probe.evidence(),
                    reference_quote_probe.book_evidence(),
                    metadata_probe_result.unwrap_or_else(|| {
                        Err("data-client metadata probe was not observed before runner exit".to_string())
                    }),
                    reference_probe_result.unwrap_or_else(|| {
                        Err("data-client quote probe was not observed before runner exit".to_string())
                    }),
                    stop,
                );
            }
            result = &mut metadata_future, if metadata_probe_result.is_none() => {
                metadata_probe_result = Some(result);
            }
            result = &mut reference_future, if reference_probe_result.is_none() => {
                reference_probe_result = Some(result);
            }
        }
    }
    let metadata_evidence = metadata_probe.evidence();
    let reference_quote_evidence = reference_quote_probe.evidence();
    let book_delta_evidence = reference_quote_probe.book_evidence();
    node_handle.stop();
    let stop =
        match tokio::time::timeout(Duration::from_secs(stop_timeout_secs), &mut run_future).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
            Err(_) => Err(BoltV3LiveNodeError::NoSubmitStopTimeout {
                timeout_secs: stop_timeout_secs,
            }),
        };
    (
        connect,
        metadata_evidence,
        reference_quote_evidence,
        book_delta_evidence,
        metadata_probe_result
            .unwrap_or_else(|| Err("data-client metadata probe was not observed".to_string())),
        reference_probe_result
            .unwrap_or_else(|| Err("data-client quote probe was not observed".to_string())),
        stop,
    )
}

async fn run_bolt_v3_no_submit_readiness_with_reference_quote_probe(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    reference_quote_probe: Result<BoltV3NoSubmitReferenceQuoteProbeHandle, BoltV3LiveNodeError>,
    configured_targets_label: &'static str,
) -> (
    Result<(), BoltV3LiveNodeError>,
    BoltV3NoSubmitReferenceQuoteEvidence,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
) {
    let reference_quote_probe = match reference_quote_probe {
        Ok(probe) => probe,
        Err(error) => {
            return (
                Err(error),
                BoltV3NoSubmitReferenceQuoteEvidence { quotes: Vec::new() },
                Err("reference quote probe setup failed".to_string()),
                Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                    "no-submit runner was not started because reference quote probe setup failed"
                ))),
            );
        }
    };
    let timeout_secs = match no_submit_start_timeout_secs(loaded) {
        Ok(timeout_secs) => timeout_secs,
        Err(error) => {
            return (
                Err(error),
                reference_quote_probe.evidence(),
                Err(
                    "reference quote probe was not observed because start timeout overflowed"
                        .to_string(),
                ),
                Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                    "no-submit runner was not started because the configured start timeout overflowed"
                ))),
            );
        }
    };
    let stop_timeout_secs = match no_submit_stop_timeout_secs(loaded) {
        Ok(timeout_secs) => timeout_secs,
        Err(_) => {
            return (
                Err(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
                reference_quote_probe.evidence(),
                Err(
                    "reference quote probe was not observed because stop timeout overflowed"
                        .to_string(),
                ),
                Err(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
            );
        }
    };
    let node_handle = node.handle();
    let run_future = node.run();
    tokio::pin!(run_future);

    let connect = tokio::select! {
        result = &mut run_future => {
            let connect = match result {
                Ok(()) => Err(BoltV3LiveNodeError::NoSubmitStartIncomplete),
                Err(error) => Err(BoltV3LiveNodeError::NoSubmitStartFailed(error)),
            };
            return (
                connect,
                reference_quote_probe.evidence(),
                Err("reference quote probe was not observed before runner exit".to_string()),
                Ok(()),
            );
        }
        result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            await_no_submit_running(&node_handle, loaded),
        ) => {
            match result {
                Ok(()) => Ok(()),
                Err(_) => {
                    return (
                        Err(BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_secs }),
                        reference_quote_probe.evidence(),
                        Err("reference quote probe was not observed because no-submit runner did not reach Running".to_string()),
                        Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                            "no-submit runner did not reach Running before the configured start timeout; NT does not observe stop signals during startup"
                        ))),
                    );
                }
            }
        }
    };

    let reference_probe = tokio::select! {
        result = &mut run_future => {
            let stop = match result {
                Ok(()) => Ok(()),
                Err(error) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
            };
            return (
                connect,
                reference_quote_probe.evidence(),
                Err("reference quote probe was not observed before runner exit".to_string()),
                stop,
            );
        }
        result = await_no_submit_reference_quote_probe(
            &reference_quote_probe,
            loaded,
            configured_targets_label,
        ) => result,
    };
    let reference_quote_evidence = reference_quote_probe.evidence();
    node_handle.stop();
    let stop =
        match tokio::time::timeout(Duration::from_secs(stop_timeout_secs), &mut run_future).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
            Err(_) => Err(BoltV3LiveNodeError::NoSubmitStopTimeout {
                timeout_secs: stop_timeout_secs,
            }),
        };
    (connect, reference_quote_evidence, reference_probe, stop)
}

fn install_no_submit_reference_quote_probe(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
    let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(loaded);
    install_no_submit_reference_quote_probe_handle(node, loaded, handle)
}

fn install_no_submit_data_client_readiness_quote_probe(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3NoSubmitReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
    let (required, ambiguous_instrument_ids) =
        no_submit_data_client_readiness_quote_subscription_plan(loaded, client_key)?;
    let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::from_plan(
        required,
        ambiguous_instrument_ids,
        DataClientReadinessProbeMarketDataKind::Quote,
        None,
    );
    install_no_submit_reference_quote_probe_handle(node, loaded, handle)
}

fn install_no_submit_data_client_readiness_probe(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<
    (
        BoltV3NoSubmitDataClientMetadataProbeHandle,
        BoltV3NoSubmitReferenceQuoteProbeHandle,
    ),
    BoltV3LiveNodeError,
> {
    let metadata_handle = BoltV3NoSubmitDataClientMetadataProbeHandle::new(loaded, client_key)?;
    let quote_handle = no_submit_data_client_readiness_quote_probe_handle(loaded, client_key)?;
    if let Some(message) = quote_handle.ambiguity_error() {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(message),
        ));
    }
    let config = no_submit_reference_quote_probe_config(loaded)?;
    node.add_actor(BoltV3NoSubmitDataClientReadinessProbe::new(
        metadata_handle.clone(),
        quote_handle.clone(),
        config,
    ))
    .map_err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup)?;
    Ok((metadata_handle, quote_handle))
}

fn install_no_submit_reference_quote_probe_handle(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
    handle: BoltV3NoSubmitReferenceQuoteProbeHandle,
) -> Result<BoltV3NoSubmitReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
    if let Some(message) = handle.ambiguity_error() {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(message),
        ));
    }
    let config = no_submit_reference_quote_probe_config(loaded)?;
    node.add_actor(BoltV3NoSubmitReferenceQuoteProbe::new(
        handle.clone(),
        config,
    ))
    .map_err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup)?;
    Ok(handle)
}

fn no_submit_reference_quote_probe_config(
    loaded: &LoadedBoltV3Config,
) -> Result<DataActorConfig, BoltV3LiveNodeError> {
    let live_canary = loaded.root.live_canary.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "bolt-v3 no-submit reference quote probe requires [live_canary]"
        ))
    })?;
    let actor_id_value = live_canary.reference_quote_probe_actor_id.as_str();
    if actor_id_value.trim().is_empty() || actor_id_value.trim() != actor_id_value {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "[live_canary].reference_quote_probe_actor_id must be non-empty without surrounding whitespace"
            ),
        ));
    }
    let actor_id = ActorId::new_checked(actor_id_value).map_err(|error| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "[live_canary].reference_quote_probe_actor_id is invalid: {error}"
        ))
    })?;

    Ok(DataActorConfig {
        actor_id: Some(actor_id),
        log_events: live_canary.reference_quote_probe_log_events,
        log_commands: live_canary.reference_quote_probe_log_commands,
    })
}

async fn await_no_submit_data_client_metadata_probe(
    probe: &BoltV3NoSubmitDataClientMetadataProbeHandle,
    loaded: &LoadedBoltV3Config,
) -> Result<(), String> {
    let timeout_secs = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| "data-client metadata probe wait requires `[live_canary]`".to_string())?
        .reference_quote_wait_timeout_seconds;
    if timeout_secs == 0 {
        return Err(
            "[live_canary].reference_quote_wait_timeout_seconds must be a positive integer"
                .to_string(),
        );
    }
    tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        probe.wait_for_metadata_response().await;
    })
    .await
    .map_err(|_| {
        format!(
            "data-client metadata probe did not observe request_instruments response within [live_canary].reference_quote_wait_timeout_seconds={timeout_secs}"
        )
    })
}

async fn await_no_submit_reference_quote_probe(
    probe: &BoltV3NoSubmitReferenceQuoteProbeHandle,
    loaded: &LoadedBoltV3Config,
    configured_targets_label: &'static str,
) -> Result<(), String> {
    let timeout_secs = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| "reference quote probe wait requires `[live_canary]`".to_string())?
        .reference_quote_wait_timeout_seconds;
    if timeout_secs == 0 {
        return Err(
            "[live_canary].reference_quote_wait_timeout_seconds must be a positive integer"
                .to_string(),
        );
    }
    tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        probe.wait_for_all_required_quotes().await
    })
    .await
    .map_err(|_| {
        format!(
            "reference quote probe did not observe all {configured_targets_label} within [live_canary].reference_quote_wait_timeout_seconds={timeout_secs}"
        )
    })?
}

async fn await_no_submit_running(node_handle: &LiveNodeHandle, loaded: &LoadedBoltV3Config) {
    let poll_interval = Duration::from_millis(
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_ms,
    );
    while node_handle.state() != NodeState::Running {
        tokio::time::sleep(poll_interval).await;
    }
}

fn no_submit_required_execution_accounts_registered(
    runtime: &BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let cache = runtime.node.kernel().cache();
    let cache = cache.borrow();
    let client_venues = loaded
        .root
        .clients
        .iter()
        .filter(|(client_key, _)| {
            runtime
                .registration_summary()
                .clients
                .get(*client_key)
                .is_some_and(|registered| registered.execution)
        })
        .filter(|(_, client)| client.execution.is_some())
        .filter(|(_, client)| cache.account_for_venue(&client.venue).is_none())
        .map(|(client_key, client)| format!("clients.{client_key} ({})", client.venue))
        .collect::<Vec<_>>();

    if client_venues.is_empty() {
        Ok(())
    } else {
        Err(BoltV3LiveNodeError::NoSubmitExecutionAccountsMissing { client_venues })
    }
}

fn no_submit_start_timeout_secs(loaded: &LoadedBoltV3Config) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_connection_secs
        .checked_add(loaded.root.nautilus.timeout_reconciliation_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_portfolio_secs))
        .ok_or(BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow)
}

fn no_submit_stop_timeout_secs(loaded: &LoadedBoltV3Config) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .ok_or(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow)
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
    let adapters = map_bolt_v3_adapters(&transport_loaded, &resolved)
        .map_err(BoltV3LiveNodeError::AdapterMapping)?;
    build_live_node_with_clients(&transport_loaded, &resolved, adapters)
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
    let mapping_loaded = no_submit_transport_loaded_config(loaded);
    build_live_node_with_clients(&mapping_loaded, &resolved, adapters)
}

fn build_live_node_with_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    let proof_executor_enabled = loaded
        .root
        .live_canary
        .as_ref()
        .and_then(|live_canary| live_canary.proof_policy.as_ref())
        .is_some_and(|proof_policy| proof_policy.enabled);
    let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> =
        if loaded.strategies.is_empty() && !proof_executor_enabled {
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
        };
    let submit_admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(
        decision_evidence.clone(),
    ));
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
    register_canary_proof_executor_on_node(
        &mut node,
        loaded,
        decision_evidence,
        submit_admission.clone(),
    )
    .map_err(|error| {
        BoltV3LiveNodeError::StrategyRegistration(BoltV3StrategyRegistrationError::Evidence {
            message: error.to_string(),
        })
    })?;
    for strategy in &strategy_summary.registered {
        log::info!(
            "bolt-v3 registered strategy: strategy_instance_id={} strategy_archetype={} nt_strategy_id={}",
            strategy.strategy_instance_id,
            strategy.strategy_archetype.as_str(),
            strategy.registered_strategy_id
        );
    }
    Ok((
        BoltV3LiveNodeRuntime::new(
            node,
            summary.clone(),
            submit_admission,
            resolved.redaction_values(),
        ),
        summary,
    ))
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
        bypass: loaded.root.risk.nautilus.bypass,
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
    use crate::bolt_v3_config::{
        BoltV3RootConfig, DataClientReadinessProbeBlock, DataClientReadinessProbeBookType,
        DataClientReadinessProbeMarketDataKind, DataClientReadinessProbeQuoteTargetBlock,
        DataClientReadinessProbeQuoteTargetSource, ReferenceDataBlock,
    };
    use nautilus_model::data::{BookOrder, OrderBookDelta, OrderBookDeltas};
    use nautilus_model::enums::{BookAction, OrderSide};
    use nautilus_model::identifiers::TraderId;
    use nautilus_model::types::{Price, Quantity};

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
            quote_targets: Some(BTreeMap::from([(
                "configured_quote_probe".to_string(),
                DataClientReadinessProbeQuoteTargetBlock {
                    instrument_id: InstrumentId::from("REFERENCE.POLYMARKET"),
                },
            )])),
        });

        let (required, ambiguous) =
            no_submit_data_client_readiness_quote_subscription_plan(&loaded, "polymarket_main")
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
            quote_targets: None,
        });

        let handle = no_submit_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
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
                .push(BoltV3NoSubmitReferenceQuote {
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
            quote_targets: None,
        });

        let handle = no_submit_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
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
            quote_targets: None,
        });

        let handle = no_submit_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
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
            quote_targets: None,
        });

        let handle = no_submit_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
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
                .push(BoltV3NoSubmitReferenceQuote {
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
            .push(BoltV3NoSubmitReferenceQuote {
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
            .push(BoltV3NoSubmitReferenceQuote {
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
            quote_targets: None,
        });

        let handle = no_submit_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
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
    fn no_submit_quote_probe_captures_quotes_with_wall_clock_time() {
        let source = include_str!("bolt_v3_live_node.rs");
        let runtime_source = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("runtime source should precede tests");

        assert!(
            runtime_source.contains("current_unix_nanos()?"),
            "no-submit quote evidence must capture receive time from the process wall clock"
        );
        assert!(
            !runtime_source.contains(".record_quote(quote, self.timestamp_ns().as_u64())"),
            "actor timestamp is not strong enough evidence for quote receive time"
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
    fn no_submit_controlled_connect_rejects_unobserved_reference_probe() {
        let error = no_submit_controlled_connect_result(
            Ok(()),
            &Err("reference quote probe timed out".to_string()),
        )
        .expect_err("controlled connect must not be satisfied when reference quote evidence was not observed");

        match error {
            BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { reason } => {
                assert_eq!(reason, "reference quote probe timed out");
            }
            other => panic!("expected NoSubmitReferenceProbeFailed, got {other}"),
        }
    }

    #[test]
    fn no_submit_transport_config_preserves_identity_but_removes_strategy_instances() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        assert!(
            !loaded.strategies.is_empty(),
            "fixture must include strategy config to prove no-submit transport strips it"
        );

        let no_submit_loaded = no_submit_transport_loaded_config(&loaded);

        assert!(
            no_submit_loaded.strategies.is_empty(),
            "no-submit transport runtime must not register strategy actors"
        );
        assert_eq!(no_submit_loaded.root_path, loaded.root_path);
        assert_eq!(
            no_submit_loaded.config_bundle_checksum,
            loaded.config_bundle_checksum
        );
        assert_eq!(
            no_submit_loaded.root.strategy_files,
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

        let scoped = trade_transport_loaded_config(&loaded)
            .expect("strategy-bound transport scope should be derived from config");

        assert_eq!(scoped.root.clients.len(), 2);
        assert!(scoped.root.clients.contains_key("polymarket_main"));
        assert!(scoped.root.clients.contains_key("reference_data"));
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
        no_submit_transport_adapter_configs(
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
        let runtime_loaded = no_submit_transport_loaded_config(&probe_loaded);

        assert!(
            !probe_loaded.strategies.is_empty(),
            "probe adapter mapping input must keep strategies for provider-owned data filters"
        );
        assert!(
            runtime_loaded.strategies.is_empty(),
            "no-submit data-client probes must not register strategy actors"
        );
        assert_eq!(runtime_loaded.root.clients.len(), 1);
        assert!(runtime_loaded.root.clients.contains_key("polymarket_main"));
    }

    #[test]
    fn no_submit_adapter_mapping_preserves_strategy_derived_market_filters() {
        use crate::{
            bolt_v3_providers::{
                binance::ResolvedBoltV3BinanceSecrets, polymarket::ResolvedBoltV3PolymarketSecrets,
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
                private_key: "fixture-poly-private-key".to_string(),
                api_key: "fixture-poly-api-key".to_string(),
                api_secret: "fixture-poly-api-secret".to_string(),
                passphrase: "fixture-poly-passphrase".to_string(),
            }),
        );
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(ResolvedBoltV3BinanceSecrets {
                api_key: "fixture-binance-api-key".to_string(),
                api_secret: "fixture-binance-api-secret".to_string(),
            }),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let adapters = no_submit_transport_adapter_configs(&loaded, &resolved)
            .expect("no-submit adapter mapping should retain market identity filters");
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
            "no-submit adapter mapping must keep strategy-derived provider filters"
        );
        assert_eq!(
            data.filters[0]
                .market_slugs()
                .expect("no-submit data config must keep configured target slug filters")
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
        let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(&loaded);
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
        let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(&loaded);
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
        let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(&loaded);
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
    fn live_runner_zero_admission_writer_outputs_blocked_canary_evidence() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let mut loaded =
            crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new("config/root.toml"))
                .expect("root config should load");
        loaded.root.persistence.catalog_directory =
            temp.path().join("catalog").to_string_lossy().to_string();
        let canary_evidence_path = {
            let live_canary = loaded
                .root
                .live_canary
                .as_mut()
                .expect("root config should include live canary");
            live_canary.approval_id = "phase8-zero-admission-test".to_string();
            live_canary.max_live_order_count = 1;
            live_canary.max_notional_per_order = "1.00".to_string();
            let operator_evidence = live_canary
                .operator_evidence
                .as_mut()
                .expect("root config should include operator evidence");
            operator_evidence.head_sha = "1".repeat(40);
            operator_evidence.ssm_manifest_path = temp
                .path()
                .join("ssm-manifest.json")
                .to_string_lossy()
                .to_string();
            operator_evidence.ssm_manifest_sha256 =
                write_phase8_live_node_test_file(&operator_evidence.ssm_manifest_path, "{}");
            operator_evidence.strategy_input_evidence_path = temp
                .path()
                .join("strategy-input.json")
                .to_string_lossy()
                .to_string();
            operator_evidence.strategy_input_evidence_sha256 = write_phase8_live_node_test_file(
                &operator_evidence.strategy_input_evidence_path,
                "{}",
            );
            operator_evidence.financial_envelope_path = temp
                .path()
                .join("financial-envelope.json")
                .to_string_lossy()
                .to_string();
            operator_evidence.financial_envelope_sha256 = write_phase8_live_node_test_file(
                &operator_evidence.financial_envelope_path,
                r#"{
  "max_live_order_count": 1,
  "max_notional_per_order": "1.00",
  "strategy_instance_id": "bitcoin_updown_main",
  "oms_type": "netting",
  "execution_client_id": "polymarket_main",
  "configured_target_id": "btc_updown_5m",
  "target_kind": "rotating",
  "rotating_market_family": "updown",
  "underlying_asset": "BTC",
  "cadence_secs": 300,
  "cadence_slug_token": "5m",
  "market_selection_rule": "current",
  "retry_interval_secs": 1,
  "blocked_after_secs": 1,
  "price_to_beat_source": "gate_session:decision_reference",
  "edge_threshold_basis_points": 100,
  "order_notional_target": "1.00",
  "maximum_position_notional": "10.00",
  "book_impact_cap_bps": 50,
  "entry_side": "buy",
  "entry_position_side": "long",
  "entry_order_type": "limit",
  "entry_time_in_force": "fok",
  "entry_expire_time_unix_nanos": null,
  "entry_trigger_price": null,
  "entry_activation_price": null,
  "entry_trigger_type": null,
  "entry_trigger_instrument_id": null,
  "entry_trailing_offset": null,
  "entry_trailing_offset_type": null,
  "entry_is_post_only": false,
  "entry_is_reduce_only": false,
  "entry_is_quote_quantity": false,
  "exit_side": "sell",
  "exit_position_side": "long",
  "exit_order_type": "market",
  "exit_time_in_force": "ioc",
  "exit_expire_time_unix_nanos": null,
  "exit_trigger_price": null,
  "exit_activation_price": null,
  "exit_trigger_type": null,
  "exit_trigger_instrument_id": null,
  "exit_trailing_offset": null,
  "exit_trailing_offset_type": null,
  "exit_is_post_only": false,
  "exit_is_reduce_only": true,
  "exit_is_quote_quantity": false,
  "forced_exit_side": "sell",
  "forced_exit_position_side": "long",
  "forced_exit_order_type": "market",
  "forced_exit_time_in_force": "ioc",
  "forced_exit_expire_time_unix_nanos": null,
  "forced_exit_trigger_price": null,
  "forced_exit_activation_price": null,
  "forced_exit_trigger_type": null,
  "forced_exit_trigger_instrument_id": null,
  "forced_exit_trailing_offset": null,
  "forced_exit_trailing_offset_type": null,
  "forced_exit_is_post_only": false,
  "forced_exit_is_reduce_only": true,
  "forced_exit_is_quote_quantity": false
}"#,
            );
            operator_evidence.canary_evidence_path = temp
                .path()
                .join("phase8-canary-evidence.json")
                .to_string_lossy()
                .to_string();
            operator_evidence.canary_evidence_path.clone()
        };

        let written_path =
            write_bolt_v3_blocked_before_submit_canary_evidence(&loaded, "phase8-test-run")
                .expect("zero-admission writer should write blocked evidence");

        assert_eq!(written_path, canary_evidence_path);
        let evidence: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&canary_evidence_path).expect("canary evidence should be readable"),
        )
        .expect("canary evidence should parse");
        assert_eq!(evidence["outcome"], "blocked_before_submit");
        assert_eq!(
            evidence["block_reasons"],
            serde_json::json!(["runtime_no_admitted_order"])
        );
        assert_eq!(
            evidence["submit_admission_ref"]["reason"],
            "blocked_before_submit"
        );
        assert_eq!(evidence["submit_admission_ref"]["admitted_order_count"], 0);
        assert_eq!(evidence["runtime_capture_ref"]["run_id"], "phase8-test-run");
    }

    fn write_phase8_live_node_test_file(path: &str, contents: &str) -> String {
        write_phase8_live_node_test_bytes(path, contents.as_bytes())
    }

    fn write_phase8_live_node_test_bytes(path: &str, bytes: &[u8]) -> String {
        std::fs::write(path, bytes).expect("test evidence file should write");
        Phase8OperatorApprovalEnvelope::sha256_file(std::path::Path::new(path))
            .expect("test evidence sha256 should compute")
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
    fn no_submit_timeout_sums_fail_closed_on_overflow() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.timeout_connection_secs = u64::MAX;
        loaded.root.nautilus.timeout_reconciliation_secs = 1;
        let start_error = no_submit_start_timeout_secs(&loaded)
            .expect_err("no-submit start timeout overflow must fail closed");
        assert!(
            matches!(
                start_error,
                BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow
            ),
            "expected start timeout overflow rejection, got {start_error:?}"
        );

        loaded.root.nautilus.timeout_disconnection_secs = u64::MAX;
        loaded.root.nautilus.delay_post_stop_secs = 1;
        let stop_error = no_submit_stop_timeout_secs(&loaded)
            .expect_err("no-submit stop timeout overflow must fail closed");
        assert!(
            matches!(stop_error, BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
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
        assert_eq!(cfg.risk_engine.max_order_submit_rate, "100/00:00:01");
        assert_eq!(cfg.risk_engine.max_order_modify_rate, "100/00:00:01");
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
