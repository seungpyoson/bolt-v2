use std::{fs, path::PathBuf};

use anyhow::{Result, anyhow, bail};
use nautilus_common::msgbus::{
    MStr, ShareableMessageHandler, TypedHandler, subscribe_any, subscribe_bars,
    subscribe_book_deltas, subscribe_book_depth10, subscribe_index_prices,
    subscribe_instrument_close, subscribe_instruments, subscribe_mark_prices, subscribe_quotes,
    subscribe_trades, unsubscribe_any, unsubscribe_bars, unsubscribe_book_deltas,
    unsubscribe_book_depth10, unsubscribe_index_prices, unsubscribe_instrument_close,
    unsubscribe_instruments, unsubscribe_mark_prices, unsubscribe_quotes, unsubscribe_trades,
};
use nautilus_live::node::LiveNode;
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
    worker_handle: Option<JoinHandle<Result<()>>>,
    typed_handlers: Option<TypedHandlers>,
    any_handlers: Option<AnyHandlers>,
}

impl Drop for NormalizedSinkGuards {
    fn drop(&mut self) {
        self.unsubscribe_all();
        self.sender.take();
    }
}

impl NormalizedSinkGuards {
    pub async fn shutdown(mut self) -> Result<()> {
        self.unsubscribe_all();
        self.sender.take();

        if let Some(handle) = self.worker_handle.take() {
            match handle.await {
                Ok(result) => result?,
                Err(error) => return Err(anyhow!("normalized sink worker join failed: {error}")),
            }
        }

        Ok(())
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

fn send_sink_message(sender: &UnboundedSender<SinkMessage>, message: SinkMessage, label: &str) {
    if let Err(error) = sender.send(message) {
        log::warn!("Failed to enqueue {label} for normalized sink: {error}");
    }
}

async fn run_sink_worker(
    mut receiver: UnboundedReceiver<SinkMessage>,
    mut writer: FeatherWriter,
    status_path: PathBuf,
) -> Result<()> {
    while let Some(message) = receiver.recv().await {
        match message {
            SinkMessage::Quote(quote) => {
                writer
                    .write(quote)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::Trade(trade) => {
                writer
                    .write(trade)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::Bar(bar) => {
                writer
                    .write(bar)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::Deltas(deltas) => {
                for delta in deltas.deltas {
                    writer
                        .write(delta)
                        .await
                        .map_err(|e| anyhow!(e.to_string()))?;
                }
            }
            SinkMessage::Depth10(depth) => {
                writer
                    .write(depth)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::MarkPrice(price) => {
                writer
                    .write(price)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::IndexPrice(price) => {
                writer
                    .write(price)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::Instrument(instrument) => {
                writer
                    .write_instrument(instrument)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::InstrumentClose(close) => {
                writer
                    .write(close)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
            SinkMessage::InstrumentStatus(status) => {
                append_jsonl(&status_path, &status)?;
            }
        }
    }

    writer.close().await.map_err(|e| anyhow!(e.to_string()))?;
    Ok(())
}

pub fn wire_normalized_sinks(
    node: &LiveNode,
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

    let (sender, receiver) = unbounded_channel();
    let worker_handle = spawn_local(run_sink_worker(receiver, writer, status_path));

    let quotes_sender = sender.clone();
    let quotes = TypedHandler::from(move |quote: &QuoteTick| {
        send_sink_message(&quotes_sender, SinkMessage::Quote(*quote), "QuoteTick");
    });
    subscribe_quotes(quotes_pattern(), quotes.clone(), None);

    let trades_sender = sender.clone();
    let trades = TypedHandler::from(move |trade: &TradeTick| {
        send_sink_message(&trades_sender, SinkMessage::Trade(*trade), "TradeTick");
    });
    subscribe_trades(trades_pattern(), trades.clone(), None);

    let bars_sender = sender.clone();
    let bars = TypedHandler::from(move |bar: &Bar| {
        send_sink_message(&bars_sender, SinkMessage::Bar(*bar), "Bar");
    });
    subscribe_bars(bars_pattern(), bars.clone(), None);

    let deltas_sender = sender.clone();
    let book_deltas = TypedHandler::from(move |deltas: &OrderBookDeltas| {
        send_sink_message(
            &deltas_sender,
            SinkMessage::Deltas(deltas.clone()),
            "OrderBookDeltas",
        );
    });
    subscribe_book_deltas(book_deltas_pattern(), book_deltas.clone(), None);

    let depth_sender = sender.clone();
    let book_depth10 = TypedHandler::from(move |depth: &OrderBookDepth10| {
        send_sink_message(
            &depth_sender,
            SinkMessage::Depth10(*depth),
            "OrderBookDepth10",
        );
    });
    subscribe_book_depth10(book_depth10_pattern(), book_depth10.clone(), None);

    let mark_sender = sender.clone();
    let mark_prices = TypedHandler::from(move |price: &MarkPriceUpdate| {
        send_sink_message(
            &mark_sender,
            SinkMessage::MarkPrice(*price),
            "MarkPriceUpdate",
        );
    });
    subscribe_mark_prices(mark_prices_pattern(), mark_prices.clone(), None);

    let index_sender = sender.clone();
    let index_prices = TypedHandler::from(move |price: &IndexPriceUpdate| {
        send_sink_message(
            &index_sender,
            SinkMessage::IndexPrice(*price),
            "IndexPriceUpdate",
        );
    });
    subscribe_index_prices(index_prices_pattern(), index_prices.clone(), None);

    let instrument_sender = sender.clone();
    let instruments = ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
        if let Some(instrument) = message.downcast_ref::<InstrumentAny>() {
            send_sink_message(
                &instrument_sender,
                SinkMessage::Instrument(instrument.clone()),
                "InstrumentAny",
            );
        }
    });
    subscribe_instruments(instruments_pattern(), instruments.clone(), None);

    let close_sender = sender.clone();
    let instrument_closes =
        ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
            if let Some(close) = message.downcast_ref::<InstrumentClose>() {
                send_sink_message(
                    &close_sender,
                    SinkMessage::InstrumentClose(*close),
                    "InstrumentClose",
                );
            }
        });
    subscribe_instrument_close(instrument_closes_pattern(), instrument_closes.clone(), None);

    let status_sender = sender.clone();
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
        worker_handle: Some(worker_handle),
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
    })
}
