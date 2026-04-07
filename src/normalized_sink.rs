use std::{cell::RefCell, fs, future::Future, path::PathBuf, rc::Rc};

use anyhow::Result;
use nautilus_common::msgbus::{MStr, ShareableMessageHandler, subscribe_any, unsubscribe_any};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    data::{
        Bar, CustomData, Data, IndexPriceUpdate, InstrumentStatus, MarkPriceUpdate, OrderBookDelta,
        OrderBookDeltas, OrderBookDepth10, QuoteTick, TradeTick, close::InstrumentClose,
    },
    enums::MarketStatusAction,
    instruments::InstrumentAny,
};
use nautilus_persistence::{
    backend::feather::{FeatherWriter, RotationConfig},
    parquet::create_object_store_from_path,
};

use crate::raw_types::append_jsonl;

pub struct NormalizedSinkGuards {
    pub feather_writer: Rc<RefCell<FeatherWriter>>,
    pub feather_handler: ShareableMessageHandler,
    pub status_handler: ShareableMessageHandler,
}

impl Drop for NormalizedSinkGuards {
    fn drop(&mut self) {
        unsubscribe_any(MStr::pattern("*"), &self.feather_handler);
        unsubscribe_any(MStr::pattern("*"), &self.status_handler);
    }
}

fn run_feather_write<F>(
    runtime: &tokio::runtime::Handle,
    future: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime.block_on(future))
    } else {
        let _guard = runtime.enter();
        runtime.block_on(future)
    }
}

pub fn spool_root_for_instance(base: &str, instance_id: &str) -> String {
    format!("{base}/live/{instance_id}")
}

pub fn wire_normalized_sinks(
    node: &LiveNode,
    catalog_path: &str,
    flush_interval_ms: u64,
) -> Result<NormalizedSinkGuards> {
    let instance_id = node.instance_id().to_string();
    let spool_root = spool_root_for_instance(catalog_path, &instance_id);
    fs::create_dir_all(&spool_root)?;
    let (object_store, base_path, _uri) = create_object_store_from_path(&spool_root, None)?;
    let status_path = PathBuf::from(&spool_root)
        .join("status")
        .join("instrument_status.jsonl");

    let writer = Rc::new(RefCell::new(FeatherWriter::new(
        base_path,
        object_store,
        node.kernel().clock(),
        RotationConfig::NoRotation,
        None,
        None,
        Some(flush_interval_ms),
    )));
    let runtime = nautilus_common::live::get_runtime().handle().clone();
    let writer_for_bus = writer.clone();
    let feather_handler = ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
        if let Some(quote) = message.downcast_ref::<QuoteTick>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*quote)) {
                log::warn!("Failed to write QuoteTick: {err}");
            }
        } else if let Some(trade) = message.downcast_ref::<TradeTick>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*trade)) {
                log::warn!("Failed to write TradeTick: {err}");
            }
        } else if let Some(bar) = message.downcast_ref::<Bar>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*bar)) {
                log::warn!("Failed to write Bar: {err}");
            }
        } else if let Some(delta) = message.downcast_ref::<OrderBookDelta>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*delta)) {
                log::warn!("Failed to write OrderBookDelta: {err}");
            }
        } else if let Some(depth) = message.downcast_ref::<OrderBookDepth10>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*depth)) {
                log::warn!("Failed to write OrderBookDepth10: {err}");
            }
        } else if let Some(price) = message.downcast_ref::<IndexPriceUpdate>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*price)) {
                log::warn!("Failed to write IndexPriceUpdate: {err}");
            }
        } else if let Some(price) = message.downcast_ref::<MarkPriceUpdate>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*price)) {
                log::warn!("Failed to write MarkPriceUpdate: {err}");
            }
        } else if let Some(close) = message.downcast_ref::<InstrumentClose>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) = run_feather_write(&runtime, writer.write(*close)) {
                log::warn!("Failed to write InstrumentClose: {err}");
            }
        } else if let Some(deltas) = message.downcast_ref::<OrderBookDeltas>() {
            let mut writer = writer_for_bus.borrow_mut();
            for delta in &deltas.deltas {
                if let Err(err) = run_feather_write(&runtime, writer.write(*delta)) {
                    log::warn!("Failed to write OrderBookDelta from OrderBookDeltas: {err}");
                }
            }
        } else if let Some(custom) = message.downcast_ref::<CustomData>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) =
                run_feather_write(&runtime, writer.write_data(Data::Custom(custom.clone())))
            {
                log::warn!("Failed to write CustomData: {err}");
            }
        } else if let Some(instrument) = message.downcast_ref::<InstrumentAny>() {
            let mut writer = writer_for_bus.borrow_mut();
            if let Err(err) =
                run_feather_write(&runtime, writer.write_instrument(instrument.clone()))
            {
                log::warn!("Failed to write InstrumentAny: {err}");
            }
        }
    });
    subscribe_any(MStr::pattern("*"), feather_handler.clone(), None);

    let status_handler = ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
        if let Some(status) = message.downcast_ref::<InstrumentStatus>() {
            if status.action != MarketStatusAction::Close {
                return;
            }

            if let Err(err) = append_jsonl(&status_path, status) {
                log::warn!("Failed to write InstrumentStatus JSONL: {err}");
            }
        }
    });
    subscribe_any(MStr::pattern("*"), status_handler.clone(), None);

    Ok(NormalizedSinkGuards {
        feather_writer: writer,
        feather_handler,
        status_handler,
    })
}
