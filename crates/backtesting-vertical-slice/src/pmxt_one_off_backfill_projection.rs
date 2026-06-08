//! PMXT one-off historical rows projected through NautilusTrader Polymarket APIs.
//!
//! PMXT is intentionally scoped as one-off backfill data. This module proves the
//! selected source rows can be transformed into NT-native objects without making
//! PMXT a canonical source-proof input or a reusable venue abstraction.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_polymarket::{
    common::enums::PolymarketOrderSide,
    http::{
        models::GammaMarket,
        parse::{create_instrument_from_def, parse_gamma_market},
    },
    websocket::{
        messages::{
            PolymarketBookLevel, PolymarketBookSnapshot, PolymarketQuote, PolymarketQuotes,
        },
        parse::{parse_book_deltas, parse_book_snapshot},
    },
};
use ustr::Ustr;

use crate::{catalog_projection::logical_catalog_hash, source_proof::SourceProofUsageScope};

#[derive(Debug, Clone)]
pub struct PmxtOneOffProjectionRequest {
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub selected_condition_id: String,
    pub selected_token_id: String,
    pub gamma_markets: Vec<GammaMarket>,
    pub rows: Vec<PmxtOneOffSelectedRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PmxtOneOffSelectedRow {
    BookSnapshot(PmxtOneOffSnapshotRow),
    PriceChange(PmxtPriceChangeRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtOneOffSnapshotRow {
    pub market: String,
    pub asset_id: String,
    pub bids: Vec<PmxtBookLevel>,
    pub asks: Vec<PmxtBookLevel>,
    pub timestamp_ms: String,
    pub ts_init: UnixNanos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtPriceChangeRow {
    pub market: String,
    pub asset_id: String,
    pub price: String,
    pub side: PmxtOneOffTickSide,
    pub size: String,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
    pub timestamp_ms: String,
    pub ts_init: UnixNanos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmxtOneOffTickSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtBookLevel {
    pub price: String,
    pub size: String,
}

#[derive(Debug, Clone)]
pub struct PmxtOneOffNtProjection {
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub instrument: InstrumentAny,
    pub order_book_deltas: Vec<OrderBookDelta>,
    pub trade_ticks: Vec<TradeTick>,
    pub nt_surfaces_used: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtOneOffCatalogProjection {
    pub catalog_root: PathBuf,
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub nt_instrument_id: String,
    pub order_book_delta_count: u64,
    pub trade_tick_count: u64,
    pub catalog_hash: String,
}

pub fn project_pmxt_one_off_rows_to_nt(
    request: PmxtOneOffProjectionRequest,
) -> Result<PmxtOneOffNtProjection> {
    ensure!(
        request.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT one-off projection only accepts one_off_backfill_data usage_scope"
    );

    let selected_def = request
        .gamma_markets
        .iter()
        .map(parse_gamma_market)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .find(|def| {
            def.condition_id.as_str() == request.selected_condition_id
                && def.token_id.as_str() == request.selected_token_id
        })
        .with_context(|| {
            format!(
                "NT Polymarket Gamma parser did not produce selected token {:?} for condition {:?}",
                request.selected_token_id, request.selected_condition_id
            )
        })?;

    let instrument = create_instrument_from_def(&selected_def, UnixNanos::default())
        .context("create NT Polymarket BinaryOption from selected Gamma definition")?;
    let (instrument_id, price_precision, size_precision) = binary_option_l2_metadata(&instrument)?;

    let mut order_book_deltas = Vec::new();
    let trade_ticks = Vec::new();
    let mut nt_surfaces_used = vec![
        "nautilus_polymarket::http::parse::parse_gamma_market".to_string(),
        "nautilus_polymarket::http::parse::create_instrument_from_def".to_string(),
    ];

    for row in request.rows {
        match row {
            PmxtOneOffSelectedRow::BookSnapshot(row) => {
                ensure_selected_row(
                    &row.market,
                    &row.asset_id,
                    &request.selected_condition_id,
                    &request.selected_token_id,
                )?;
                let snapshot = PolymarketBookSnapshot {
                    market: Ustr::from(row.market.as_str()),
                    asset_id: Ustr::from(row.asset_id.as_str()),
                    bids: row.bids.into_iter().map(Into::into).collect(),
                    asks: row.asks.into_iter().map(Into::into).collect(),
                    timestamp: row.timestamp_ms,
                };
                let parsed = parse_book_snapshot(
                    &snapshot,
                    instrument_id,
                    price_precision,
                    size_precision,
                    row.ts_init,
                )
                .context("parse PMXT one-off book snapshot with NT Polymarket parser")?;
                order_book_deltas.extend(parsed.deltas);
                push_surface_once(
                    &mut nt_surfaces_used,
                    "nautilus_polymarket::websocket::parse::parse_book_snapshot",
                );
            }
            PmxtOneOffSelectedRow::PriceChange(row) => {
                ensure_selected_row(
                    &row.market,
                    &row.asset_id,
                    &request.selected_condition_id,
                    &request.selected_token_id,
                )?;
                let quotes = PolymarketQuotes {
                    market: Ustr::from(row.market.as_str()),
                    price_changes: vec![PolymarketQuote {
                        asset_id: Ustr::from(row.asset_id.as_str()),
                        price: row.price,
                        side: row.side.into(),
                        size: row.size,
                        hash: String::new(),
                        best_bid: row.best_bid,
                        best_ask: row.best_ask,
                    }],
                    timestamp: row.timestamp_ms,
                };
                let parsed = parse_book_deltas(
                    &quotes,
                    instrument_id,
                    price_precision,
                    size_precision,
                    row.ts_init,
                )
                .context("parse PMXT one-off price_change with NT Polymarket parser")?;
                order_book_deltas.extend(parsed.deltas);
                push_surface_once(
                    &mut nt_surfaces_used,
                    "nautilus_polymarket::websocket::parse::parse_book_deltas",
                );
            }
        }
    }

    Ok(PmxtOneOffNtProjection {
        source_binding: request.source_binding,
        usage_scope: request.usage_scope,
        instrument,
        order_book_deltas,
        trade_ticks,
        nt_surfaces_used,
    })
}

pub fn write_pmxt_one_off_projection_to_catalog(
    catalog_root: &Path,
    projection: &PmxtOneOffNtProjection,
) -> Result<PmxtOneOffCatalogProjection> {
    ensure!(
        projection.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT one-off catalog projection only accepts one_off_backfill_data usage_scope"
    );
    ensure_clean_catalog_root(catalog_root)?;
    fs::create_dir_all(catalog_root).with_context(|| {
        format!(
            "create PMXT one-off catalog root {}",
            catalog_root.display()
        )
    })?;

    let instrument_id = projection.instrument.id().to_string();
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![projection.instrument.clone()])
        .context("write PMXT one-off instrument to NT catalog")?;
    if !projection.order_book_deltas.is_empty() {
        catalog
            .write_to_parquet(projection.order_book_deltas.clone(), None, None, None)
            .context("write PMXT one-off L2 deltas to NT catalog")?;
    }
    if !projection.trade_ticks.is_empty() {
        catalog
            .write_to_parquet(projection.trade_ticks.clone(), None, None, None)
            .context("write PMXT one-off trade ticks to NT catalog")?;
    }

    Ok(PmxtOneOffCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        source_binding: projection.source_binding.clone(),
        usage_scope: projection.usage_scope,
        nt_instrument_id: instrument_id,
        order_book_delta_count: projection.order_book_deltas.len() as u64,
        trade_tick_count: projection.trade_ticks.len() as u64,
        catalog_hash: logical_catalog_hash(catalog_root)?,
    })
}

fn binary_option_l2_metadata(instrument: &InstrumentAny) -> Result<(InstrumentId, u8, u8)> {
    match instrument {
        InstrumentAny::BinaryOption(binary_option) => Ok((
            binary_option.id(),
            binary_option.price_precision(),
            binary_option.size_precision(),
        )),
        other => bail!("expected NT Polymarket parser to produce BinaryOption, got {other:?}"),
    }
}

fn ensure_selected_row(
    market: &str,
    asset_id: &str,
    selected_condition_id: &str,
    selected_token_id: &str,
) -> Result<()> {
    ensure!(
        market == selected_condition_id,
        "PMXT one-off row market {market:?} does not match selected condition {selected_condition_id:?}"
    );
    ensure!(
        asset_id == selected_token_id,
        "PMXT one-off row asset_id {asset_id:?} does not match selected token {selected_token_id:?}"
    );
    Ok(())
}

fn ensure_clean_catalog_root(catalog_root: &Path) -> Result<()> {
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    Ok(())
}

fn push_surface_once(surfaces: &mut Vec<String>, surface: &str) {
    if !surfaces.iter().any(|existing| existing == surface) {
        surfaces.push(surface.to_string());
    }
}

impl From<PmxtBookLevel> for PolymarketBookLevel {
    fn from(value: PmxtBookLevel) -> Self {
        Self {
            price: value.price,
            size: value.size,
        }
    }
}

impl From<PmxtOneOffTickSide> for PolymarketOrderSide {
    fn from(value: PmxtOneOffTickSide) -> Self {
        match value {
            PmxtOneOffTickSide::Buy => Self::Buy,
            PmxtOneOffTickSide::Sell => Self::Sell,
        }
    }
}
