use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, anyhow, bail};
use nautilus_common::msgbus::{
    MStr, ShareableMessageHandler, TypedHandler, subscribe_any, subscribe_bars,
    subscribe_book_deltas, subscribe_book_depth10, subscribe_index_prices,
    subscribe_instrument_close, subscribe_instruments, subscribe_mark_prices, subscribe_quotes,
    subscribe_trades, unsubscribe_any, unsubscribe_bars, unsubscribe_book_deltas,
    unsubscribe_book_depth10, unsubscribe_index_prices, unsubscribe_instrument_close,
    unsubscribe_instruments, unsubscribe_mark_prices, unsubscribe_quotes, unsubscribe_trades,
};
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::{
    data::{
        Bar, IndexPriceUpdate, InstrumentStatus, MarkPriceUpdate, OrderBookDeltas,
        OrderBookDepth10, QuoteTick, TradeTick, close::InstrumentClose,
    },
    enums::MarketStatusAction,
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

use crate::raw_types::append_jsonl;

struct TypedHandlers {
    quotes: TypedHandler<QuoteTick>,
    trades: TypedHandler<TradeTick>,
    bars: TypedHandler<Bar>,
    book_deltas: TypedHandler<OrderBookDeltas>,
    book_depth10: TypedHandler<OrderBookDepth10>,
    mark_prices: TypedHandler<MarkPriceUpdate>,
    index_prices: TypedHandler<IndexPriceUpdate>,
}

struct AnyHandlers {
    instruments: ShareableMessageHandler,
    instrument_closes: ShareableMessageHandler,
    instrument_statuses: ShareableMessageHandler,
}

#[derive(Clone)]
struct SinkFailureState {
    unhealthy: Arc<AtomicBool>,
    first_error: Arc<Mutex<Option<String>>>,
    notifier: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    stop_handle: LiveNodeHandle,
}

impl SinkFailureState {
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

enum SinkMessage {
    Quote(QuoteTick),
    Trade(TradeTick),
    Bar(Bar),
    Deltas(OrderBookDeltas),
    Depth10(OrderBookDepth10),
    MarkPrice(MarkPriceUpdate),
    IndexPrice(IndexPriceUpdate),
    Instrument(InstrumentAny),
    InstrumentClose(InstrumentClose),
    InstrumentStatus(InstrumentStatus),
}

pub struct NormalizedSinkGuards {
    sender: Option<UnboundedSender<SinkMessage>>,
    supervisor_handle: Option<JoinHandle<Result<()>>>,
    typed_handlers: Option<TypedHandlers>,
    any_handlers: Option<AnyHandlers>,
    failure_state: SinkFailureState,
    failure_receiver: Option<oneshot::Receiver<()>>,
}

impl Drop for NormalizedSinkGuards {
    fn drop(&mut self) {
        self.unsubscribe_all();
        self.sender.take();
    }
}

impl NormalizedSinkGuards {
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
                    join_error = Some(anyhow!("normalized sink worker join failed: {error}"))
                }
            }
        }

        match (self.failure_state.error_message(), join_error) {
            (Some(primary), Some(secondary)) => {
                log::error!("Normalized sink secondary error: {secondary}");
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

fn instruments_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.instrument.*.*")
}

fn instrument_closes_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.close.*.*")
}

fn instrument_statuses_pattern() -> MStr<nautilus_common::msgbus::Pattern> {
    MStr::pattern("data.status.*.*")
}

fn ensure_local_catalog_path(catalog_path: &str) -> Result<()> {
    if catalog_path.contains("://") {
        bail!(
            "Task 3 normalized sink currently supports only local catalog paths, got `{catalog_path}`"
        );
    }

    Ok(())
}

fn send_sink_message(
    sender: &UnboundedSender<SinkMessage>,
    message: SinkMessage,
    label: &str,
    failure_state: &SinkFailureState,
) {
    if failure_state.is_unhealthy() {
        return;
    }

    if let Err(error) = sender.send(message) {
        failure_state.record_failure(format!(
            "Normalized sink channel closed while enqueueing {label}: {error}"
        ));
    }
}

async fn run_sink_worker(
    mut receiver: UnboundedReceiver<SinkMessage>,
    mut writer: FeatherWriter,
    status_path: PathBuf,
    failure_state: SinkFailureState,
) -> Result<()> {
    let mut primary_error: Option<anyhow::Error> = None;
    let mut startup_buffer = VecDeque::new();
    let mut writes_enabled = failure_state.stop_handle.is_running();

    loop {
        let message = if writes_enabled {
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
                        writes_enabled = true;
                    }
                    continue;
                }
            }
        };

        let Some(message) = message else {
            writes_enabled = true;
            if startup_buffer.is_empty() {
                break;
            }
            continue;
        };

        if !writes_enabled {
            startup_buffer.push_back(message);
            if failure_state.stop_handle.is_running() {
                writes_enabled = true;
            }
            continue;
        }

        if let Err(error) = write_sink_message(&mut writer, &status_path, message).await {
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

    if let Some(error) = primary_error {
        Err(error)
    } else {
        Ok(())
    }
}

async fn write_sink_message(
    writer: &mut FeatherWriter,
    status_path: &PathBuf,
    message: SinkMessage,
) -> Result<()> {
    match message {
        SinkMessage::Quote(quote) => writer
            .write(quote)
            .await
            .map_err(|e| anyhow!("QuoteTick write failed: {e}")),
        SinkMessage::Trade(trade) => writer
            .write(trade)
            .await
            .map_err(|e| anyhow!("TradeTick write failed: {e}")),
        SinkMessage::Bar(bar) => writer
            .write(bar)
            .await
            .map_err(|e| anyhow!("Bar write failed: {e}")),
        SinkMessage::Deltas(deltas) => {
            for delta in deltas.deltas {
                writer
                    .write(delta)
                    .await
                    .map_err(|e| anyhow!("OrderBookDelta write failed: {e}"))?;
            }
            Ok(())
        }
        SinkMessage::Depth10(depth) => writer
            .write(depth)
            .await
            .map_err(|e| anyhow!("OrderBookDepth10 write failed: {e}")),
        SinkMessage::MarkPrice(price) => writer
            .write(price)
            .await
            .map_err(|e| anyhow!("MarkPriceUpdate write failed: {e}")),
        SinkMessage::IndexPrice(price) => writer
            .write(price)
            .await
            .map_err(|e| anyhow!("IndexPriceUpdate write failed: {e}")),
        SinkMessage::Instrument(instrument) => writer
            .write_instrument(instrument)
            .await
            .map_err(|e| anyhow!("InstrumentAny write failed: {e}")),
        SinkMessage::InstrumentClose(close) => writer
            .write(close)
            .await
            .map_err(|e| anyhow!("InstrumentClose write failed: {e}")),
        SinkMessage::InstrumentStatus(status) => append_jsonl(status_path, &status)
            .map_err(|e| anyhow!("InstrumentStatus JSONL write failed: {e}")),
    }
}

pub fn wire_normalized_sinks(
    node: &LiveNode,
    stop_handle: LiveNodeHandle,
    catalog_path: &str,
    flush_interval_ms: u64,
) -> Result<NormalizedSinkGuards> {
    ensure_local_catalog_path(catalog_path)?;

    let instance_id = node.instance_id().to_string();
    let spool_root = spool_root_for_instance(catalog_path, &instance_id);
    fs::create_dir_all(&spool_root)?;
    let (object_store, base_path, _uri) = create_object_store_from_path(&spool_root, None)?;
    let status_path = PathBuf::from(&spool_root)
        .join("status")
        .join("instrument_status.jsonl");

    let writer = FeatherWriter::new(
        base_path,
        object_store,
        node.kernel().clock(),
        RotationConfig::NoRotation,
        None,
        None,
        Some(flush_interval_ms),
    );

    // Unbounded is intentional: sink handlers must never block the NT message bus.
    // If the sink falls behind, memory can grow until the process is stopped.
    // This is an accepted Task 3 tradeoff for current local-first Polymarket capture.
    let (sender, receiver) = unbounded_channel();
    let (failure_state, failure_receiver) = SinkFailureState::new(stop_handle);
    let worker_handle = spawn_local(run_sink_worker(
        receiver,
        writer,
        status_path,
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
                let join_error = anyhow!("normalized sink worker join failed: {error}");
                supervisor_failure_state.record_failure(join_error.to_string());
                Err(join_error)
            }
        }
    });

    let quotes_sender = sender.clone();
    let quotes_failure_state = failure_state.clone();
    let quotes = TypedHandler::from(move |quote: &QuoteTick| {
        send_sink_message(
            &quotes_sender,
            SinkMessage::Quote(*quote),
            "QuoteTick",
            &quotes_failure_state,
        );
    });
    subscribe_quotes(quotes_pattern(), quotes.clone(), None);

    let trades_sender = sender.clone();
    let trades_failure_state = failure_state.clone();
    let trades = TypedHandler::from(move |trade: &TradeTick| {
        send_sink_message(
            &trades_sender,
            SinkMessage::Trade(*trade),
            "TradeTick",
            &trades_failure_state,
        );
    });
    subscribe_trades(trades_pattern(), trades.clone(), None);

    let bars_sender = sender.clone();
    let bars_failure_state = failure_state.clone();
    let bars = TypedHandler::from(move |bar: &Bar| {
        send_sink_message(
            &bars_sender,
            SinkMessage::Bar(*bar),
            "Bar",
            &bars_failure_state,
        );
    });
    subscribe_bars(bars_pattern(), bars.clone(), None);

    let deltas_sender = sender.clone();
    let deltas_failure_state = failure_state.clone();
    let book_deltas = TypedHandler::from(move |deltas: &OrderBookDeltas| {
        send_sink_message(
            &deltas_sender,
            SinkMessage::Deltas(deltas.clone()),
            "OrderBookDeltas",
            &deltas_failure_state,
        );
    });
    subscribe_book_deltas(book_deltas_pattern(), book_deltas.clone(), None);

    let depth_sender = sender.clone();
    let depth_failure_state = failure_state.clone();
    let book_depth10 = TypedHandler::from(move |depth: &OrderBookDepth10| {
        send_sink_message(
            &depth_sender,
            SinkMessage::Depth10(*depth),
            "OrderBookDepth10",
            &depth_failure_state,
        );
    });
    subscribe_book_depth10(book_depth10_pattern(), book_depth10.clone(), None);

    let mark_sender = sender.clone();
    let mark_failure_state = failure_state.clone();
    let mark_prices = TypedHandler::from(move |price: &MarkPriceUpdate| {
        send_sink_message(
            &mark_sender,
            SinkMessage::MarkPrice(*price),
            "MarkPriceUpdate",
            &mark_failure_state,
        );
    });
    subscribe_mark_prices(mark_prices_pattern(), mark_prices.clone(), None);

    let index_sender = sender.clone();
    let index_failure_state = failure_state.clone();
    let index_prices = TypedHandler::from(move |price: &IndexPriceUpdate| {
        send_sink_message(
            &index_sender,
            SinkMessage::IndexPrice(*price),
            "IndexPriceUpdate",
            &index_failure_state,
        );
    });
    subscribe_index_prices(index_prices_pattern(), index_prices.clone(), None);

    let instrument_sender = sender.clone();
    let instrument_failure_state = failure_state.clone();
    let instruments = ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
        if let Some(instrument) = message.downcast_ref::<InstrumentAny>() {
            send_sink_message(
                &instrument_sender,
                SinkMessage::Instrument(instrument.clone()),
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
                send_sink_message(
                    &close_sender,
                    SinkMessage::InstrumentClose(*close),
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
                if status.action != MarketStatusAction::Close {
                    return;
                }

                send_sink_message(
                    &status_sender,
                    SinkMessage::InstrumentStatus(*status),
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

    Ok(NormalizedSinkGuards {
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
    use nautilus_model::{
        data::QuoteTick,
        identifiers::InstrumentId,
        types::{Price, Quantity},
    };

    #[test]
    fn failure_state_latches_first_error_and_sets_stop_flag() {
        let handle = LiveNodeHandle::new();
        let (state, _receiver) = SinkFailureState::new(handle.clone());

        state.record_failure("first failure");
        state.record_failure("second failure");

        assert!(state.is_unhealthy());
        assert!(handle.should_stop());
        assert_eq!(state.error_message().as_deref(), Some("first failure"));
    }

    #[test]
    fn send_failure_marks_sink_unhealthy() {
        let handle = LiveNodeHandle::new();
        let (state, _receiver) = SinkFailureState::new(handle.clone());
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

        send_sink_message(&sender, SinkMessage::Quote(quote), "QuoteTick", &state);

        assert!(state.is_unhealthy());
        assert!(handle.should_stop());
        assert!(
            state
                .error_message()
                .unwrap()
                .contains("Normalized sink channel closed while enqueueing QuoteTick")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn record_failure_notifies_receiver() {
        let handle = LiveNodeHandle::new();
        let (state, receiver) = SinkFailureState::new(handle);

        state.record_failure("failure");

        receiver.await.unwrap();
    }
}
