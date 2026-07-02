use super::*;

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3StrategyFreeReferenceQuoteEvidence {
    pub quotes: Vec<BoltV3StrategyFreeReferenceQuote>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StrategyFreeBookDeltas {
    pub data_client_id: String,
    pub instrument_id: String,
    pub delta_count: u64,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
    pub captured_at_unix_nanos: u64,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StrategyFreeBookDeltasEvidence {
    pub deltas: Vec<BoltV3StrategyFreeBookDeltas>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3StrategyFreeTrade {
    pub data_client_id: String,
    pub instrument_id: String,
    pub price: f64,
    pub size: f64,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
    pub captured_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StrategyFreeReferenceQuoteSubscription {
    pub(super) data_client_id: ClientId,
    pub(super) instrument_id: InstrumentId,
}

pub(super) const METADATA_RESPONSE_EMPTY_TARGETS_FAILURE: &str =
    "metadata_response readiness probe produced no source-owned instrument targets";

/// Live state for a trade chunk-count readiness walk. The probe subscribes one
/// chunk of the instrument universe at a time (so it never holds more than
/// `chunk_size` channels at once, staying below the venue's silent delivery
/// ceiling), watches it for `chunk_observation_window_seconds`, then advances.
/// It passes as soon as `required_live_markets` (`m`) distinct markets have
/// traded, and fails closed once the whole universe has been walked without
/// reaching `m`. Interior mutability mirrors the surrounding handle: the actor
/// is single-threaded (`!Send`), so `Cell`/`RefCell` is sufficient.
#[cfg_attr(not(test), allow(dead_code))]
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
    /// Distinct markets that fired at least one trade across subscribed
    /// chunks. Instrument IDs are enough here because the chunk-count probe is
    /// scoped to one data client.
    fired_instrument_ids: RefCell<BTreeSet<String>>,
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
    pub(super) trades: Rc<RefCell<Vec<BoltV3StrategyFreeTrade>>>,
    pub(super) quote_notify: Rc<tokio::sync::Notify>,
    /// Present only for a trade chunk-count probe (`market_data_kind = "trade"`
    /// with `quote_target_source = "metadata_response"`); drives the chunked
    /// walk over the instrument universe instead of a fixed sampled target set.
    chunk_walk: Option<Rc<ChunkCountWalk>>,
}

impl BoltV3StrategyFreeReferenceQuoteProbeHandle {
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
            trades: Rc::new(RefCell::new(Vec::new())),
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
            trades: Rc::new(RefCell::new(Vec::new())),
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
            trades: Rc::new(RefCell::new(Vec::new())),
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
                fired_instrument_ids: RefCell::new(BTreeSet::new()),
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
    #[cfg(test)]
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
        walk.complete.set(false);
        walk.fired_instrument_ids.borrow_mut().clear();
        walk.started.set(true);
        self.quote_notify.notify_one();
    }

    /// Take the next chunk to subscribe, installing it as the probe's current
    /// `required` set so recorded trades match against it. Returns `None` once
    /// the universe is exhausted.
    #[cfg(test)]
    pub(super) fn chunk_count_next_chunk(
        &self,
    ) -> Option<Vec<StrategyFreeReferenceQuoteSubscription>> {
        let walk = self.chunk_walk.as_ref()?;
        let cursor = walk.cursor.get();
        let chunk = match walk.chunks.borrow().get(cursor).cloned() {
            Some(chunk) => chunk,
            None => {
                walk.complete.set(true);
                if !self.chunk_count_passed() {
                    self.fail_metadata_response_probe(format!(
                            "trade chunk-count readiness probe exhausted {} chunk(s) with {} distinct fired market(s), below required min_observed_targets={}",
                            walk.chunks.borrow().len(),
                            walk.fired_instrument_ids.borrow().len(),
                            walk.required_live_markets,
                        ));
                }
                return None;
            }
        };
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
    #[cfg(test)]
    pub(super) fn chunk_count_current_chunk(&self) -> Vec<StrategyFreeReferenceQuoteSubscription> {
        self.required.borrow().clone()
    }

    pub(super) fn chunk_count_passed(&self) -> bool {
        match &self.chunk_walk {
            Some(walk) => trade_chunk_count_probe_passed(
                walk.fired_instrument_ids.borrow().len(),
                walk.required_live_markets,
            ),
            None => false,
        }
    }

    #[cfg(test)]
    pub(super) fn chunk_walk_started(&self) -> bool {
        self.chunk_walk
            .as_ref()
            .is_some_and(|walk| walk.started.get())
    }

    /// `(number_of_chunks, per_chunk_window_seconds)` for sizing the overall
    /// walk timeout once the universe is known.
    #[cfg(test)]
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
                observed_required_book_delta_count(&required, &book_deltas) >= required_observations
            }
            DataClientReadinessProbeMarketDataKind::Trade => {
                let trades = self.trades.borrow();
                observed_required_trade_count(&required, &trades) >= required_observations
            }
        }
    }

    pub(super) fn required_market_data_count(&self) -> usize {
        if let Some(walk) = &self.chunk_walk {
            return walk.required_live_markets;
        }
        let required_len = self.required.borrow().len();
        if self.metadata_response_data_client_id.is_some() && required_len == 0 {
            return self
                .min_observed_targets
                .or(self.metadata_response_max_quote_targets)
                .unwrap_or(0);
        }
        self.required_observation_count(required_len)
    }

    pub(super) fn observed_market_data_count(&self) -> usize {
        if let Some(walk) = &self.chunk_walk {
            return walk.fired_instrument_ids.borrow().len();
        }
        let required = self.required.borrow();
        match self.market_data_kind {
            DataClientReadinessProbeMarketDataKind::Quote => {
                observed_required_quote_count(&required, &self.quotes.borrow())
            }
            DataClientReadinessProbeMarketDataKind::Book => {
                observed_required_book_delta_count(&required, &self.book_deltas.borrow())
            }
            DataClientReadinessProbeMarketDataKind::Trade => {
                observed_required_trade_count(&required, &self.trades.borrow())
            }
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

    pub(super) fn fail_late_metadata_response_instrument(&self, instrument_id: InstrumentId) {
        self.fail_metadata_response_probe(format!(
            "metadata_response published source-owned instrument {instrument_id} after readiness subscriptions were installed; metadata_response readiness requires startup metadata to be complete before LiveNodeHandle::is_running()"
        ));
    }

    #[cfg(test)]
    pub(super) fn evidence(&self) -> BoltV3StrategyFreeReferenceQuoteEvidence {
        BoltV3StrategyFreeReferenceQuoteEvidence {
            quotes: self.quotes.borrow().clone(),
        }
    }

    #[cfg(test)]
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
        if metadata_quote_targets == 0 {
            self.fail_metadata_response_probe(METADATA_RESPONSE_EMPTY_TARGETS_FAILURE.to_string());
            return Vec::new();
        }
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

    pub(super) fn record_book_deltas(&self, deltas: &OrderBookDeltas, captured_at_unix_nanos: u64) {
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

    pub(super) fn record_trade(&self, trade: &TradeTick) {
        if self.market_data_kind != DataClientReadinessProbeMarketDataKind::Trade {
            return;
        }
        if let Some(walk) = &self.chunk_walk {
            if walk.complete.get() {
                return;
            }
            if self
                .required
                .borrow()
                .iter()
                .any(|required| trade.instrument_id == required.instrument_id)
            {
                walk.fired_instrument_ids
                    .borrow_mut()
                    .insert(trade.instrument_id.to_string());
                if self.chunk_count_passed() {
                    walk.complete.set(true);
                    self.quote_notify.notify_one();
                }
            }
            return;
        }
        let trade_instrument_id = trade.instrument_id.to_string();
        if self
            .ambiguous_instrument_ids
            .borrow()
            .contains(&trade_instrument_id)
        {
            return;
        }
        let mut matched_required = false;
        {
            let required = self.required.borrow();
            let mut trades = self.trades.borrow_mut();
            for required in required.iter() {
                if trade.instrument_id == required.instrument_id {
                    matched_required = true;
                    trades.push(BoltV3StrategyFreeTrade {
                        data_client_id: required.data_client_id.to_string(),
                        instrument_id: required.instrument_id.to_string(),
                        price: trade.price.as_f64(),
                        size: trade.size.as_f64(),
                        ts_event_unix_nanos: trade.ts_event.as_u64(),
                        ts_init_unix_nanos: trade.ts_init.as_u64(),
                        captured_at_unix_nanos: get_atomic_clock_realtime().get_time_ns().as_u64(),
                    });
                }
            }
        }
        if matched_required && self.has_all_required_market_data() {
            self.quote_notify.notify_one();
        }
    }

    #[cfg(test)]
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
#[cfg(test)]
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

fn observed_required_trade_count(
    required: &[StrategyFreeReferenceQuoteSubscription],
    trades: &[BoltV3StrategyFreeTrade],
) -> usize {
    let mut observed = BTreeSet::new();
    for required in required {
        let required_instrument_id = required.instrument_id.to_string();
        if trades.iter().any(|trade| {
            trade.data_client_id.as_str() == required.data_client_id.as_str()
                && trade.instrument_id.as_str() == required_instrument_id.as_str()
        }) {
            observed.insert((&required.data_client_id, &required.instrument_id));
        }
    }
    observed.len()
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
    if readiness_probe.quote_target_source != DataClientReadinessProbeQuoteTargetSource::Configured
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
