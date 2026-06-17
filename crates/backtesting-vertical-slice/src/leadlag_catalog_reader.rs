use std::{collections::BTreeMap, fs, path::Path};

use ahash::AHashMap;
use anyhow::{Context, Result, bail};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    enums::BookType,
    orderbook::OrderBook,
};
use serde::{Deserialize, Serialize};

use crate::research_reader::{CatalogQuerySpec, query_catalog_typed};

const NANOS_PER_MILLISECOND: u64 = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeadLagCatalogReadConfig {
    pub catalog_uri: String,
    pub storage_options: Option<AHashMap<String, String>>,
    pub instrument_ids: Vec<String>,
    pub start: Option<UnixNanos>,
    pub end: Option<UnixNanos>,
    pub where_clause: Option<String>,
    pub files: Option<Vec<String>>,
    pub optimize_file_loading: bool,
    pub book_type: LeadLagCatalogBookType,
    pub clock: LeadLagCatalogClock,
    pub instrument_aliases: Vec<LeadLagInstrumentAlias>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct LeadLagInstrumentAlias {
    pub instrument_id: String,
    pub asset_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeadLagCatalogBookType {
    L1Mbp,
    L2Mbp,
    L3Mbo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeadLagCatalogClock {
    TsEvent,
    TsInit,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LeadLagTopOfBookRow {
    pub asset_id: String,
    pub instrument_id: String,
    pub ts_ms: u64,
    pub ts_venue_ms: u64,
    pub best_bid: f64,
    pub best_ask: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LeadLagTradeRow {
    pub asset_id: String,
    pub instrument_id: String,
    pub ts_ms: u64,
    pub ts_venue_ms: u64,
    pub price: f64,
    pub size: f64,
    pub side: String,
}

#[derive(Deserialize)]
struct LeadLagCatalogReadToml {
    catalog_uri: String,
    #[serde(default)]
    storage_options: BTreeMap<String, String>,
    instrument_ids: Vec<String>,
    start_unix_nanos: Option<u64>,
    end_unix_nanos: Option<u64>,
    where_clause: Option<String>,
    files: Option<Vec<String>>,
    optimize_file_loading: bool,
    book_type: String,
    clock: String,
    instrument_aliases: Vec<LeadLagInstrumentAlias>,
}

impl LeadLagCatalogReadConfig {
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("read lead-lag catalog config {}", path.display()))?;
        Self::from_toml_str(&text)
            .with_context(|| format!("parse lead-lag catalog config {}", path.display()))
    }

    pub fn from_toml_str(text: &str) -> Result<Self> {
        let raw: LeadLagCatalogReadToml =
            toml::from_str(text).context("decode lead-lag catalog TOML")?;
        Self::from_raw_toml(raw)
    }

    fn query_spec(&self) -> CatalogQuerySpec {
        CatalogQuerySpec {
            catalog_uri: self.catalog_uri.clone(),
            storage_options: self.storage_options.clone(),
            instrument_ids: Some(self.instrument_ids.clone()),
            start: self.start,
            end: self.end,
            where_clause: self.where_clause.clone(),
            files: self.files.clone(),
            optimize_file_loading: self.optimize_file_loading,
        }
    }

    fn alias_map(&self) -> Result<AHashMap<String, String>> {
        let mut aliases = AHashMap::new();
        for alias in &self.instrument_aliases {
            if alias.instrument_id.trim().is_empty() {
                bail!("lead-lag catalog alias missing instrument_id");
            }
            if alias.asset_id.trim().is_empty() {
                bail!("lead-lag catalog alias missing asset_id");
            }
            if aliases
                .insert(alias.instrument_id.clone(), alias.asset_id.clone())
                .is_some()
            {
                bail!(
                    "lead-lag catalog alias duplicated for instrument_id {}",
                    alias.instrument_id
                );
            }
        }
        for instrument_id in &self.instrument_ids {
            if !aliases.contains_key(instrument_id) {
                bail!("lead-lag catalog alias missing for instrument_id {instrument_id}");
            }
        }
        Ok(aliases)
    }
    fn from_raw_toml(raw: LeadLagCatalogReadToml) -> Result<Self> {
        Ok(Self {
            catalog_uri: raw.catalog_uri,
            storage_options: if raw.storage_options.is_empty() {
                None
            } else {
                Some(raw.storage_options.into_iter().collect())
            },
            instrument_ids: raw.instrument_ids,
            start: raw.start_unix_nanos.map(UnixNanos::from),
            end: raw.end_unix_nanos.map(UnixNanos::from),
            where_clause: raw.where_clause,
            files: raw.files,
            optimize_file_loading: raw.optimize_file_loading,
            book_type: LeadLagCatalogBookType::parse(&raw.book_type)?,
            clock: LeadLagCatalogClock::parse(&raw.clock)?,
            instrument_aliases: raw.instrument_aliases,
        })
    }
}

impl LeadLagCatalogBookType {
    fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_uppercase().as_str() {
            "L1_MBP" => Ok(Self::L1Mbp),
            "L2_MBP" => Ok(Self::L2Mbp),
            "L3_MBO" => Ok(Self::L3Mbo),
            other => bail!("unsupported lead-lag catalog book_type {other:?}"),
        }
    }

    fn to_nt(self) -> BookType {
        match self {
            Self::L1Mbp => BookType::L1_MBP,
            Self::L2Mbp => BookType::L2_MBP,
            Self::L3Mbo => BookType::L3_MBO,
        }
    }
}

impl LeadLagCatalogClock {
    fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "ts_event" => Ok(Self::TsEvent),
            "ts_init" => Ok(Self::TsInit),
            other => bail!("unsupported lead-lag catalog clock {other:?}"),
        }
    }

    fn select(self, ts_event: UnixNanos, ts_init: UnixNanos) -> UnixNanos {
        match self {
            Self::TsEvent => ts_event,
            Self::TsInit => ts_init,
        }
    }
}

pub fn read_leadlag_top_of_book_from_catalog(
    config: &LeadLagCatalogReadConfig,
) -> Result<Vec<LeadLagTopOfBookRow>> {
    let aliases = config.alias_map()?;
    let spec = config.query_spec();
    let mut deltas = query_catalog_typed::<OrderBookDelta>(&spec)?;
    deltas.sort_by(|left, right| {
        left.instrument_id
            .to_string()
            .cmp(&right.instrument_id.to_string())
            .then_with(|| left.ts_init.cmp(&right.ts_init))
            .then_with(|| left.ts_event.cmp(&right.ts_event))
            .then_with(|| left.sequence.cmp(&right.sequence))
    });

    let mut grouped: BTreeMap<String, Vec<OrderBookDelta>> = BTreeMap::new();
    for delta in deltas {
        grouped
            .entry(delta.instrument_id.to_string())
            .or_default()
            .push(delta);
    }

    let mut rows = Vec::new();
    for (instrument_id, instrument_deltas) in grouped {
        let asset_id = aliases
            .get(&instrument_id)
            .with_context(|| format!("lead-lag catalog alias missing for {instrument_id}"))?
            .clone();
        if instrument_deltas.is_empty() {
            continue;
        }
        for quote in OrderBook::deltas_to_quotes(config.book_type.to_nt(), &instrument_deltas) {
            rows.push(LeadLagTopOfBookRow {
                asset_id: asset_id.clone(),
                instrument_id: instrument_id.clone(),
                ts_ms: nanos_to_millis(config.clock.select(quote.ts_event, quote.ts_init)),
                ts_venue_ms: nanos_to_millis(quote.ts_event),
                best_bid: quote.bid_price.as_f64(),
                best_ask: quote.ask_price.as_f64(),
            });
        }
    }
    rows.sort_by(|left, right| {
        left.asset_id
            .cmp(&right.asset_id)
            .then_with(|| left.ts_ms.cmp(&right.ts_ms))
            .then_with(|| left.ts_venue_ms.cmp(&right.ts_venue_ms))
    });
    Ok(rows)
}

pub fn read_leadlag_trades_from_catalog(
    config: &LeadLagCatalogReadConfig,
) -> Result<Vec<LeadLagTradeRow>> {
    let aliases = config.alias_map()?;
    let trade_spec = config.query_spec();
    let mut trades = query_catalog_typed::<TradeTick>(&trade_spec)?;
    trades.sort_by(|left, right| {
        left.instrument_id
            .to_string()
            .cmp(&right.instrument_id.to_string())
            .then_with(|| {
                config
                    .clock
                    .select(left.ts_event, left.ts_init)
                    .cmp(&config.clock.select(right.ts_event, right.ts_init))
            })
            .then_with(|| left.ts_event.cmp(&right.ts_event))
            .then_with(|| left.trade_id.to_string().cmp(&right.trade_id.to_string()))
    });

    trades
        .into_iter()
        .map(|trade| {
            let instrument_id = trade.instrument_id.to_string();
            let asset_id = aliases
                .get(&instrument_id)
                .with_context(|| format!("lead-lag catalog alias missing for {instrument_id}"))?
                .clone();
            Ok(LeadLagTradeRow {
                asset_id,
                instrument_id,
                ts_ms: nanos_to_millis(config.clock.select(trade.ts_event, trade.ts_init)),
                ts_venue_ms: nanos_to_millis(trade.ts_event),
                price: trade.price.as_f64(),
                size: trade.size.as_f64(),
                side: trade.aggressor_side.to_string(),
            })
        })
        .collect()
}

fn nanos_to_millis(nanos: UnixNanos) -> u64 {
    nanos.as_u64() / NANOS_PER_MILLISECOND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_without_hardcoded_runtime_identity() {
        let config = LeadLagCatalogReadConfig::from_toml_str(
            r#"
catalog_uri = "s3://example/catalog"
instrument_ids = ["token-a.POLYMARKET"]
start_unix_nanos = 1700000000000000000
end_unix_nanos = 1700000300000000000
where_clause = "ts_init >= 1700000000000000000"
files = ["data.parquet"]
optimize_file_loading = true
book_type = "L2_MBP"
clock = "ts_event"

[storage_options]
aws_region = "us-east-1"

[[instrument_aliases]]
instrument_id = "token-a.POLYMARKET"
asset_id = "token-a"
"#,
        )
        .expect("parse config");

        assert_eq!(config.catalog_uri, "s3://example/catalog");
        assert_eq!(config.instrument_ids, vec!["token-a.POLYMARKET"]);
        assert_eq!(config.book_type, LeadLagCatalogBookType::L2Mbp);
        assert_eq!(config.clock, LeadLagCatalogClock::TsEvent);
        assert_eq!(
            config
                .storage_options
                .as_ref()
                .expect("storage options")
                .get("aws_region"),
            Some(&"us-east-1".to_string())
        );
        assert_eq!(
            config
                .alias_map()
                .expect("aliases")
                .get("token-a.POLYMARKET"),
            Some(&"token-a".to_string())
        );
    }

    #[test]
    fn rejects_missing_instrument_alias() {
        let config = LeadLagCatalogReadConfig {
            catalog_uri: "file:///catalog".to_string(),
            storage_options: None,
            instrument_ids: vec!["missing.POLYMARKET".to_string()],
            start: None,
            end: None,
            where_clause: None,
            files: None,
            optimize_file_loading: true,
            book_type: LeadLagCatalogBookType::L2Mbp,
            clock: LeadLagCatalogClock::TsEvent,
            instrument_aliases: Vec::new(),
        };

        let err = config.alias_map().expect_err("missing alias must fail");

        assert!(err.to_string().contains("missing.POLYMARKET"));
    }

    #[test]
    fn rejects_duplicate_instrument_aliases() {
        let config = LeadLagCatalogReadConfig {
            catalog_uri: "file:///catalog".to_string(),
            storage_options: None,
            instrument_ids: vec!["token-a.POLYMARKET".to_string()],
            start: None,
            end: None,
            where_clause: None,
            files: None,
            optimize_file_loading: true,
            book_type: LeadLagCatalogBookType::L2Mbp,
            clock: LeadLagCatalogClock::TsEvent,
            instrument_aliases: vec![
                LeadLagInstrumentAlias {
                    instrument_id: "token-a.POLYMARKET".to_string(),
                    asset_id: "token-a".to_string(),
                },
                LeadLagInstrumentAlias {
                    instrument_id: "token-a.POLYMARKET".to_string(),
                    asset_id: "token-b".to_string(),
                },
            ],
        };

        let err = config
            .alias_map()
            .expect_err("duplicate alias must fail closed");

        assert!(err.to_string().contains("token-a.POLYMARKET"));
    }
}
