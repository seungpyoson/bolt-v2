use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use arrow::{
    array::{StringArray, UInt64Array},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use nautilus_model::events::{OrderEventAny, PositionEvent};
use parquet::arrow::ArrowWriter;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const ORDER_EVENTS_CLASS: &str = stringify!(order_events);
pub const POSITION_EVENTS_CLASS: &str = stringify!(position_events);
const EVENTS_JSONL_FILE: &str = "events.jsonl";
const DATA_DIR: &str = stringify!(data);
const PART_ZERO_PARQUET_FILE: &str = "part-0.parquet";

const POSITION_OPENED_EVENT_TYPE: &str = stringify!(PositionOpened);
const POSITION_CHANGED_EVENT_TYPE: &str = stringify!(PositionChanged);
const POSITION_CLOSED_EVENT_TYPE: &str = stringify!(PositionClosed);
const POSITION_ADJUSTED_EVENT_TYPE: &str = stringify!(PositionAdjusted);

const EVENT_TYPE_FIELD: &str = stringify!(event_type);
const TRADER_ID_FIELD: &str = stringify!(trader_id);
const STRATEGY_ID_FIELD: &str = stringify!(strategy_id);
const INSTRUMENT_ID_FIELD: &str = stringify!(instrument_id);
const CLIENT_ORDER_ID_FIELD: &str = stringify!(client_order_id);
const VENUE_ORDER_ID_FIELD: &str = stringify!(venue_order_id);
const ACCOUNT_ID_FIELD: &str = stringify!(account_id);
const EVENT_ID_FIELD: &str = stringify!(event_id);
const ENTRY_FIELD: &str = stringify!(entry);
const SIDE_FIELD: &str = stringify!(side);
const SIGNED_QTY_FIELD: &str = stringify!(signed_qty);
const QUANTITY_FIELD: &str = stringify!(quantity);
const PEAK_QUANTITY_FIELD: &str = stringify!(peak_quantity);
const LAST_QTY_FIELD: &str = stringify!(last_qty);
const LAST_PX_FIELD: &str = stringify!(last_px);
const CURRENCY_FIELD: &str = stringify!(currency);
const AVG_PX_OPEN_FIELD: &str = stringify!(avg_px_open);
const AVG_PX_CLOSE_FIELD: &str = stringify!(avg_px_close);
const REALIZED_RETURN_FIELD: &str = stringify!(realized_return);
const REALIZED_PNL_FIELD: &str = stringify!(realized_pnl);
const UNREALIZED_PNL_FIELD: &str = stringify!(unrealized_pnl);
const DURATION_FIELD: &str = stringify!(duration);
const TS_OPENED_FIELD: &str = stringify!(ts_opened);
const TS_CLOSED_FIELD: &str = stringify!(ts_closed);
const TS_EVENT_FIELD: &str = stringify!(ts_event);
const TS_INIT_FIELD: &str = stringify!(ts_init);
const PAYLOAD_JSON_FIELD: &str = stringify!(payload_json);
const POSITION_ID_FIELD: &str = stringify!(position_id);
const OPENING_ORDER_ID_FIELD: &str = stringify!(opening_order_id);
const CLOSING_ORDER_ID_FIELD: &str = stringify!(closing_order_id);
const ADJUSTMENT_TYPE_FIELD: &str = stringify!(adjustment_type);
const QUANTITY_CHANGE_FIELD: &str = stringify!(quantity_change);
const PNL_CHANGE_FIELD: &str = stringify!(pnl_change);
const REASON_FIELD: &str = stringify!(reason);

macro_rules! json_payload {
    ({ $( $key:expr => $value:expr ),+ $(,)? }) => {{
        let mut object = serde_json::Map::new();
        $(
            object.insert($key.to_string(), json!($value));
        )+
        serde_json::Value::Object(object)
    }};
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderEventRow {
    pub event_type: String,
    pub strategy_id: String,
    pub instrument_id: String,
    pub client_order_id: String,
    pub venue_order_id: Option<String>,
    pub account_id: Option<String>,
    pub ts_event: u64,
    pub ts_init: u64,
    pub payload_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PositionEventRow {
    pub event_type: String,
    pub trader_id: String,
    pub strategy_id: String,
    pub instrument_id: String,
    pub position_id: String,
    pub account_id: String,
    pub event_id: Option<String>,
    pub opening_order_id: Option<String>,
    pub closing_order_id: Option<String>,
    pub side: Option<String>,
    pub quantity: Option<String>,
    pub ts_opened: Option<u64>,
    pub ts_closed: Option<u64>,
    pub realized_pnl: Option<String>,
    pub unrealized_pnl: Option<String>,
    pub ts_event: u64,
    pub ts_init: u64,
    pub payload_json: String,
}

pub fn order_events_path(spool_root: &Path) -> PathBuf {
    spool_root.join(ORDER_EVENTS_CLASS).join(EVENTS_JSONL_FILE)
}

pub fn position_events_path(spool_root: &Path) -> PathBuf {
    spool_root
        .join(POSITION_EVENTS_CLASS)
        .join(EVENTS_JSONL_FILE)
}

pub fn order_event_row(event: &OrderEventAny) -> Result<OrderEventRow> {
    let boxed = event.clone().into_boxed();
    Ok(OrderEventRow {
        event_type: format!("{:?}", event.event_type()),
        strategy_id: event.strategy_id().to_string(),
        instrument_id: event.instrument_id().to_string(),
        client_order_id: event.client_order_id().to_string(),
        venue_order_id: event.venue_order_id().map(|value| value.to_string()),
        account_id: event.account_id().map(|value| value.to_string()),
        ts_event: event.ts_event().as_u64(),
        ts_init: boxed.ts_init().as_u64(),
        payload_json: serde_json::to_string(event)?,
    })
}

pub fn position_event_row(event: &PositionEvent) -> Result<PositionEventRow> {
    // PositionEvent does not offer a serde-backed canonical payload in the same way
    // OrderEventAny does, so this field intentionally stores a lossless-enough summary.
    match event {
        PositionEvent::PositionOpened(opened) => Ok(PositionEventRow {
            event_type: POSITION_OPENED_EVENT_TYPE.to_string(),
            trader_id: opened.trader_id.to_string(),
            strategy_id: opened.strategy_id.to_string(),
            instrument_id: opened.instrument_id.to_string(),
            position_id: opened.position_id.to_string(),
            account_id: opened.account_id.to_string(),
            event_id: Some(opened.event_id.to_string()),
            opening_order_id: Some(opened.opening_order_id.to_string()),
            closing_order_id: None,
            side: Some(format!("{:?}", opened.side).to_uppercase()),
            quantity: Some(opened.quantity.to_string()),
            ts_opened: Some(opened.ts_event.as_u64()),
            ts_closed: None,
            realized_pnl: None,
            unrealized_pnl: None,
            ts_event: opened.ts_event.as_u64(),
            ts_init: opened.ts_init.as_u64(),
            payload_json: json_payload!({
                EVENT_TYPE_FIELD => POSITION_OPENED_EVENT_TYPE,
                TRADER_ID_FIELD => opened.trader_id.to_string(),
                EVENT_ID_FIELD => opened.event_id.to_string(),
                ENTRY_FIELD => format!("{:?}", opened.entry).to_uppercase(),
                SIDE_FIELD => format!("{:?}", opened.side).to_uppercase(),
                SIGNED_QTY_FIELD => opened.signed_qty,
                QUANTITY_FIELD => opened.quantity.to_string(),
                LAST_QTY_FIELD => opened.last_qty.to_string(),
                LAST_PX_FIELD => opened.last_px.to_string(),
                CURRENCY_FIELD => opened.currency.to_string(),
                AVG_PX_OPEN_FIELD => opened.avg_px_open,
                TS_EVENT_FIELD => opened.ts_event.as_u64(),
                TS_INIT_FIELD => opened.ts_init.as_u64(),
            })
            .to_string(),
        }),
        PositionEvent::PositionChanged(changed) => Ok(PositionEventRow {
            event_type: POSITION_CHANGED_EVENT_TYPE.to_string(),
            trader_id: changed.trader_id.to_string(),
            strategy_id: changed.strategy_id.to_string(),
            instrument_id: changed.instrument_id.to_string(),
            position_id: changed.position_id.to_string(),
            account_id: changed.account_id.to_string(),
            event_id: Some(changed.event_id.to_string()),
            opening_order_id: Some(changed.opening_order_id.to_string()),
            closing_order_id: None,
            side: Some(format!("{:?}", changed.side).to_uppercase()),
            quantity: Some(changed.quantity.to_string()),
            ts_opened: Some(changed.ts_opened.as_u64()),
            ts_closed: None,
            realized_pnl: changed.realized_pnl.map(|value| value.to_string()),
            unrealized_pnl: Some(changed.unrealized_pnl.to_string()),
            ts_event: changed.ts_event.as_u64(),
            ts_init: changed.ts_init.as_u64(),
            payload_json: json_payload!({
                EVENT_TYPE_FIELD => POSITION_CHANGED_EVENT_TYPE,
                TRADER_ID_FIELD => changed.trader_id.to_string(),
                EVENT_ID_FIELD => changed.event_id.to_string(),
                ENTRY_FIELD => format!("{:?}", changed.entry).to_uppercase(),
                SIDE_FIELD => format!("{:?}", changed.side).to_uppercase(),
                SIGNED_QTY_FIELD => changed.signed_qty,
                QUANTITY_FIELD => changed.quantity.to_string(),
                PEAK_QUANTITY_FIELD => changed.peak_quantity.to_string(),
                LAST_QTY_FIELD => changed.last_qty.to_string(),
                LAST_PX_FIELD => changed.last_px.to_string(),
                CURRENCY_FIELD => changed.currency.to_string(),
                AVG_PX_OPEN_FIELD => changed.avg_px_open,
                AVG_PX_CLOSE_FIELD => changed.avg_px_close,
                REALIZED_RETURN_FIELD => changed.realized_return,
                REALIZED_PNL_FIELD => changed.realized_pnl.map(|value| value.to_string()),
                UNREALIZED_PNL_FIELD => changed.unrealized_pnl.to_string(),
                TS_OPENED_FIELD => changed.ts_opened.as_u64(),
                TS_EVENT_FIELD => changed.ts_event.as_u64(),
                TS_INIT_FIELD => changed.ts_init.as_u64(),
            })
            .to_string(),
        }),
        PositionEvent::PositionClosed(closed) => Ok(PositionEventRow {
            event_type: POSITION_CLOSED_EVENT_TYPE.to_string(),
            trader_id: closed.trader_id.to_string(),
            strategy_id: closed.strategy_id.to_string(),
            instrument_id: closed.instrument_id.to_string(),
            position_id: closed.position_id.to_string(),
            account_id: closed.account_id.to_string(),
            event_id: Some(closed.event_id.to_string()),
            opening_order_id: Some(closed.opening_order_id.to_string()),
            closing_order_id: closed.closing_order_id.map(|value| value.to_string()),
            side: Some(format!("{:?}", closed.side).to_uppercase()),
            quantity: Some(closed.quantity.to_string()),
            ts_opened: Some(closed.ts_opened.as_u64()),
            ts_closed: closed.ts_closed.map(|value| value.as_u64()),
            realized_pnl: closed.realized_pnl.map(|value| value.to_string()),
            unrealized_pnl: Some(closed.unrealized_pnl.to_string()),
            ts_event: closed.ts_event.as_u64(),
            ts_init: closed.ts_init.as_u64(),
            payload_json: json_payload!({
                EVENT_TYPE_FIELD => POSITION_CLOSED_EVENT_TYPE,
                TRADER_ID_FIELD => closed.trader_id.to_string(),
                EVENT_ID_FIELD => closed.event_id.to_string(),
                ENTRY_FIELD => format!("{:?}", closed.entry).to_uppercase(),
                SIDE_FIELD => format!("{:?}", closed.side).to_uppercase(),
                SIGNED_QTY_FIELD => closed.signed_qty,
                QUANTITY_FIELD => closed.quantity.to_string(),
                PEAK_QUANTITY_FIELD => closed.peak_quantity.to_string(),
                LAST_QTY_FIELD => closed.last_qty.to_string(),
                LAST_PX_FIELD => closed.last_px.to_string(),
                CURRENCY_FIELD => closed.currency.to_string(),
                AVG_PX_OPEN_FIELD => closed.avg_px_open,
                AVG_PX_CLOSE_FIELD => closed.avg_px_close,
                REALIZED_RETURN_FIELD => closed.realized_return,
                REALIZED_PNL_FIELD => closed.realized_pnl.map(|value| value.to_string()),
                UNREALIZED_PNL_FIELD => closed.unrealized_pnl.to_string(),
                DURATION_FIELD => closed.duration,
                TS_OPENED_FIELD => closed.ts_opened.as_u64(),
                TS_CLOSED_FIELD => closed.ts_closed.map(|value| value.as_u64()),
                TS_EVENT_FIELD => closed.ts_event.as_u64(),
                TS_INIT_FIELD => closed.ts_init.as_u64(),
            })
            .to_string(),
        }),
        PositionEvent::PositionAdjusted(adjusted) => Ok(PositionEventRow {
            event_type: POSITION_ADJUSTED_EVENT_TYPE.to_string(),
            trader_id: adjusted.trader_id.to_string(),
            strategy_id: adjusted.strategy_id.to_string(),
            instrument_id: adjusted.instrument_id.to_string(),
            position_id: adjusted.position_id.to_string(),
            account_id: adjusted.account_id.to_string(),
            event_id: Some(adjusted.event_id.to_string()),
            opening_order_id: None,
            closing_order_id: None,
            side: None,
            quantity: adjusted.quantity_change.map(|value| value.to_string()),
            ts_opened: None,
            ts_closed: None,
            realized_pnl: adjusted.pnl_change.map(|value| value.to_string()),
            unrealized_pnl: None,
            ts_event: adjusted.ts_event.as_u64(),
            ts_init: adjusted.ts_init.as_u64(),
            payload_json: json_payload!({
                EVENT_TYPE_FIELD => POSITION_ADJUSTED_EVENT_TYPE,
                TRADER_ID_FIELD => adjusted.trader_id.to_string(),
                EVENT_ID_FIELD => adjusted.event_id.to_string(),
                ADJUSTMENT_TYPE_FIELD => format!("{:?}", adjusted.adjustment_type),
                QUANTITY_CHANGE_FIELD => adjusted.quantity_change.map(|value| value.to_string()),
                PNL_CHANGE_FIELD => adjusted.pnl_change.map(|value| value.to_string()),
                REASON_FIELD => adjusted.reason.map(|value| value.to_string()),
                TS_EVENT_FIELD => adjusted.ts_event.as_u64(),
                TS_INIT_FIELD => adjusted.ts_init.as_u64(),
            })
            .to_string(),
        }),
    }
}

pub fn convert_sidecars_to_parquet(
    source_instance_dir: &Path,
    output_root: &Path,
) -> Result<Vec<&'static str>> {
    let mut converted = Vec::new();

    if convert_order_events_to_parquet(source_instance_dir, output_root)? {
        converted.push(ORDER_EVENTS_CLASS);
    }
    if convert_position_events_to_parquet(source_instance_dir, output_root)? {
        converted.push(POSITION_EVENTS_CLASS);
    }

    Ok(converted)
}

fn convert_order_events_to_parquet(source_instance_dir: &Path, output_root: &Path) -> Result<bool> {
    let source_path = order_events_path(source_instance_dir);
    if !source_path.is_file() {
        return Ok(false);
    }

    let rows: Vec<OrderEventRow> = read_jsonl_rows(&source_path)?;
    if rows.is_empty() {
        return Ok(false);
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new(EVENT_TYPE_FIELD, DataType::Utf8, false),
        Field::new(STRATEGY_ID_FIELD, DataType::Utf8, false),
        Field::new(INSTRUMENT_ID_FIELD, DataType::Utf8, false),
        Field::new(CLIENT_ORDER_ID_FIELD, DataType::Utf8, false),
        Field::new(VENUE_ORDER_ID_FIELD, DataType::Utf8, true),
        Field::new(ACCOUNT_ID_FIELD, DataType::Utf8, true),
        Field::new(TS_EVENT_FIELD, DataType::UInt64, false),
        Field::new(TS_INIT_FIELD, DataType::UInt64, false),
        Field::new(PAYLOAD_JSON_FIELD, DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.event_type.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.strategy_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.instrument_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.client_order_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.venue_order_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.account_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.ts_event).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.ts_init).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.payload_json.as_str())
                    .collect::<Vec<_>>(),
            )),
        ],
    )?;

    write_record_batch(
        batch,
        &output_root
            .join(DATA_DIR)
            .join(ORDER_EVENTS_CLASS)
            .join(PART_ZERO_PARQUET_FILE),
    )?;
    Ok(true)
}

fn convert_position_events_to_parquet(
    source_instance_dir: &Path,
    output_root: &Path,
) -> Result<bool> {
    let source_path = position_events_path(source_instance_dir);
    if !source_path.is_file() {
        return Ok(false);
    }

    let rows: Vec<PositionEventRow> = read_jsonl_rows(&source_path)?;
    if rows.is_empty() {
        return Ok(false);
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new(EVENT_TYPE_FIELD, DataType::Utf8, false),
        Field::new(TRADER_ID_FIELD, DataType::Utf8, false),
        Field::new(STRATEGY_ID_FIELD, DataType::Utf8, false),
        Field::new(INSTRUMENT_ID_FIELD, DataType::Utf8, false),
        Field::new(POSITION_ID_FIELD, DataType::Utf8, false),
        Field::new(ACCOUNT_ID_FIELD, DataType::Utf8, false),
        Field::new(EVENT_ID_FIELD, DataType::Utf8, true),
        Field::new(OPENING_ORDER_ID_FIELD, DataType::Utf8, true),
        Field::new(CLOSING_ORDER_ID_FIELD, DataType::Utf8, true),
        Field::new(SIDE_FIELD, DataType::Utf8, true),
        Field::new(QUANTITY_FIELD, DataType::Utf8, true),
        Field::new(TS_OPENED_FIELD, DataType::UInt64, true),
        Field::new(TS_CLOSED_FIELD, DataType::UInt64, true),
        Field::new(REALIZED_PNL_FIELD, DataType::Utf8, true),
        Field::new(UNREALIZED_PNL_FIELD, DataType::Utf8, true),
        Field::new(TS_EVENT_FIELD, DataType::UInt64, false),
        Field::new(TS_INIT_FIELD, DataType::UInt64, false),
        Field::new(PAYLOAD_JSON_FIELD, DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.event_type.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.trader_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.strategy_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.instrument_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.position_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.account_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.event_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.opening_order_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.closing_order_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.side.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.quantity.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.ts_opened).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.ts_closed).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.realized_pnl.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.unrealized_pnl.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.ts_event).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.ts_init).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.payload_json.as_str())
                    .collect::<Vec<_>>(),
            )),
        ],
    )?;

    write_record_batch(
        batch,
        &output_root
            .join(DATA_DIR)
            .join(POSITION_EVENTS_CLASS)
            .join(PART_ZERO_PARQUET_FILE),
    )?;
    Ok(true)
}

fn read_jsonl_rows<T>(path: &Path) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        rows.push(serde_json::from_str(&line)?);
    }
    Ok(rows)
}

fn write_record_batch(batch: RecordBatch, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None)?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(())
}
