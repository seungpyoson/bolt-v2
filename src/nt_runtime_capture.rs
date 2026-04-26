use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, anyhow, bail};
use nautilus_common::msgbus::{
    MStr, ShareableMessageHandler, TypedHandler, subscribe_account_state, subscribe_any,
    subscribe_bars, subscribe_book_deltas, subscribe_book_depth10, subscribe_funding_rates,
    subscribe_index_prices, subscribe_instrument_close, subscribe_instruments,
    subscribe_mark_prices, subscribe_order_events, subscribe_position_events, subscribe_quotes,
    subscribe_trades, unsubscribe_account_state, unsubscribe_any, unsubscribe_bars,
    unsubscribe_book_deltas, unsubscribe_book_depth10, unsubscribe_funding_rates,
    unsubscribe_index_prices, unsubscribe_instrument_close, unsubscribe_instruments,
    unsubscribe_mark_prices, unsubscribe_order_events, unsubscribe_position_events,
    unsubscribe_quotes, unsubscribe_trades,
};
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::{
    data::{
        Bar, FundingRateUpdate, IndexPriceUpdate, InstrumentStatus, MarkPriceUpdate,
        OrderBookDeltas, OrderBookDepth10, QuoteTick, TradeTick, close::InstrumentClose,
    },
    events::{AccountState, OrderEventAny, PositionEvent},
    instruments::InstrumentAny,
};
use nautilus_persistence::{
    backend::feather::{FeatherWriter, RotationConfig},
    parquet::create_object_store_from_path,
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    sync::oneshot,
    task::{JoinHandle, spawn_local},
};

use crate::{execution_state, raw_types::JsonlAppender};

struct TypedHandlers {
    quotes: TypedHandler<QuoteTick>,
    trades: TypedHandler<TradeTick>,
    bars: TypedHandler<Bar>,
    book_deltas: TypedHandler<OrderBookDeltas>,
    book_depth10: TypedHandler<OrderBookDepth10>,
    mark_prices: TypedHandler<MarkPriceUpdate>,
    index_prices: TypedHandler<IndexPriceUpdate>,
    funding_rates: TypedHandler<FundingRateUpdate>,
    order_events: TypedHandler<OrderEventAny>,
    position_events: TypedHandler<PositionEvent>,
    account_states: TypedHandler<AccountState>,
}

struct AnyHandlers {
    instruments: ShareableMessageHandler,
    instrument_closes: ShareableMessageHandler,
    instrument_statuses: ShareableMessageHandler,
}

struct SidecarPaths {
    status: PathBuf,
    account_states: PathBuf,
    funding_rates: PathBuf,
    order_events: PathBuf,
    position_events: PathBuf,
}

struct SidecarWriters {
    status: JsonlAppender,
    account_states: JsonlAppender,
    funding_rates: JsonlAppender,
    order_events: JsonlAppender,
    position_events: JsonlAppender,
}

#[derive(Clone)]
struct CaptureFailureState {
    unhealthy: Arc<AtomicBool>,
    first_error: Arc<Mutex<Option<String>>>,
    notifier: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    stop_handle: LiveNodeHandle,
}

impl CaptureFailureState {
    fn new(stop_handle: LiveNodeHandle) -> (Self, oneshot::Receiver<()>) {
        let (notifier, receiver) = oneshot::channel();
        (
            Self {
                unhealthy: Arc::new(AtomicBool::new(false)),
                first_error: Arc::new(Mutex::new(None)),
                notifier: Arc::new(Mutex::new(Some(notifier))),
                stop_handle,
            },
            receiver,
        )
    }

    fn is_unhealthy(&self) -> bool {
        self.unhealthy.load(Ordering::Relaxed)
    }

    fn error_message(&self) -> Option<String> {
        self.first_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record_failure(&self, message: impl Into<String>) {
        let message = message.into();
        let is_first = self
            .unhealthy
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok();

        if is_first {
            let mut slot = self
                .first_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(message.clone());
            }
            log::error!("{message}");
            if let Some(notifier) = self
                .notifier
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = notifier.send(());
            }
            self.stop_handle.stop();
        }
    }
}

enum CaptureMessage {
    Quote(QuoteTick),
    Trade(TradeTick),
    Bar(Bar),
    Deltas(OrderBookDeltas),
    Depth10(Box<OrderBookDepth10>),
    MarkPrice(MarkPriceUpdate),
    IndexPrice(IndexPriceUpdate),
    FundingRate(FundingRateUpdate),
    Instrument(Box<InstrumentAny>),
    InstrumentClose(InstrumentClose),
    InstrumentStatus(InstrumentStatus),
    OrderEvent(Box<OrderEventAny>),
    PositionEvent(Box<PositionEvent>),
    AccountState(Box<AccountState>),
}

pub struct NtRuntimeCaptureGuards {
    sender: Option<UnboundedSender<CaptureMessage>>,
    supervisor_handle: Option<JoinHandle<Result<()>>>,
    typed_handlers: Option<TypedHandlers>,
    any_handlers: Option<AnyHandlers>,
    failure_state: CaptureFailureState,
    failure_receiver: Option<oneshot::Receiver<()>>,
}

impl Drop for NtRuntimeCaptureGuards {
    fn drop(&mut self) {
        self.unsubscribe_all();
        self.sender.take();
    }
}

impl NtRuntimeCaptureGuards {
    pub fn take_failure_receiver(&mut self) -> Option<oneshot::Receiver<()>> {
        self.failure_receiver.take()
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.unsubscribe_all();
        self.sender.take();
        self.failure_receiver.take();

        let mut join_error: Option<anyhow::Error> = None;
        if let Some(handle) = self.supervisor_handle.take() {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => join_error = Some(error),
                Err(error) => {
                    join_error = Some(anyhow!("NT runtime capture worker join failed: {error}"))
                }
            }
        }

        match (self.failure_state.error_message(), join_error) {
            (Some(primary), Some(secondary)) => {
                log::error!("NT runtime capture secondary error: {secondary}");
                Err(anyhow!(primary))
            }
            (Some(primary), None) => Err(anyhow!(primary)),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(()),
        }
    }

    fn unsubscribe_all(&mut self) {
        if let Some(typed) = self.typed_handlers.take() {
            unsubscribe_quotes(quotes_pattern(), &typed.quotes);
            unsubscribe_trades(trades_pattern(), &typed.trades);
            unsubscribe_bars(bars_pattern(), &typed.bars);
            unsubscribe_book_deltas(book_deltas_pattern(), &typed.book_deltas);
            unsubscribe_book_depth10(book_depth10_pattern(), &typed.book_depth10);
            unsubscribe_mark_prices(mark_prices_pattern(), &typed.mark_prices);
            unsubscribe_index_prices(index_prices_pattern(), &typed.index_prices);
            unsubscribe_funding_rates(funding_rates_pattern(), &typed.funding_rates);
            unsubscribe_order_events(order_events_pattern(), &typed.order_events);
            unsubscribe_position_events(position_events_pattern(), &typed.position_events);
            unsubscribe_account_state(account_states_pattern(), &typed.account_states);
        }

        if let Some(any) = self.any_handlers.take() {
            unsubscribe_instruments(instruments_pattern(), &any.instruments);
            unsubscribe_instrument_close(instrument_closes_pattern(), &any.instrument_closes);
            unsubscribe_any(instrument_statuses_pattern(), &any.instrument_statuses);
        }
    }
}

pub fn spool_root_for_instance(base: &str, instance_id: &str) -> String {
    format!("{base}/live/{instance_id}")
}

fn quotes_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.quotes.*.*")
}

fn trades_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.trades.*.*")
}

fn bars_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.bars.*")
}

fn book_deltas_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.book.deltas.*.*")
}

fn book_depth10_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.book.depth10.*.*")
}

fn mark_prices_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.mark_prices.*.*")
}

fn index_prices_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.index_prices.*.*")
}

fn funding_rates_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.funding_rates.*.*")
}

fn instruments_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.instrument.*.*")
}

fn instrument_closes_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.close.*.*")
}

fn instrument_statuses_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.status.*.*")
}

fn order_events_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("events.order.*")
}

fn position_events_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("events.position.*")
}

fn account_states_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("events.account.*")
}

fn per_instrument_stream_types() -> HashSet<String> {
    // Bars are intentionally excluded. FeatherWriter keys per-instrument writers by
    // (type, instrument_id), but Bar schema metadata is also bar_type-specific. Grouping all
    // bars for one instrument into a single per-instrument writer would mix multiple bar_type
    // streams behind the first bar_type metadata seen. Bars therefore remain on the legacy flat
    // spool contract until a bar-type-safe offline path is introduced.
    HashSet::from([
        "quotes".to_string(),
        "trades".to_string(),
        "order_book_deltas".to_string(),
        "order_book_depths".to_string(),
        "index_prices".to_string(),
        "mark_prices".to_string(),
        "instrument_closes".to_string(),
        "instruments".to_string(),
    ])
}

fn ensure_local_catalog_path(catalog_path: &str) -> Result<()> {
    if catalog_path.contains("://") {
        bail!(
            "Task 3 NT runtime capture currently supports only local catalog paths, got `{catalog_path}`"
        );
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct ContractStartupSummary {
    supported: Vec<String>,
    conditional: Vec<String>,
    disabled: Vec<String>,
    unsupported: Vec<String>,
}

fn contract_startup_summary(
    contract: &crate::venue_contract::VenueContract,
) -> ContractStartupSummary {
    let mut summary = ContractStartupSummary {
        supported: Vec::new(),
        conditional: Vec::new(),
        disabled: Vec::new(),
        unsupported: Vec::new(),
    };

    for (name, stream) in &contract.streams {
        let effective_policy = contract.effective_policy(name);
        match (&stream.capability, effective_policy) {
            (crate::venue_contract::Capability::Unsupported, _) => {
                summary.unsupported.push(name.clone());
            }
            (
                crate::venue_contract::Capability::Supported
                | crate::venue_contract::Capability::Conditional,
                Some(crate::venue_contract::Policy::Disabled),
            ) => {
                summary.disabled.push(name.clone());
            }
            (crate::venue_contract::Capability::Supported, _) => {
                summary.supported.push(name.clone());
            }
            (crate::venue_contract::Capability::Conditional, _) => {
                summary.conditional.push(name.clone());
            }
        }
    }

    summary
}

fn format_contract_startup_log(contract: &crate::venue_contract::VenueContract) -> String {
    let summary = contract_startup_summary(contract);
    format!(
        "Contract loaded: {} -- supported {:?}; conditional {:?}; disabled {:?}; unsupported {:?}. Startup subscriptions are unchanged; contract policy is enforced during stream-to-lake conversion.",
        contract.venue,
        summary.supported,
        summary.conditional,
        summary.disabled,
        summary.unsupported,
    )
}

fn send_capture_message(
    sender: &UnboundedSender<CaptureMessage>,
    message: CaptureMessage,
    label: &str,
    failure_state: &CaptureFailureState,
) {
    if failure_state.is_unhealthy() {
        return;
    }

    if let Err(error) = sender.send(message) {
        failure_state.record_failure(format!(
            "NT runtime capture channel closed while enqueueing {label}: {error}"
        ));
    }
}

async fn run_capture_worker(
    mut receiver: UnboundedReceiver<CaptureMessage>,
    mut writer: FeatherWriter,
    mut sidecar_writers: SidecarWriters,
    sidecar_paths: SidecarPaths,
    failure_state: CaptureFailureState,
) -> Result<()> {
    let mut primary_error: Option<anyhow::Error> = None;
    let mut startup_buffer = VecDeque::new();
    let mut saw_running = failure_state.stop_handle.is_running();

    loop {
        let message = if saw_running {
            if let Some(message) = startup_buffer.pop_front() {
                Some(message)
            } else {
                receiver.recv().await
            }
        } else {
            tokio::select! {
                maybe_message = receiver.recv() => maybe_message,
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => {
                    if failure_state.stop_handle.is_running() {
                        saw_running = true;
                    }
                    continue;
                }
            }
        };

        let Some(message) = message else {
            if !saw_running {
                startup_buffer.clear();
                break;
            }

            if startup_buffer.is_empty() {
                break;
            }
            continue;
        };

        if !saw_running {
            startup_buffer.push_back(message);
            if failure_state.stop_handle.is_running() {
                saw_running = true;
            }
            continue;
        }

        if let Err(error) =
            write_capture_message(&mut writer, &mut sidecar_writers, &sidecar_paths, message).await
        {
            failure_state.record_failure(error.to_string());
            primary_error = Some(error);
            break;
        }
    }

    if let Err(error) = writer.close().await {
        let close_error = anyhow!("Failed to close FeatherWriter: {error}");
        if primary_error.is_none() {
            failure_state.record_failure(close_error.to_string());
            primary_error = Some(close_error);
        } else {
            log::error!("{close_error}");
        }
    }

    if let Err(error) = sidecar_writers.status.close() {
        let close_error = anyhow!("Failed to close instrument status JSONL writer: {error}");
        if primary_error.is_none() {
            failure_state.record_failure(close_error.to_string());
            primary_error = Some(close_error);
        } else {
            log::error!("{close_error}");
        }
    }

    if let Err(error) = sidecar_writers.account_states.close() {
        let close_error = anyhow!("Failed to close account state JSONL writer: {error}");
        if primary_error.is_none() {
            failure_state.record_failure(close_error.to_string());
            primary_error = Some(close_error);
        } else {
            log::error!("{close_error}");
        }
    }

    if let Err(error) = sidecar_writers.funding_rates.close() {
        let close_error = anyhow!("Failed to close funding rates JSONL writer: {error}");
        if primary_error.is_none() {
            failure_state.record_failure(close_error.to_string());
            primary_error = Some(close_error);
        } else {
            log::error!("{close_error}");
        }
    }

    if let Err(error) = sidecar_writers.order_events.close() {
        let close_error = anyhow!("Failed to close order event JSONL writer: {error}");
        if primary_error.is_none() {
            failure_state.record_failure(close_error.to_string());
            primary_error = Some(close_error);
        } else {
            log::error!("{close_error}");
        }
    }

    if let Err(error) = sidecar_writers.position_events.close() {
        let close_error = anyhow!("Failed to close position event JSONL writer: {error}");
        if primary_error.is_none() {
            failure_state.record_failure(close_error.to_string());
            primary_error = Some(close_error);
        } else {
            log::error!("{close_error}");
        }
    }

    if let Some(error) = primary_error {
        Err(error)
    } else {
        Ok(())
    }
}

async fn write_capture_message(
    writer: &mut FeatherWriter,
    sidecar_writers: &mut SidecarWriters,
    sidecar_paths: &SidecarPaths,
    message: CaptureMessage,
) -> Result<()> {
    match message {
        CaptureMessage::Quote(quote) => writer
            .write(quote)
            .await
            .map_err(|e| anyhow!("QuoteTick write failed: {e}")),
        CaptureMessage::Trade(trade) => writer
            .write(trade)
            .await
            .map_err(|e| anyhow!("TradeTick write failed: {e}")),
        CaptureMessage::Bar(bar) => writer
            .write(bar)
            .await
            .map_err(|e| anyhow!("Bar write failed: {e}")),
        CaptureMessage::Deltas(deltas) => {
            for delta in deltas.deltas {
                writer
                    .write(delta)
                    .await
                    .map_err(|e| anyhow!("OrderBookDelta write failed: {e}"))?;
            }
            Ok(())
        }
        CaptureMessage::Depth10(depth) => writer
            .write(*depth)
            .await
            .map_err(|e| anyhow!("OrderBookDepth10 write failed: {e}")),
        CaptureMessage::MarkPrice(price) => writer
            .write(price)
            .await
            .map_err(|e| anyhow!("MarkPriceUpdate write failed: {e}")),
        CaptureMessage::IndexPrice(price) => writer
            .write(price)
            .await
            .map_err(|e| anyhow!("IndexPriceUpdate write failed: {e}")),
        CaptureMessage::FundingRate(funding_rate) => sidecar_writers
            .funding_rates
            .append(&sidecar_paths.funding_rates, &funding_rate)
            .map_err(|e| anyhow!("FundingRateUpdate JSONL write failed: {e}")),
        CaptureMessage::Instrument(instrument) => writer
            .write_instrument(*instrument)
            .await
            .map_err(|e| anyhow!("InstrumentAny write failed: {e}")),
        CaptureMessage::InstrumentClose(close) => writer
            .write(close)
            .await
            .map_err(|e| anyhow!("InstrumentClose write failed: {e}")),
        CaptureMessage::InstrumentStatus(status) => sidecar_writers
            .status
            .append(&sidecar_paths.status, &status)
            .map_err(|e| anyhow!("InstrumentStatus JSONL write failed: {e}")),
        CaptureMessage::OrderEvent(event) => {
            let row = execution_state::order_event_row(&event)
                .map_err(|e| anyhow!("OrderEvent summary failed: {e}"))?;
            sidecar_writers
                .order_events
                .append(&sidecar_paths.order_events, &row)
                .map_err(|e| anyhow!("OrderEvent JSONL write failed: {e}"))
        }
        CaptureMessage::PositionEvent(event) => {
            let row = execution_state::position_event_row(&event)
                .map_err(|e| anyhow!("PositionEvent summary failed: {e}"))?;
            sidecar_writers
                .position_events
                .append(&sidecar_paths.position_events, &row)
                .map_err(|e| anyhow!("PositionEvent JSONL write failed: {e}"))
        }
        CaptureMessage::AccountState(state) => sidecar_writers
            .account_states
            .append(&sidecar_paths.account_states, &state)
            .map_err(|e| anyhow!("AccountState JSONL write failed: {e}")),
    }
}

pub fn wire_nt_runtime_capture(
    node: &LiveNode,
    stop_handle: LiveNodeHandle,
    catalog_path: &str,
    flush_interval_ms: u64,
    contract_path: Option<&str>,
) -> Result<NtRuntimeCaptureGuards> {
    ensure_local_catalog_path(catalog_path)?;

    if let Some(path) = contract_path {
        let normalized = crate::venue_contract::normalize_local_absolute_contract_path(
            std::path::Path::new(path),
        )?;
        let contract = crate::venue_contract::VenueContract::load_and_validate(&normalized)?;
        log::info!("{}", format_contract_startup_log(&contract));
    }

    let instance_id = node.instance_id().to_string();
    let spool_root = spool_root_for_instance(catalog_path, &instance_id);
    fs::create_dir_all(&spool_root)?;
    let (object_store, base_path, _uri) = create_object_store_from_path(&spool_root, None)?;
    let spool_root_path = PathBuf::from(&spool_root);
    let sidecar_paths = SidecarPaths {
        status: spool_root_path
            .join("status")
            .join("instrument_status.jsonl"),
        account_states: spool_root_path.join("accounts").join("account_state.jsonl"),
        funding_rates: spool_root_path.join("funding_rates").join("updates.jsonl"),
        order_events: execution_state::order_events_path(&spool_root_path),
        position_events: execution_state::position_events_path(&spool_root_path),
    };

    let writer = FeatherWriter::new(
        base_path,
        object_store,
        node.kernel().clock(),
        RotationConfig::NoRotation,
        None,
        Some(per_instrument_stream_types()),
        Some(flush_interval_ms),
    );

    // Unbounded is intentional: sink handlers must never block the NT message bus.
    // If the sink falls behind, memory can grow until the process is stopped.
    // This is an accepted Task 3 tradeoff for current local-first Polymarket capture.
    let (sender, receiver) = unbounded_channel();
    let (failure_state, failure_receiver) = CaptureFailureState::new(stop_handle);
    let worker_handle = spawn_local(run_capture_worker(
        receiver,
        writer,
        SidecarWriters {
            status: JsonlAppender::new(),
            account_states: JsonlAppender::new(),
            funding_rates: JsonlAppender::new(),
            order_events: JsonlAppender::new(),
            position_events: JsonlAppender::new(),
        },
        sidecar_paths,
        failure_state.clone(),
    ));
    let supervisor_failure_state = failure_state.clone();
    let supervisor_handle = spawn_local(async move {
        match worker_handle.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                supervisor_failure_state.record_failure(error.to_string());
                Err(error)
            }
            Err(error) => {
                let join_error = anyhow!("NT runtime capture worker join failed: {error}");
                supervisor_failure_state.record_failure(join_error.to_string());
                Err(join_error)
            }
        }
    });

    let quotes_sender = sender.clone();
    let quotes_failure_state = failure_state.clone();
    let quotes = TypedHandler::from(move |quote: &QuoteTick| {
        send_capture_message(
            &quotes_sender,
            CaptureMessage::Quote(*quote),
            "QuoteTick",
            &quotes_failure_state,
        );
    });
    subscribe_quotes(quotes_pattern(), quotes.clone(), None);

    let trades_sender = sender.clone();
    let trades_failure_state = failure_state.clone();
    let trades = TypedHandler::from(move |trade: &TradeTick| {
        send_capture_message(
            &trades_sender,
            CaptureMessage::Trade(*trade),
            "TradeTick",
            &trades_failure_state,
        );
    });
    subscribe_trades(trades_pattern(), trades.clone(), None);

    let bars_sender = sender.clone();
    let bars_failure_state = failure_state.clone();
    let bars = TypedHandler::from(move |bar: &Bar| {
        send_capture_message(
            &bars_sender,
            CaptureMessage::Bar(*bar),
            "Bar",
            &bars_failure_state,
        );
    });
    subscribe_bars(bars_pattern(), bars.clone(), None);

    let deltas_sender = sender.clone();
    let deltas_failure_state = failure_state.clone();
    let book_deltas = TypedHandler::from(move |deltas: &OrderBookDeltas| {
        send_capture_message(
            &deltas_sender,
            CaptureMessage::Deltas(deltas.clone()),
            "OrderBookDeltas",
            &deltas_failure_state,
        );
    });
    subscribe_book_deltas(book_deltas_pattern(), book_deltas.clone(), None);

    let depth_sender = sender.clone();
    let depth_failure_state = failure_state.clone();
    let book_depth10 = TypedHandler::from(move |depth: &OrderBookDepth10| {
        send_capture_message(
            &depth_sender,
            CaptureMessage::Depth10(Box::new(*depth)),
            "OrderBookDepth10",
            &depth_failure_state,
        );
    });
    subscribe_book_depth10(book_depth10_pattern(), book_depth10.clone(), None);

    let mark_sender = sender.clone();
    let mark_failure_state = failure_state.clone();
    let mark_prices = TypedHandler::from(move |price: &MarkPriceUpdate| {
        send_capture_message(
            &mark_sender,
            CaptureMessage::MarkPrice(*price),
            "MarkPriceUpdate",
            &mark_failure_state,
        );
    });
    subscribe_mark_prices(mark_prices_pattern(), mark_prices.clone(), None);

    let index_sender = sender.clone();
    let index_failure_state = failure_state.clone();
    let index_prices = TypedHandler::from(move |price: &IndexPriceUpdate| {
        send_capture_message(
            &index_sender,
            CaptureMessage::IndexPrice(*price),
            "IndexPriceUpdate",
            &index_failure_state,
        );
    });
    subscribe_index_prices(index_prices_pattern(), index_prices.clone(), None);

    let funding_sender = sender.clone();
    let funding_failure_state = failure_state.clone();
    let funding_rates = TypedHandler::from(move |funding_rate: &FundingRateUpdate| {
        send_capture_message(
            &funding_sender,
            CaptureMessage::FundingRate(*funding_rate),
            "FundingRateUpdate",
            &funding_failure_state,
        );
    });
    subscribe_funding_rates(funding_rates_pattern(), funding_rates.clone(), None);

    let order_events_sender = sender.clone();
    let order_events_failure_state = failure_state.clone();
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        send_capture_message(
            &order_events_sender,
            CaptureMessage::OrderEvent(Box::new(event.clone())),
            "OrderEventAny",
            &order_events_failure_state,
        );
    });
    subscribe_order_events(order_events_pattern(), order_events.clone(), None);

    let position_events_sender = sender.clone();
    let position_events_failure_state = failure_state.clone();
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        send_capture_message(
            &position_events_sender,
            CaptureMessage::PositionEvent(Box::new(event.clone())),
            "PositionEvent",
            &position_events_failure_state,
        );
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);

    let account_sender = sender.clone();
    let account_failure_state = failure_state.clone();
    let account_states = TypedHandler::from(move |state: &AccountState| {
        send_capture_message(
            &account_sender,
            CaptureMessage::AccountState(Box::new(state.clone())),
            "AccountState",
            &account_failure_state,
        );
    });
    subscribe_account_state(account_states_pattern(), account_states.clone(), None);

    let instrument_sender = sender.clone();
    let instrument_failure_state = failure_state.clone();
    let instruments = ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
        if let Some(instrument) = message.downcast_ref::<InstrumentAny>() {
            send_capture_message(
                &instrument_sender,
                CaptureMessage::Instrument(Box::new(instrument.clone())),
                "InstrumentAny",
                &instrument_failure_state,
            );
        }
    });
    subscribe_instruments(instruments_pattern(), instruments.clone(), None);

    let close_sender = sender.clone();
    let close_failure_state = failure_state.clone();
    let instrument_closes =
        ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
            if let Some(close) = message.downcast_ref::<InstrumentClose>() {
                send_capture_message(
                    &close_sender,
                    CaptureMessage::InstrumentClose(*close),
                    "InstrumentClose",
                    &close_failure_state,
                );
            }
        });
    subscribe_instrument_close(instrument_closes_pattern(), instrument_closes.clone(), None);

    let status_sender = sender.clone();
    let status_failure_state = failure_state.clone();
    let instrument_statuses =
        ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
            if let Some(status) = message.downcast_ref::<InstrumentStatus>() {
                send_capture_message(
                    &status_sender,
                    CaptureMessage::InstrumentStatus(*status),
                    "InstrumentStatus",
                    &status_failure_state,
                );
            }
        });
    subscribe_any(
        instrument_statuses_pattern(),
        instrument_statuses.clone(),
        None,
    );

    Ok(NtRuntimeCaptureGuards {
        sender: Some(sender),
        supervisor_handle: Some(supervisor_handle),
        typed_handlers: Some(TypedHandlers {
            quotes,
            trades,
            bars,
            book_deltas,
            book_depth10,
            mark_prices,
            index_prices,
            funding_rates,
            order_events,
            position_events,
            account_states,
        }),
        any_handlers: Some(AnyHandlers {
            instruments,
            instrument_closes,
            instrument_statuses,
        }),
        failure_state,
        failure_receiver: Some(failure_receiver),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue_contract::{Capability, Policy, Provenance, StreamContract, VenueContract};
    use nautilus_model::{
        data::QuoteTick,
        identifiers::InstrumentId,
        types::{Price, Quantity},
    };

    #[test]
    fn failure_state_latches_first_error_and_sets_stop_flag() {
        let handle = LiveNodeHandle::new();
        let (state, _receiver) = CaptureFailureState::new(handle.clone());

        state.record_failure("first failure");
        state.record_failure("second failure");

        assert!(state.is_unhealthy());
        assert!(handle.should_stop());
        assert_eq!(state.error_message().as_deref(), Some("first failure"));
    }

    #[test]
    fn send_failure_marks_sink_unhealthy() {
        let handle = LiveNodeHandle::new();
        let (state, _receiver) = CaptureFailureState::new(handle.clone());
        let (sender, receiver) = unbounded_channel();
        drop(receiver);

        let quote = QuoteTick::new(
            InstrumentId::from("0xabc-123456789.POLYMARKET"),
            Price::from("0.45"),
            Price::from("0.55"),
            Quantity::from("100"),
            Quantity::from("100"),
            1.into(),
            1.into(),
        );

        send_capture_message(&sender, CaptureMessage::Quote(quote), "QuoteTick", &state);

        assert!(state.is_unhealthy());
        assert!(handle.should_stop());
        assert!(
            state
                .error_message()
                .unwrap()
                .contains("NT runtime capture channel closed while enqueueing QuoteTick")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn record_failure_notifies_receiver() {
        let handle = LiveNodeHandle::new();
        let (state, receiver) = CaptureFailureState::new(handle);

        state.record_failure("failure");

        receiver.await.unwrap();
    }

    #[test]
    fn contract_startup_summary_separates_disabled_from_capability_buckets() {
        let contract = VenueContract {
            schema_version: 1,
            venue: "test".to_string(),
            adapter_version: "bolt-v2".to_string(),
            streams: [
                (
                    "quotes".to_string(),
                    StreamContract {
                        capability: Capability::Supported,
                        policy: Some(Policy::Required),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "trades".to_string(),
                    StreamContract {
                        capability: Capability::Conditional,
                        policy: Some(Policy::Optional),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "order_book_deltas".to_string(),
                    StreamContract {
                        capability: Capability::Supported,
                        policy: Some(Policy::Disabled),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "mark_prices".to_string(),
                    StreamContract {
                        capability: Capability::Conditional,
                        policy: Some(Policy::Disabled),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "instrument_closes".to_string(),
                    StreamContract {
                        capability: Capability::Unsupported,
                        policy: None,
                        provenance: Provenance::Native,
                        reason: Some("n/a".to_string()),
                        derived_from: None,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };

        assert_eq!(
            contract_startup_summary(&contract),
            ContractStartupSummary {
                supported: vec!["quotes".to_string()],
                conditional: vec!["trades".to_string()],
                disabled: vec!["mark_prices".to_string(), "order_book_deltas".to_string()],
                unsupported: vec!["instrument_closes".to_string()],
            }
        );
    }

    #[test]
    fn contract_startup_log_message_describes_boundary_exactly() {
        let contract = VenueContract {
            schema_version: 1,
            venue: "test".to_string(),
            adapter_version: "bolt-v2".to_string(),
            streams: [
                (
                    "quotes".to_string(),
                    StreamContract {
                        capability: Capability::Supported,
                        policy: Some(Policy::Required),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "trades".to_string(),
                    StreamContract {
                        capability: Capability::Conditional,
                        policy: Some(Policy::Optional),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "order_book_deltas".to_string(),
                    StreamContract {
                        capability: Capability::Supported,
                        policy: Some(Policy::Disabled),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "mark_prices".to_string(),
                    StreamContract {
                        capability: Capability::Conditional,
                        policy: Some(Policy::Disabled),
                        provenance: Provenance::Native,
                        reason: None,
                        derived_from: None,
                    },
                ),
                (
                    "instrument_closes".to_string(),
                    StreamContract {
                        capability: Capability::Unsupported,
                        policy: None,
                        provenance: Provenance::Native,
                        reason: Some("n/a".to_string()),
                        derived_from: None,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };

        assert_eq!(
            format_contract_startup_log(&contract),
            "Contract loaded: test -- supported [\"quotes\"]; conditional [\"trades\"]; disabled [\"mark_prices\", \"order_book_deltas\"]; unsupported [\"instrument_closes\"]. Startup subscriptions are unchanged; contract policy is enforced during stream-to-lake conversion."
        );
    }
}
