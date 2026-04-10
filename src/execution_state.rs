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

pub const ORDER_EVENTS_CLASS: &str = "order_events";
pub const POSITION_EVENTS_CLASS: &str = "position_events";

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
    spool_root.join(ORDER_EVENTS_CLASS).join("events.jsonl")
}

pub fn position_events_path(spool_root: &Path) -> PathBuf {
    spool_root.join(POSITION_EVENTS_CLASS).join("events.jsonl")
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
    match event {
        PositionEvent::PositionOpened(opened) => Ok(PositionEventRow {
            event_type: "PositionOpened".to_string(),
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
            payload_json: json!({
                "event_type": "PositionOpened",
                "event_id": opened.event_id.to_string(),
                "entry": format!("{:?}", opened.entry).to_uppercase(),
                "side": format!("{:?}", opened.side).to_uppercase(),
                "signed_qty": opened.signed_qty,
                "quantity": opened.quantity.to_string(),
                "last_qty": opened.last_qty.to_string(),
                "last_px": opened.last_px.to_string(),
                "currency": opened.currency.to_string(),
                "avg_px_open": opened.avg_px_open,
                "ts_event": opened.ts_event.as_u64(),
                "ts_init": opened.ts_init.as_u64(),
            })
            .to_string(),
        }),
        PositionEvent::PositionChanged(changed) => Ok(PositionEventRow {
            event_type: "PositionChanged".to_string(),
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
            payload_json: json!({
                "event_type": "PositionChanged",
                "event_id": changed.event_id.to_string(),
                "entry": format!("{:?}", changed.entry).to_uppercase(),
                "side": format!("{:?}", changed.side).to_uppercase(),
                "signed_qty": changed.signed_qty,
                "quantity": changed.quantity.to_string(),
                "peak_quantity": changed.peak_quantity.to_string(),
                "last_qty": changed.last_qty.to_string(),
                "last_px": changed.last_px.to_string(),
                "currency": changed.currency.to_string(),
                "avg_px_open": changed.avg_px_open,
                "avg_px_close": changed.avg_px_close,
                "realized_return": changed.realized_return,
                "realized_pnl": changed.realized_pnl.map(|value| value.to_string()),
                "unrealized_pnl": changed.unrealized_pnl.to_string(),
                "ts_opened": changed.ts_opened.as_u64(),
                "ts_event": changed.ts_event.as_u64(),
                "ts_init": changed.ts_init.as_u64(),
            })
            .to_string(),
        }),
        PositionEvent::PositionClosed(closed) => Ok(PositionEventRow {
            event_type: "PositionClosed".to_string(),
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
            payload_json: json!({
                "event_type": "PositionClosed",
                "event_id": closed.event_id.to_string(),
                "entry": format!("{:?}", closed.entry).to_uppercase(),
                "side": format!("{:?}", closed.side).to_uppercase(),
                "signed_qty": closed.signed_qty,
                "quantity": closed.quantity.to_string(),
                "peak_quantity": closed.peak_quantity.to_string(),
                "last_qty": closed.last_qty.to_string(),
                "last_px": closed.last_px.to_string(),
                "currency": closed.currency.to_string(),
                "avg_px_open": closed.avg_px_open,
                "avg_px_close": closed.avg_px_close,
                "realized_return": closed.realized_return,
                "realized_pnl": closed.realized_pnl.map(|value| value.to_string()),
                "unrealized_pnl": closed.unrealized_pnl.to_string(),
                "duration": closed.duration,
                "ts_opened": closed.ts_opened.as_u64(),
                "ts_closed": closed.ts_closed.map(|value| value.as_u64()),
                "ts_event": closed.ts_event.as_u64(),
                "ts_init": closed.ts_init.as_u64(),
            })
            .to_string(),
        }),
        PositionEvent::PositionAdjusted(adjusted) => Ok(PositionEventRow {
            event_type: "PositionAdjusted".to_string(),
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
            payload_json: json!({
                "event_type": "PositionAdjusted",
                "event_id": adjusted.event_id.to_string(),
                "adjustment_type": format!("{:?}", adjusted.adjustment_type),
                "quantity_change": adjusted.quantity_change.map(|value| value.to_string()),
                "pnl_change": adjusted.pnl_change.map(|value| value.to_string()),
                "reason": adjusted.reason.map(|value| value.to_string()),
                "ts_event": adjusted.ts_event.as_u64(),
                "ts_init": adjusted.ts_init.as_u64(),
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
        Field::new("event_type", DataType::Utf8, false),
        Field::new("strategy_id", DataType::Utf8, false),
        Field::new("instrument_id", DataType::Utf8, false),
        Field::new("client_order_id", DataType::Utf8, false),
        Field::new("venue_order_id", DataType::Utf8, true),
        Field::new("account_id", DataType::Utf8, true),
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
        Field::new("payload_json", DataType::Utf8, false),
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
            .join("data")
            .join(ORDER_EVENTS_CLASS)
            .join("part-0.parquet"),
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
        Field::new("event_type", DataType::Utf8, false),
        Field::new("strategy_id", DataType::Utf8, false),
        Field::new("instrument_id", DataType::Utf8, false),
        Field::new("position_id", DataType::Utf8, false),
        Field::new("account_id", DataType::Utf8, false),
        Field::new("event_id", DataType::Utf8, true),
        Field::new("opening_order_id", DataType::Utf8, true),
        Field::new("closing_order_id", DataType::Utf8, true),
        Field::new("side", DataType::Utf8, true),
        Field::new("quantity", DataType::Utf8, true),
        Field::new("ts_opened", DataType::UInt64, true),
        Field::new("ts_closed", DataType::UInt64, true),
        Field::new("realized_pnl", DataType::Utf8, true),
        Field::new("unrealized_pnl", DataType::Utf8, true),
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
        Field::new("payload_json", DataType::Utf8, false),
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
            .join("data")
            .join(POSITION_EVENTS_CLASS)
            .join("part-0.parquet"),
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
