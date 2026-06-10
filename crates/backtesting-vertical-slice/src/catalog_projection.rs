//! Gate 3 — NautilusTrader catalog projection.
//!
//! Projects a validated [`CanonicalTradesTable`] into a NautilusTrader
//! `ParquetDataCatalog` as `TradeTick` data plus the venue instrument, using
//! NautilusTrader APIs directly (no custom simulation behaviour), then proves
//! the resolved `bolt-v2` NautilusTrader dependency can read the projection back.
//!
//! The NautilusTrader instrument is built from accepted instrument-universe
//! metadata ([`CatalogInstrumentSpec`]); price/size precision and increments
//! are derived from the source tick size and size precision, never hardcoded.

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, ensure};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    enums::AggressorSide,
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{
        BinaryOption, CryptoFuture, CryptoPerpetual, CurrencyPair, Instrument, InstrumentAny,
    },
    types::{Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_trades::{CanonicalTradesTable, TradeAggressorSide},
    source_proof::SourceProofFidelityClass,
};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// Accepted spot instrument metadata needed to build the NautilusTrader
/// `CurrencyPair`. Built from the accepted instrument-universe payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotInstrumentSpec {
    /// NautilusTrader instrument id, such as `SYMBOL.VENUE`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol.
    pub raw_symbol: String,
    /// Base currency code, for example `BNB`.
    pub base_currency: String,
    /// Quote currency code, for example `USDC`.
    pub quote_currency: String,
    /// Price tick size as a decimal string, for example `0.1`.
    pub price_increment: String,
    /// Base size precision as a decimal string, for example `0.0001`.
    pub size_increment: String,
    /// Minimum order quantity decimal string.
    pub min_quantity: String,
    /// Maximum order quantity decimal string.
    pub max_quantity: String,
    /// Minimum order notional decimal string (quote currency).
    pub min_notional: String,
    /// Maximum order notional decimal string (quote currency).
    pub max_notional: String,
}

/// Instrument spec parsed from run-spec TOML and projected through NT's native
/// instrument constructors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CatalogInstrumentSpec {
    CryptoPerpetual(CryptoPerpetualInstrumentSpec),
    CryptoFuture(CryptoFutureInstrumentSpec),
    Spot(SpotInstrumentSpec),
}

impl CatalogInstrumentSpec {
    #[cfg(test)]
    pub(crate) fn spot_mut(&mut self) -> Option<&mut SpotInstrumentSpec> {
        match self {
            Self::Spot(spec) => Some(spec),
            Self::CryptoPerpetual(_) | Self::CryptoFuture(_) => None,
        }
    }
}

/// TOML discriminator for an NT [`CryptoPerpetual`] instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CryptoPerpetualInstrumentKind {
    CryptoPerpetual,
}

/// Accepted crypto perpetual metadata needed to build NT's
/// [`CryptoPerpetual`] instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoPerpetualInstrumentSpec {
    pub instrument_kind: CryptoPerpetualInstrumentKind,
    pub nt_instrument_id: String,
    pub raw_symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub settlement_currency: String,
    pub is_inverse: bool,
    pub price_increment: String,
    pub size_increment: String,
    pub min_quantity: String,
    pub max_quantity: String,
    pub min_notional: String,
    pub max_notional: String,
    pub multiplier: Option<String>,
    pub lot_size: Option<String>,
    pub max_price: Option<String>,
    pub min_price: Option<String>,
    pub margin_init: Option<String>,
    pub margin_maint: Option<String>,
    pub maker_fee: Option<String>,
    pub taker_fee: Option<String>,
}

/// TOML discriminator for an NT [`CryptoFuture`] instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CryptoFutureInstrumentKind {
    CryptoFuture,
}

/// Accepted crypto future metadata needed to build NT's [`CryptoFuture`]
/// instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoFutureInstrumentSpec {
    pub instrument_kind: CryptoFutureInstrumentKind,
    pub nt_instrument_id: String,
    pub raw_symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub settlement_currency: String,
    pub is_inverse: bool,
    pub activation_time_nanos: u64,
    pub expiration_time_nanos: u64,
    pub price_increment: String,
    pub size_increment: String,
    pub min_quantity: String,
    pub max_quantity: String,
    pub min_notional: String,
    pub max_notional: String,
    pub multiplier: Option<String>,
    pub lot_size: Option<String>,
    pub max_price: Option<String>,
    pub min_price: Option<String>,
    pub margin_init: Option<String>,
    pub margin_maint: Option<String>,
    pub maker_fee: Option<String>,
    pub taker_fee: Option<String>,
}

/// A source of accepted metadata that can build one NT instrument.
pub trait CatalogInstrumentSpecSource {
    /// Build the native NT instrument variant for this spec.
    ///
    /// # Errors
    ///
    /// Returns an error if any field fails to parse or violates NT instrument
    /// correctness checks.
    fn build_instrument_any(&self) -> Result<InstrumentAny>;
}

impl CatalogInstrumentSpecSource for SpotInstrumentSpec {
    fn build_instrument_any(&self) -> Result<InstrumentAny> {
        Ok(InstrumentAny::CurrencyPair(build_currency_pair(self)?))
    }
}

impl CatalogInstrumentSpecSource for CatalogInstrumentSpec {
    fn build_instrument_any(&self) -> Result<InstrumentAny> {
        match self {
            Self::Spot(spec) => spec.build_instrument_any(),
            Self::CryptoPerpetual(spec) => Ok(InstrumentAny::CryptoPerpetual(
                build_crypto_perpetual(spec)?,
            )),
            Self::CryptoFuture(spec) => Ok(InstrumentAny::CryptoFuture(build_crypto_future(spec)?)),
        }
    }
}

/// Result of projecting canonical trades into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub trade_count: usize,
    /// Deterministic SHA-256 hex over the catalog's written data files.
    pub catalog_hash: String,
    pub fidelity_class: SourceProofFidelityClass,
}

/// Build the NautilusTrader `CurrencyPair` from accepted instrument metadata.
///
/// Every NautilusTrader constructor on this path is routed through its checked
/// (`*_checked`) variant so malformed accepted metadata surfaces as an error,
/// never a panic.
///
/// # Errors
///
/// Returns an error if any field fails to parse or fails NautilusTrader's
/// instrument correctness checks.
pub fn build_currency_pair(spec: &SpotInstrumentSpec) -> Result<CurrencyPair> {
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let raw_symbol = Symbol::new_checked(&spec.raw_symbol)
        .map_err(|error| anyhow::anyhow!("invalid raw_symbol {:?}: {error}", spec.raw_symbol))?;
    let base_currency = parse_venue_currency(&spec.base_currency, "base_currency")?;
    let quote_currency = parse_venue_currency(&spec.quote_currency, "quote_currency")?;
    let price_increment = Price::from_str(&spec.price_increment).map_err(|error| {
        anyhow::anyhow!(
            "invalid price_increment {:?}: {error}",
            spec.price_increment
        )
    })?;
    let size_increment = Quantity::from_str(&spec.size_increment).map_err(|error| {
        anyhow::anyhow!("invalid size_increment {:?}: {error}", spec.size_increment)
    })?;
    // Single source of precision: the parsed increment. Deriving precision any
    // other way (for example a decimal-string char count) can disagree with the
    // precision NautilusTrader infers from the same value — `Price::from_str`
    // even accepts scientific notation — and panic `CurrencyPair::new_checked`'s
    // precision-equality check.
    let price_precision = price_increment.precision;
    let size_precision = size_increment.precision;
    let max_quantity = Quantity::from_str(&spec.max_quantity).map_err(|error| {
        anyhow::anyhow!("invalid max_quantity {:?}: {error}", spec.max_quantity)
    })?;
    let min_quantity = Quantity::from_str(&spec.min_quantity).map_err(|error| {
        anyhow::anyhow!("invalid min_quantity {:?}: {error}", spec.min_quantity)
    })?;
    let max_notional = Money::new_checked(
        spec.max_notional.parse().context("max_notional")?,
        quote_currency,
    )
    .map_err(|error| anyhow::anyhow!("invalid max_notional {:?}: {error}", spec.max_notional))?;
    let min_notional = Money::new_checked(
        spec.min_notional.parse().context("min_notional")?,
        quote_currency,
    )
    .map_err(|error| anyhow::anyhow!("invalid min_notional {:?}: {error}", spec.min_notional))?;

    CurrencyPair::new_checked(
        instrument_id,
        raw_symbol,
        base_currency,
        quote_currency,
        price_precision,
        size_precision,
        price_increment,
        size_increment,
        None,
        None,
        Some(max_quantity),
        Some(min_quantity),
        Some(max_notional),
        Some(min_notional),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid currency pair for {:?}: {error}",
            spec.nt_instrument_id
        )
    })
}

fn parse_instrument_id(value: &str) -> Result<InstrumentId> {
    InstrumentId::from_str(value).with_context(|| format!("invalid nt_instrument_id {value:?}"))
}

fn parse_raw_symbol(value: &str) -> Result<Symbol> {
    Symbol::new_checked(value)
        .map_err(|error| anyhow::anyhow!("invalid raw_symbol {value:?}: {error}"))
}

fn parse_venue_currency(value: &str, label: &str) -> Result<Currency> {
    let code = value.trim();
    ensure!(!code.is_empty(), "{label} must not be empty");
    Ok(Currency::get_or_create_crypto(code))
}

fn parse_price(value: &str, label: &str) -> Result<Price> {
    Price::from_str(value).map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_quantity(value: &str, label: &str) -> Result<Quantity> {
    Quantity::from_str(value).map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_optional_quantity(value: Option<&str>, label: &str) -> Result<Option<Quantity>> {
    value.map(|value| parse_quantity(value, label)).transpose()
}

fn parse_optional_price(value: Option<&str>, label: &str) -> Result<Option<Price>> {
    value.map(|value| parse_price(value, label)).transpose()
}

fn parse_optional_decimal(value: Option<&str>, label: &str) -> Result<Option<Decimal>> {
    value
        .map(|value| Decimal::from_str(value).with_context(|| format!("invalid {label} {value:?}")))
        .transpose()
}

fn parse_money(value: &str, currency: Currency, label: &str) -> Result<Money> {
    Money::new_checked(
        value
            .parse()
            .with_context(|| format!("invalid {label} {value:?}"))?,
        currency,
    )
    .map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn derivative_common_fields(
    input: DerivativeCommonFieldInput<'_>,
) -> Result<DerivativeCommonFields> {
    let instrument_id = parse_instrument_id(input.nt_instrument_id)?;
    let raw_symbol = parse_raw_symbol(input.raw_symbol)?;
    let price_increment = parse_price(input.price_increment, "price_increment")?;
    let size_increment = parse_quantity(input.size_increment, "size_increment")?;
    Ok(DerivativeCommonFields {
        instrument_id,
        raw_symbol,
        price_precision: price_increment.precision,
        size_precision: size_increment.precision,
        price_increment,
        size_increment,
        min_quantity: parse_quantity(input.min_quantity, "min_quantity")?,
        max_quantity: parse_quantity(input.max_quantity, "max_quantity")?,
        min_notional: parse_money(input.min_notional, input.quote_currency, "min_notional")?,
        max_notional: parse_money(input.max_notional, input.quote_currency, "max_notional")?,
    })
}

struct DerivativeCommonFieldInput<'a> {
    nt_instrument_id: &'a str,
    raw_symbol: &'a str,
    quote_currency: Currency,
    price_increment: &'a str,
    size_increment: &'a str,
    min_quantity: &'a str,
    max_quantity: &'a str,
    min_notional: &'a str,
    max_notional: &'a str,
}

struct DerivativeCommonFields {
    instrument_id: InstrumentId,
    raw_symbol: Symbol,
    price_precision: u8,
    size_precision: u8,
    price_increment: Price,
    size_increment: Quantity,
    min_quantity: Quantity,
    max_quantity: Quantity,
    min_notional: Money,
    max_notional: Money,
}

/// Build the NautilusTrader instrument variant from accepted metadata.
///
/// # Errors
///
/// Returns an error if any field fails to parse or fails NautilusTrader's
/// instrument correctness checks.
pub fn build_catalog_instrument(spec: &CatalogInstrumentSpec) -> Result<InstrumentAny> {
    spec.build_instrument_any()
}

/// Build NT's [`CryptoPerpetual`] from accepted derivative metadata.
///
/// # Errors
///
/// Returns an error if accepted metadata cannot construct a checked NT crypto
/// perpetual.
pub fn build_crypto_perpetual(spec: &CryptoPerpetualInstrumentSpec) -> Result<CryptoPerpetual> {
    let base_currency = parse_venue_currency(&spec.base_currency, "base_currency")?;
    let quote_currency = parse_venue_currency(&spec.quote_currency, "quote_currency")?;
    let settlement_currency =
        parse_venue_currency(&spec.settlement_currency, "settlement_currency")?;
    let common = derivative_common_fields(DerivativeCommonFieldInput {
        nt_instrument_id: &spec.nt_instrument_id,
        raw_symbol: &spec.raw_symbol,
        quote_currency,
        price_increment: &spec.price_increment,
        size_increment: &spec.size_increment,
        min_quantity: &spec.min_quantity,
        max_quantity: &spec.max_quantity,
        min_notional: &spec.min_notional,
        max_notional: &spec.max_notional,
    })?;
    CryptoPerpetual::new_checked(
        common.instrument_id,
        common.raw_symbol,
        base_currency,
        quote_currency,
        settlement_currency,
        spec.is_inverse,
        common.price_precision,
        common.size_precision,
        common.price_increment,
        common.size_increment,
        parse_optional_quantity(spec.multiplier.as_deref(), "multiplier")?,
        parse_optional_quantity(spec.lot_size.as_deref(), "lot_size")?,
        Some(common.max_quantity),
        Some(common.min_quantity),
        Some(common.max_notional),
        Some(common.min_notional),
        parse_optional_price(spec.max_price.as_deref(), "max_price")?,
        parse_optional_price(spec.min_price.as_deref(), "min_price")?,
        parse_optional_decimal(spec.margin_init.as_deref(), "margin_init")?,
        parse_optional_decimal(spec.margin_maint.as_deref(), "margin_maint")?,
        parse_optional_decimal(spec.maker_fee.as_deref(), "maker_fee")?,
        parse_optional_decimal(spec.taker_fee.as_deref(), "taker_fee")?,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid crypto perpetual for {:?}: {error}",
            spec.nt_instrument_id
        )
    })
}

/// Build NT's [`CryptoFuture`] from accepted derivative metadata.
///
/// # Errors
///
/// Returns an error if accepted metadata cannot construct a checked NT crypto
/// future.
pub fn build_crypto_future(spec: &CryptoFutureInstrumentSpec) -> Result<CryptoFuture> {
    let underlying = parse_venue_currency(&spec.base_currency, "base_currency")?;
    let quote_currency = parse_venue_currency(&spec.quote_currency, "quote_currency")?;
    let settlement_currency =
        parse_venue_currency(&spec.settlement_currency, "settlement_currency")?;
    ensure!(
        spec.activation_time_nanos < spec.expiration_time_nanos,
        "activation_time_nanos must be before expiration_time_nanos"
    );
    let common = derivative_common_fields(DerivativeCommonFieldInput {
        nt_instrument_id: &spec.nt_instrument_id,
        raw_symbol: &spec.raw_symbol,
        quote_currency,
        price_increment: &spec.price_increment,
        size_increment: &spec.size_increment,
        min_quantity: &spec.min_quantity,
        max_quantity: &spec.max_quantity,
        min_notional: &spec.min_notional,
        max_notional: &spec.max_notional,
    })?;
    CryptoFuture::new_checked(
        common.instrument_id,
        common.raw_symbol,
        underlying,
        quote_currency,
        settlement_currency,
        spec.is_inverse,
        UnixNanos::from(spec.activation_time_nanos),
        UnixNanos::from(spec.expiration_time_nanos),
        common.price_precision,
        common.size_precision,
        common.price_increment,
        common.size_increment,
        parse_optional_quantity(spec.multiplier.as_deref(), "multiplier")?,
        parse_optional_quantity(spec.lot_size.as_deref(), "lot_size")?,
        Some(common.max_quantity),
        Some(common.min_quantity),
        Some(common.max_notional),
        Some(common.min_notional),
        parse_optional_price(spec.max_price.as_deref(), "max_price")?,
        parse_optional_price(spec.min_price.as_deref(), "min_price")?,
        parse_optional_decimal(spec.margin_init.as_deref(), "margin_init")?,
        parse_optional_decimal(spec.margin_maint.as_deref(), "margin_maint")?,
        parse_optional_decimal(spec.maker_fee.as_deref(), "maker_fee")?,
        parse_optional_decimal(spec.taker_fee.as_deref(), "taker_fee")?,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid crypto future for {:?}: {error}",
            spec.nt_instrument_id
        )
    })
}

fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    decimal.normalize_assign();
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Convert canonical trade rows into NautilusTrader `TradeTick`s at the
/// instrument's price/size precision.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn canonical_rows_to_trade_ticks<I: Instrument + ?Sized>(
    table: &CanonicalTradesTable,
    instrument: &I,
) -> Result<Vec<TradeTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| {
            let price_str = rescaled(&row.price, price_precision)?;
            let price = Price::from_str(&price_str).map_err(|error| {
                anyhow::anyhow!("invalid rescaled price {price_str:?}: {error}")
            })?;
            let size_str = rescaled(&row.size, size_precision)?;
            let size = Quantity::from_str(&size_str)
                .map_err(|error| anyhow::anyhow!("invalid rescaled size {size_str:?}: {error}"))?;
            let aggressor = match row.aggressor_side.as_str() {
                s if s == TradeAggressorSide::Buyer.as_str() => AggressorSide::Buyer,
                s if s == TradeAggressorSide::Seller.as_str() => AggressorSide::Seller,
                other => anyhow::bail!("unknown aggressor side {other:?}"),
            };
            let ts = UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
            let trade_id = TradeId::new_checked(&row.trade_id)
                .map_err(|error| anyhow::anyhow!("invalid trade_id {:?}: {error}", row.trade_id))?;
            Ok(TradeTick::new(
                instrument_id,
                price,
                size,
                aggressor,
                trade_id,
                ts,
                ts,
            ))
        })
        .collect()
}

/// Project a canonical trades table into a NautilusTrader `ParquetDataCatalog`.
///
/// Writes the venue instrument and the `TradeTick` projection under
/// `catalog_root`, then returns a [`CatalogProjection`] with a deterministic
/// catalog hash. NautilusTrader writes its native
/// `data/<data_type>/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_canonical_trades_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalTradesTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    let instrument_id = instrument.id();
    let row_instrument_id = table.rows[0]
        .nt_instrument_id
        .as_deref()
        .context("canonical row missing nt_instrument_id")?;
    ensure!(
        instrument_id.to_string() == row_instrument_id,
        "instrument id {instrument_id} does not match canonical rows {}",
        row_instrument_id
    );
    let ticks = canonical_rows_to_trade_ticks(table, &instrument)?;
    let trade_count = ticks.len();

    // Fail closed on a dirty catalog root. NautilusTrader's `write_to_parquet`
    // skips writing when a file for the same instrument/interval already exists,
    // so projecting into a non-empty root could silently read back stale data
    // under this run's source proof and a stale catalog hash. The caller owns
    // the output lifecycle and must hand us a clean (absent or empty) root.
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        trade_count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `TradeTick` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_trade_ticks(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<TradeTick>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<TradeTick>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query trade ticks from catalog")
}

/// Deterministic SHA-256 hex over the logical NT catalog contents.
///
/// This intentionally hashes NT-read instruments and `TradeTick` values, not
/// raw Parquet bytes or paths. Parquet writer metadata can legitimately drift
/// across NT/Arrow builds while representing identical logical catalog input.
pub(crate) fn logical_catalog_hash(root: &Path) -> Result<String> {
    let mut catalog = ParquetDataCatalog::new(root, None, None, None, None);
    let mut instruments = catalog
        .query_instruments(None)
        .context("query instruments from catalog for logical hash")?;
    instruments.sort_by_key(|instrument| instrument.id().to_string());
    let mut ticks = catalog
        .query_typed_data::<TradeTick>(None, None, None, None, None, true)
        .context("query trade ticks from catalog for logical hash")?;
    ticks.sort_by_key(|tick| {
        (
            tick.ts_event.as_u64(),
            tick.trade_id.to_string(),
            tick.instrument_id.to_string(),
        )
    });
    let mut deltas = catalog
        .query_typed_data::<OrderBookDelta>(None, None, None, None, None, true)
        .context("query order book deltas from catalog for logical hash")?;
    deltas.sort_by_key(|delta| {
        (
            delta.ts_event.as_u64(),
            delta.instrument_id.to_string(),
            delta.sequence,
            delta.action.to_string(),
            delta.order.side.to_string(),
            delta.order.price.as_decimal().to_string(),
            delta.order.size.as_decimal().to_string(),
            delta.order.order_id,
        )
    });

    let mut hasher = Sha256::new();
    hasher.update(b"nautilus-logical-catalog.v1");
    for instrument in instruments {
        hasher.update([0u8]);
        update_instrument_hash(&mut hasher, &instrument)?;
    }
    for tick in ticks {
        hasher.update([2u8]);
        hasher.update(tick.instrument_id.to_string().as_bytes());
        hasher.update([3u8]);
        hasher.update(tick.trade_id.to_string().as_bytes());
        hasher.update([4u8]);
        hasher.update(tick.price.as_decimal().to_string().as_bytes());
        hasher.update([5u8]);
        hasher.update(tick.size.as_decimal().to_string().as_bytes());
        hasher.update([6u8]);
        hasher.update(tick.aggressor_side.to_string().as_bytes());
        hasher.update([7u8]);
        hasher.update(tick.ts_event.as_u64().to_string().as_bytes());
        hasher.update([8u8]);
        hasher.update(tick.ts_init.as_u64().to_string().as_bytes());
    }
    for delta in deltas {
        hasher.update([9u8]);
        hasher.update(delta.instrument_id.to_string().as_bytes());
        hasher.update([10u8]);
        hasher.update(delta.action.to_string().as_bytes());
        hasher.update([11u8]);
        hasher.update(delta.order.side.to_string().as_bytes());
        hasher.update([12u8]);
        hasher.update(delta.order.price.as_decimal().to_string().as_bytes());
        hasher.update([13u8]);
        hasher.update(delta.order.size.as_decimal().to_string().as_bytes());
        hasher.update([14u8]);
        hasher.update(delta.order.order_id.to_string().as_bytes());
        hasher.update([15u8]);
        hasher.update(delta.flags.to_string().as_bytes());
        hasher.update([16u8]);
        hasher.update(delta.sequence.to_string().as_bytes());
        hasher.update([17u8]);
        hasher.update(delta.ts_event.as_u64().to_string().as_bytes());
        hasher.update([18u8]);
        hasher.update(delta.ts_init.as_u64().to_string().as_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

fn update_hash_field(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
    hasher.update([0xff]);
}

fn update_optional_hash_field<T: ToString>(hasher: &mut Sha256, label: &str, value: Option<&T>) {
    match value {
        Some(value) => update_hash_field(hasher, label, &value.to_string()),
        None => update_hash_field(hasher, label, "<none>"),
    }
}

fn update_instrument_hash(hasher: &mut Sha256, instrument: &InstrumentAny) -> Result<()> {
    match instrument {
        InstrumentAny::CurrencyPair(currency_pair) => {
            update_currency_pair_hash(hasher, currency_pair)?
        }
        InstrumentAny::BinaryOption(binary_option) => {
            update_binary_option_hash(hasher, binary_option)?
        }
        InstrumentAny::CryptoPerpetual(crypto_perpetual) => {
            update_crypto_perpetual_hash(hasher, crypto_perpetual)?
        }
        InstrumentAny::CryptoFuture(crypto_future) => {
            update_crypto_future_hash(hasher, crypto_future)?
        }
        other => {
            anyhow::bail!(
                "logical catalog hash does not support instrument type for {}",
                other.id()
            );
        }
    }
    Ok(())
}

fn update_crypto_perpetual_hash(hasher: &mut Sha256, instrument: &CryptoPerpetual) -> Result<()> {
    ensure!(
        instrument.info.is_none(),
        "logical catalog hash does not support opaque crypto perpetual info for {}",
        instrument.id
    );
    update_hash_field(hasher, "instrument.type", "crypto_perpetual");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.base_currency",
        &instrument.base_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.quote_currency",
        &instrument.quote_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.settlement_currency",
        &instrument.settlement_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.is_inverse",
        &instrument.is_inverse.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.multiplier",
        &instrument.multiplier.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.lot_size",
        &instrument.lot_size.as_decimal().to_string(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

fn update_crypto_future_hash(hasher: &mut Sha256, instrument: &CryptoFuture) -> Result<()> {
    ensure!(
        instrument.info.is_none(),
        "logical catalog hash does not support opaque crypto future info for {}",
        instrument.id
    );
    update_hash_field(hasher, "instrument.type", "crypto_future");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.underlying",
        &instrument.underlying.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.quote_currency",
        &instrument.quote_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.settlement_currency",
        &instrument.settlement_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.is_inverse",
        &instrument.is_inverse.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.activation_ns",
        &instrument.activation_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.expiration_ns",
        &instrument.expiration_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.multiplier",
        &instrument.multiplier.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.lot_size",
        &instrument.lot_size.as_decimal().to_string(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

fn update_binary_option_hash(hasher: &mut Sha256, instrument: &BinaryOption) -> Result<()> {
    update_hash_field(hasher, "instrument.type", "binary_option");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.asset_class",
        instrument.asset_class.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.currency",
        &instrument.currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.activation_ns",
        &instrument.activation_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.expiration_ns",
        &instrument.expiration_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_optional_hash_field(hasher, "instrument.outcome", instrument.outcome.as_ref());
    update_optional_hash_field(
        hasher,
        "instrument.description",
        instrument.description.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_optional_params_hash(hasher, "instrument.info", instrument.info.as_ref())?;
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

fn update_optional_params_hash(
    hasher: &mut Sha256,
    label: &str,
    value: Option<&Params>,
) -> Result<()> {
    let Some(params) = value else {
        update_hash_field(hasher, label, "<none>");
        return Ok(());
    };
    update_hash_field(hasher, &format!("{label}.len"), &params.len().to_string());
    let mut entries = params.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(key, _)| key.as_str());
    for (key, value) in entries {
        update_hash_field(hasher, &format!("{label}.key"), key);
        update_hash_field(
            hasher,
            &format!("{label}.value"),
            &serde_json::to_string(value).context("serialize instrument params value")?,
        );
    }
    Ok(())
}

fn update_currency_pair_hash(hasher: &mut Sha256, instrument: &CurrencyPair) -> Result<()> {
    ensure!(
        instrument.info.is_none(),
        "logical catalog hash does not support opaque currency pair info for {}",
        instrument.id
    );
    update_hash_field(hasher, "instrument.type", "currency_pair");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.base_currency",
        &instrument.base_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.quote_currency",
        &instrument.quote_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.multiplier",
        &instrument.multiplier.as_decimal().to_string(),
    );
    update_optional_hash_field(hasher, "instrument.lot_size", instrument.lot_size.as_ref());
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        canonical_trades::{CanonicalInstrumentIdentity, normalize_sample_spot_tick_trades},
        source_proof::{
            AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
            IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus,
            RequiredCheck, RequiredChecks, SourceCandidateClass, SourceProofClaimLimit,
            SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
            TimeRange, select_accepted_dataset,
        },
    };

    fn spec() -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
            raw_symbol: "BNBUSDC".to_string(),
            base_currency: "BNB".to_string(),
            quote_currency: "USDC".to_string(),
            price_increment: "0.1".to_string(),
            size_increment: "0.0001".to_string(),
            min_quantity: "0.0001".to_string(),
            max_quantity: "1400".to_string(),
            min_notional: "5".to_string(),
            max_notional: "200000".to_string(),
        }
    }

    fn linear_perpetual_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoPerpetual(CryptoPerpetualInstrumentSpec {
            instrument_kind: CryptoPerpetualInstrumentKind::CryptoPerpetual,
            nt_instrument_id: "BTCUSDT.BYBIT".to_string(),
            raw_symbol: "BTCUSDT".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USDT".to_string(),
            settlement_currency: "USDT".to_string(),
            is_inverse: false,
            price_increment: "0.1".to_string(),
            size_increment: "0.001".to_string(),
            min_quantity: "0.001".to_string(),
            max_quantity: "1000".to_string(),
            min_notional: "5".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.1".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn linear_future_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoFuture(CryptoFutureInstrumentSpec {
            instrument_kind: CryptoFutureInstrumentKind::CryptoFuture,
            nt_instrument_id: "BTCUSDT-05JUN26.BYBIT".to_string(),
            raw_symbol: "BTCUSDT-05JUN26".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USDT".to_string(),
            settlement_currency: "USDT".to_string(),
            is_inverse: false,
            activation_time_nanos: 1_778_832_000_000_000_000,
            expiration_time_nanos: 1_780_646_400_000_000_000,
            price_increment: "0.1".to_string(),
            size_increment: "0.001".to_string(),
            min_quantity: "0.001".to_string(),
            max_quantity: "1000".to_string(),
            min_notional: "5".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.1".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn inverse_perpetual_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoPerpetual(CryptoPerpetualInstrumentSpec {
            instrument_kind: CryptoPerpetualInstrumentKind::CryptoPerpetual,
            nt_instrument_id: "BTCUSD.BYBIT".to_string(),
            raw_symbol: "BTCUSD".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USD".to_string(),
            settlement_currency: "BTC".to_string(),
            is_inverse: true,
            price_increment: "0.5".to_string(),
            size_increment: "1".to_string(),
            min_quantity: "1".to_string(),
            max_quantity: "1000000".to_string(),
            min_notional: "1".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.5".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn inverse_future_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoFuture(CryptoFutureInstrumentSpec {
            instrument_kind: CryptoFutureInstrumentKind::CryptoFuture,
            nt_instrument_id: "BTCUSDM26.BYBIT".to_string(),
            raw_symbol: "BTCUSDM26".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USD".to_string(),
            settlement_currency: "BTC".to_string(),
            is_inverse: true,
            activation_time_nanos: 1_764_892_800_000_000_000,
            expiration_time_nanos: 1_781_020_800_000_000_000,
            price_increment: "0.5".to_string(),
            size_increment: "1".to_string(),
            min_quantity: "1".to_string(),
            max_quantity: "1000000".to_string(),
            min_notional: "1".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.5".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn accepted_dataset() -> AcceptedDataset {
        let checks = RequiredChecks {
            source_access: RequiredCheck::passed("manifest"),
            license: RequiredCheck::passed("attestation"),
            schema: RequiredCheck::passed("schema"),
            time_semantics: RequiredCheck::passed("ms_to_nanos"),
            instrument_universe: RequiredCheck::passed("universe"),
            coverage: RequiredCheck::passed("manifest"),
            retention_freshness: RequiredCheck::passed("retention"),
            granularity: RequiredCheck::passed("native"),
            completeness: RequiredCheck::passed("manifest"),
            nt_mapping: RequiredCheck::passed("TradeTick"),
            cost: RequiredCheck::passed("free"),
            storage: RequiredCheck::passed("artifact_root"),
        };
        let object = IngestManifestObjectRecord {
            s3_uri: "s3://bolt-parquet/.../symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
            source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz"
                .to_string(),
            sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            bytes: 8505,
            archive_date: "2026-03-01".to_string(),
            schema_columns: vec![
                "id".to_string(),
                "timestamp".to_string(),
                "price".to_string(),
                "volume".to_string(),
                "side".to_string(),
                "rpi".to_string(),
            ],
        };
        let forbidden_claims = vec!["No execution-quality claims.".to_string()];
        let proof = SourceProofReport {
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            contract_version: "backfill-table-contract.v1".to_string(),
            schema_version: "backfill-source-proof.v1".to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "bybit-spot-tick-trades".to_string(),
            venue: "bybit".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            table_family: "trades".to_string(),
            evidence_state: EvidenceState::OwnerArchiveBackfillable,
            source_candidate_class: SourceCandidateClass::OfficialFree,
            source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            official_free_gap_ref: None,
            paid_vendor_gap_ref: None,
            fixture_type: FixtureType::PerpsSpot,
            requested_time_range: TimeRange {
                start_utc: "2025-06-01T00:00:00Z".to_string(),
                end_utc: "2026-06-01T00:00:00Z".to_string(),
            },
            coverage_time_range: TimeRange {
                start_utc: "2026-03-01T00:00:00Z".to_string(),
                end_utc: "2026-03-02T00:00:00Z".to_string(),
            },
            instrument_universe_id: "bybit-spot-instruments-2026-03-01".to_string(),
            raw_sample_uri: object.s3_uri.clone(),
            raw_sample_hash: object.sha256.clone(),
            schema_sample_uri: "s3://.../schema.json".to_string(),
            schema_sample_hash: "bf26db".to_string(),
            license_ref: "https://public.bybit.com/ (attestation)".to_string(),
            license_scope: LicenseScope::Public,
            retention_ref: "https://public.bybit.com/".to_string(),
            cost_ref: "cost://free-public-archive".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            l2_replay_evidence: L2ReplayEvidence {
                order_book_delta_ref: None,
                sufficient_snapshot_cadence_ref: None,
                no_tick_size_change_universe_ref: None,
                timed_instrument_epoch_replay_ref: None,
            },
            forbidden_claims: forbidden_claims.clone(),
            claim_limits: claim_limits_for(&forbidden_claims),
            cross_market_components: Vec::new(),
            acceptance_scope: Some(AcceptanceScope {
                planned_objects: 1,
                completed_objects: 1,
                failed_objects: 0,
                skipped_objects: 0,
                accepted_bytes: object.bytes,
                selector_scope_violations: 0,
            }),
            gap_policy_id: String::new(),
            required_checks: checks,
            acceptance_mode: None,
            accepted_by: None,
            accepted_at: None,
            supersedes_source_proof_id: None,
        }
        .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
        .unwrap();
        select_accepted_dataset(&proof, &object, &object.sha256).unwrap()
    }

    fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
        claims
            .iter()
            .enumerate()
            .map(|(index, claim)| SourceProofClaimLimit {
                id: format!("claim-limit-{}", index + 1),
                severity: "blocking".to_string(),
                claim: claim.clone(),
                reason: "source fidelity does not prove this claim".to_string(),
                evidence_ref: "source-proof://fidelity-class".to_string(),
            })
            .collect()
    }

    const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
        1,1772323201665,617.2,0.3,buy,0\n\
        2,1772323312219,617.9,0.1456,sell,0\n\
        3,1772323312236,617,0.1544,sell,0\n";

    fn canonical_table() -> CanonicalTradesTable {
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            SAMPLE_CSV,
            42,
            "ingest-run-test",
        )
        .unwrap()
    }

    #[test]
    fn build_currency_pair_honours_trailing_zero_increment() {
        let mut spec = spec();
        spec.price_increment = "0.10".to_string();
        let instrument = build_currency_pair(&spec).expect("build instrument");
        // Precision derived from the increment must agree with the increment's
        // own precision, or `CurrencyPair::new` would carry mismatched scales.
        assert_eq!(instrument.price_precision(), 2);
    }

    #[test]
    fn build_currency_pair_rejects_malformed_decimal() {
        let mut spec = spec();
        spec.price_increment = "not-a-number".to_string();
        assert!(build_currency_pair(&spec).is_err());
    }

    #[test]
    fn build_currency_pair_rejects_out_of_range_notional() {
        // A notional that parses as an f64 but exceeds NautilusTrader's Money
        // range must surface as an error, never a panic, on the accepted-data path.
        let mut spec = spec();
        spec.max_notional = "1e40".to_string();
        assert!(build_currency_pair(&spec).is_err());
    }

    #[test]
    fn build_currency_pair_rejects_blank_raw_symbol() {
        // A blank raw symbol must error via the checked Symbol constructor,
        // never panic.
        let mut spec = spec();
        spec.raw_symbol = String::new();
        assert!(build_currency_pair(&spec).is_err());
    }

    #[test]
    fn build_currency_pair_derives_precision_from_scientific_increment() {
        // `Price::from_str` accepts scientific notation, so precision must be
        // derived from the parsed increment (not a decimal-string char count),
        // or `CurrencyPair::new` would panic on a precision mismatch.
        let mut spec = spec();
        spec.price_increment = "1e-2".to_string();
        let instrument = build_currency_pair(&spec).expect("scientific increment");
        assert_eq!(instrument.price_precision(), 2);
    }

    #[test]
    fn canonical_rows_to_trade_ticks_rejects_invalid_trade_id() {
        // A trade id longer than NautilusTrader's 36-char id limit must error,
        // never panic, when projected to a TradeTick.
        let long_id = "x".repeat(40);
        let csv = format!(
            "id,timestamp,price,volume,side,rpi\n{long_id},1772323201665,617.2,0.3,buy,0\n"
        );
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            &csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let instrument = build_currency_pair(&spec()).expect("instrument");
        assert!(canonical_rows_to_trade_ticks(&table, &instrument).is_err());
    }

    #[test]
    fn canonical_rows_to_trade_ticks_accepts_trailing_zero_source_values() {
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,617.20,0.3000,buy,0\n";
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument)
            .expect("trailing zero source values are exact at instrument precision");
        assert_eq!(ticks[0].price, Price::from("617.2"));
        assert_eq!(ticks[0].size, Quantity::from("0.3000"));
    }

    #[test]
    fn builds_currency_pair_from_accepted_spec() {
        let instrument = build_currency_pair(&spec()).expect("build instrument");
        assert_eq!(instrument.id().to_string(), "BNBUSDC.BYBIT");
        assert_eq!(instrument.price_precision(), 1);
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn builds_crypto_perpetual_from_accepted_spec() {
        let instrument = build_catalog_instrument(&linear_perpetual_spec()).expect("instrument");
        let InstrumentAny::CryptoPerpetual(perpetual) = instrument else {
            panic!("expected CryptoPerpetual");
        };
        assert_eq!(perpetual.id().to_string(), "BTCUSDT.BYBIT");
        assert_eq!(perpetual.base_currency.to_string(), "BTC");
        assert_eq!(perpetual.quote_currency.to_string(), "USDT");
        assert_eq!(perpetual.settlement_currency.to_string(), "USDT");
        assert!(!perpetual.is_inverse);
        assert_eq!(perpetual.price_precision(), 1);
        assert_eq!(perpetual.size_precision(), 3);
    }

    #[test]
    fn builds_crypto_future_from_accepted_spec() {
        let instrument = build_catalog_instrument(&linear_future_spec()).expect("instrument");
        let InstrumentAny::CryptoFuture(future) = instrument else {
            panic!("expected CryptoFuture");
        };
        assert_eq!(future.id().to_string(), "BTCUSDT-05JUN26.BYBIT");
        assert_eq!(future.underlying.to_string(), "BTC");
        assert_eq!(future.quote_currency.to_string(), "USDT");
        assert_eq!(future.settlement_currency.to_string(), "USDT");
        assert_eq!(future.activation_ns.as_u64(), 1_778_832_000_000_000_000);
        assert_eq!(future.expiration_ns.as_u64(), 1_780_646_400_000_000_000);
        assert!(!future.is_inverse);
    }

    #[test]
    fn builds_inverse_crypto_perpetual_from_accepted_spec() {
        let instrument = build_catalog_instrument(&inverse_perpetual_spec()).expect("instrument");
        let InstrumentAny::CryptoPerpetual(perpetual) = instrument else {
            panic!("expected CryptoPerpetual");
        };
        assert_eq!(perpetual.id().to_string(), "BTCUSD.BYBIT");
        assert_eq!(perpetual.base_currency.to_string(), "BTC");
        assert_eq!(perpetual.quote_currency.to_string(), "USD");
        assert_eq!(perpetual.settlement_currency.to_string(), "BTC");
        assert!(perpetual.is_inverse);
    }

    #[test]
    fn builds_inverse_crypto_future_from_accepted_spec() {
        let instrument = build_catalog_instrument(&inverse_future_spec()).expect("instrument");
        let InstrumentAny::CryptoFuture(future) = instrument else {
            panic!("expected CryptoFuture");
        };
        assert_eq!(future.id().to_string(), "BTCUSDM26.BYBIT");
        assert_eq!(future.underlying.to_string(), "BTC");
        assert_eq!(future.quote_currency.to_string(), "USD");
        assert_eq!(future.settlement_currency.to_string(), "BTC");
        assert!(future.is_inverse);
        assert!(future.activation_ns < future.expiration_ns);
    }

    #[test]
    fn catalog_instrument_spec_deserializes_legacy_spot_shape() {
        let parsed: CatalogInstrumentSpec = toml::from_str(
            r#"
nt_instrument_id = "BNBUSDC.BYBIT"
raw_symbol = "BNBUSDC"
base_currency = "BNB"
quote_currency = "USDC"
price_increment = "0.1"
size_increment = "0.0001"
min_quantity = "0.0001"
max_quantity = "1400"
min_notional = "5"
max_notional = "200000"
"#,
        )
        .expect("legacy spot spec parses");
        assert!(matches!(parsed, CatalogInstrumentSpec::Spot(_)));
    }

    #[test]
    fn projects_derivative_trade_ticks_with_nt_crypto_instrument() {
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BTCUSDT".to_string(),
            venue_symbol: "BTCUSDT".to_string(),
            nt_instrument_id: "BTCUSDT.BYBIT".to_string(),
        };
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,617.20,0.3000,buy,0\n";
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_trades_to_catalog(&table, &linear_perpetual_spec(), dir.path())
                .expect("project derivative");

        assert_eq!(projection.trade_count, 1);
        assert_eq!(projection.nt_instrument_id, "BTCUSDT.BYBIT");
        let loaded = read_back_trade_ticks(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].instrument_id.to_string(), "BTCUSDT.BYBIT");
    }

    #[test]
    fn projects_and_reads_back_trade_ticks() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_trades_to_catalog(&table, &spec(), dir.path()).expect("project");
        assert_eq!(projection.trade_count, 3);
        assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_trade_ticks(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].instrument_id.to_string(), "BNBUSDC.BYBIT");
        // 617 rescaled to price precision 1 -> 617.0
        assert_eq!(loaded[2].price, Price::from("617.0"));
    }

    #[test]
    fn binary_option_l2_catalog_records_round_trip_through_nt_catalog() {
        use nautilus_model::{
            data::{OrderBookDelta, order::BookOrder},
            enums::{AssetClass, BookAction, OrderSide, RecordFlag},
            instruments::BinaryOption,
        };
        use ustr::Ustr;

        let ts_init = UnixNanos::from(1_000_000_000u64);
        let instrument_id = InstrumentId::from_str("YES.TESTVENUE").unwrap();
        let instrument = InstrumentAny::BinaryOption(
            BinaryOption::new_checked(
                instrument_id,
                Symbol::new_checked("YES").unwrap(),
                AssetClass::Alternative,
                Currency::from_str("USD").unwrap(),
                UnixNanos::from(0),
                UnixNanos::from(2_000_000_000u64),
                2,
                6,
                Price::from("0.01"),
                Quantity::from("0.000001"),
                Some(Ustr::from("Yes")),
                Some(Ustr::from("Bounded binary option fixture")),
                None,
                Some(Quantity::from("1")),
                None,
                None,
                Some(Price::from("1.00")),
                Some(Price::from("0.01")),
                None,
                None,
                Some(Decimal::ZERO),
                Some(Decimal::ZERO),
                None,
                ts_init,
                ts_init,
            )
            .expect("binary option"),
        );
        let instrument_id = instrument.id();
        let deltas = vec![
            OrderBookDelta::clear(
                instrument_id,
                0,
                UnixNanos::from(1_772_323_201_665_000_000u64),
                ts_init,
            ),
            OrderBookDelta::new_checked(
                instrument_id,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.49"), Quantity::from("10"), 0),
                RecordFlag::F_LAST as u8,
                0,
                UnixNanos::from(1_772_323_201_665_000_000u64),
                ts_init,
            )
            .expect("bid delta"),
        ];
        let tick = TradeTick::new(
            instrument_id,
            Price::from("0.51"),
            Quantity::from("2"),
            AggressorSide::Buyer,
            TradeId::new_checked("pmxt-trade-1").unwrap(),
            UnixNanos::from(1_772_323_201_665_000_000u64),
            ts_init,
        );

        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        catalog
            .write_instruments(vec![instrument])
            .expect("write binary option instrument");
        catalog
            .write_to_parquet(deltas.clone(), None, None, None)
            .expect("write order book deltas");
        catalog
            .write_to_parquet(vec![tick], None, None, None)
            .expect("write trade tick");

        let loaded_deltas = catalog
            .query_typed_data::<OrderBookDelta>(
                Some(vec![instrument_id.to_string()]),
                None,
                None,
                None,
                None,
                true,
            )
            .expect("read order book deltas");
        let loaded_ticks =
            read_back_trade_ticks(dir.path(), &instrument_id.to_string()).expect("read ticks");

        assert_eq!(loaded_deltas.len(), deltas.len());
        assert_eq!(loaded_ticks.len(), 1);
        assert!(
            !logical_catalog_hash(dir.path())
                .expect("logical hash")
                .is_empty()
        );
    }

    #[test]
    fn projection_refuses_dirty_catalog_root() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_trades_to_catalog(&table, &spec(), dir.path())
            .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn catalog_hash_is_deterministic_across_roots() {
        let table = canonical_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_trades_to_catalog(&table, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_trades_to_catalog(&table, &spec(), dir_b.path()).unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same data must hash identically regardless of root"
        );
    }

    #[test]
    fn catalog_hash_changes_with_data_content() {
        // Two projections that differ only in one trade's price must hash
        // differently, proving the catalog hash covers the written data bytes
        // (not just file paths).
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        let table_a = canonical_table();
        let csv_b = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,999.9,0.3,buy,0\n\
            2,1772323312219,617.9,0.1456,sell,0\n\
            3,1772323312236,617,0.1544,sell,0\n";
        let table_b = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            csv_b,
            42,
            "ingest-run-test",
        )
        .expect("normalize variant");
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_trades_to_catalog(&table_a, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_trades_to_catalog(&table_b, &spec(), dir_b.path()).unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different trade data must change the catalog hash"
        );
    }

    fn expected_hash_field(hasher: &mut Sha256, label: &str, value: &str) {
        hasher.update(label.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
        hasher.update([0xff]);
    }

    fn expected_hash_optional_field<T: ToString>(
        hasher: &mut Sha256,
        label: &str,
        value: Option<&T>,
    ) {
        match value {
            Some(value) => expected_hash_field(hasher, label, &value.to_string()),
            None => expected_hash_field(hasher, label, "<none>"),
        }
    }

    fn expected_hash_currency_pair(hasher: &mut Sha256, instrument: &CurrencyPair) {
        assert!(
            instrument.info.is_none(),
            "test fixture uses no opaque info"
        );
        expected_hash_field(hasher, "instrument.type", "currency_pair");
        expected_hash_field(hasher, "instrument.id", &instrument.id.to_string());
        expected_hash_field(
            hasher,
            "instrument.raw_symbol",
            instrument.raw_symbol.as_ref(),
        );
        expected_hash_field(
            hasher,
            "instrument.base_currency",
            &instrument.base_currency.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.quote_currency",
            &instrument.quote_currency.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.price_precision",
            &instrument.price_precision.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.size_precision",
            &instrument.size_precision.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.price_increment",
            &instrument.price_increment.as_decimal().to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.size_increment",
            &instrument.size_increment.as_decimal().to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.multiplier",
            &instrument.multiplier.as_decimal().to_string(),
        );
        expected_hash_optional_field(hasher, "instrument.lot_size", instrument.lot_size.as_ref());
        expected_hash_optional_field(
            hasher,
            "instrument.max_quantity",
            instrument.max_quantity.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.min_quantity",
            instrument.min_quantity.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.max_notional",
            instrument.max_notional.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.min_notional",
            instrument.min_notional.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.max_price",
            instrument.max_price.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.min_price",
            instrument.min_price.as_ref(),
        );
        expected_hash_field(
            hasher,
            "instrument.margin_init",
            &instrument.margin_init.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.margin_maint",
            &instrument.margin_maint.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.maker_fee",
            &instrument.maker_fee.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.taker_fee",
            &instrument.taker_fee.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.ts_event",
            &instrument.ts_event.as_u64().to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.ts_init",
            &instrument.ts_init.as_u64().to_string(),
        );
    }

    fn expected_logical_catalog_hash(instrument: &CurrencyPair, ticks: &[TradeTick]) -> String {
        let mut ticks = ticks.to_vec();
        ticks.sort_by_key(|tick| {
            (
                tick.ts_event.as_u64(),
                tick.trade_id.to_string(),
                tick.instrument_id.to_string(),
            )
        });
        let mut hasher = Sha256::new();
        hasher.update(b"nautilus-logical-catalog.v1");
        hasher.update([0u8]);
        expected_hash_currency_pair(&mut hasher, instrument);
        for tick in ticks {
            hasher.update([2u8]);
            hasher.update(tick.instrument_id.to_string().as_bytes());
            hasher.update([3u8]);
            hasher.update(tick.trade_id.to_string().as_bytes());
            hasher.update([4u8]);
            hasher.update(tick.price.as_decimal().to_string().as_bytes());
            hasher.update([5u8]);
            hasher.update(tick.size.as_decimal().to_string().as_bytes());
            hasher.update([6u8]);
            hasher.update(tick.aggressor_side.to_string().as_bytes());
            hasher.update([7u8]);
            hasher.update(tick.ts_event.as_u64().to_string().as_bytes());
            hasher.update([8u8]);
            hasher.update(tick.ts_init.as_u64().to_string().as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    #[test]
    fn catalog_hash_matches_stable_currency_pair_fields() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(&table, &spec(), dir.path()).unwrap();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("ticks");
        assert_eq!(
            projection.catalog_hash,
            expected_logical_catalog_hash(&instrument, &ticks),
            "catalog hash must use explicit stable instrument fields, not Debug output"
        );
    }

    #[test]
    fn catalog_hash_ignores_writer_sidecar_files() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(&table, &spec(), dir.path()).unwrap();
        fs::write(dir.path().join("writer-version.txt"), b"nt writer metadata").unwrap();
        assert_eq!(
            projection.catalog_hash,
            logical_catalog_hash(dir.path()).unwrap(),
            "catalog hash must describe logical catalog contents, not unrelated writer files"
        );
    }

    #[test]
    fn catalog_hash_ignores_unrelated_relative_paths() {
        // Non-catalog sidecar bytes under different relative paths must not
        // affect the logical digest. The digest is over NT-read catalog records,
        // not filesystem layout.
        let root_a = tempfile::TempDir::new().unwrap();
        let root_b = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(root_a.path().join("data/alpha")).unwrap();
        fs::write(root_a.path().join("data/alpha/file.parquet"), b"identical").unwrap();
        fs::create_dir_all(root_b.path().join("data/beta")).unwrap();
        fs::write(root_b.path().join("data/beta/file.parquet"), b"identical").unwrap();
        assert_eq!(
            logical_catalog_hash(root_a.path()).unwrap(),
            logical_catalog_hash(root_b.path()).unwrap(),
            "unrelated bytes under different relative paths must not change the logical hash"
        );
    }
}
