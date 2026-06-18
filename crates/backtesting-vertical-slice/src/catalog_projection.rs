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
//! When the accepted archive carries finer prints than the venue's current
//! instrument metadata, the instrument precision is widened to the data's
//! actual scale (trailing-zero increment rescale; tick value unchanged) so
//! the projection represents the accepted data exactly.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, ensure};
use nautilus_core::{Params, UnixNanos, string::urlencoding};
use nautilus_model::{
    data::{
        Bar, BarSpecification, BarType, CatalogPathPrefix, FundingRateUpdate, IndexPriceUpdate,
        MarkPriceUpdate, OrderBookDelta, QuoteTick, TradeTick, order::BookOrder,
    },
    enums::{AggregationSource, AggressorSide, AssetClass, BookAction, OrderSide, PriceType},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{
        BinaryOption, CryptoFuture, CryptoPerpetual, CurrencyPair, Instrument, InstrumentAny,
    },
    types::{Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::{ParquetDataCatalog, urisafe_instrument_id};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use super::{
    canonical_market_data::{
        CanonicalBarRow, CanonicalBarsTable, CanonicalFundingRateRow, CanonicalFundingRatesTable,
        CanonicalIndexPriceRow, CanonicalIndexPricesTable, CanonicalMarkPriceRow,
        CanonicalMarkPricesTable, CanonicalOrderBookDeltaRow, CanonicalOrderBookDeltasTable,
        CanonicalQuoteRow, CanonicalQuotesTable, DeltaAction, DeltaSide,
    },
    canonical_trades::{CanonicalTradesTable, TradeAggressorSide},
    source_proof::SourceProofFidelityClass,
};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the order-book-delta projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// NautilusTrader data type written for the bar projection.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// NautilusTrader data type written for the top-of-book quote projection.
pub const NT_DATA_TYPE_QUOTE_TICK: &str = "QuoteTick";

/// NautilusTrader data type written for the index-price projection.
///
/// The token MUST equal the NT struct name; the catalog directory is
/// `index_prices` via NT's own `impl_catalog_path_prefix!(IndexPriceUpdate,
/// "index_prices")` — never redefined here.
pub const NT_DATA_TYPE_INDEX_PRICE_UPDATE: &str = "IndexPriceUpdate";

/// NautilusTrader data type written for the mark-price projection.
///
/// The token MUST equal the NT struct name; the catalog directory is
/// `mark_prices` via NT's own `impl_catalog_path_prefix!(MarkPriceUpdate,
/// "mark_prices")` — never redefined here.
pub const NT_DATA_TYPE_MARK_PRICE_UPDATE: &str = "MarkPriceUpdate";

/// NautilusTrader data type written for the funding-rate projection.
///
/// The token MUST equal the NT struct name; the catalog directory is
/// `funding_rate_update` via NT's own
/// `impl_catalog_path_prefix!(FundingRateUpdate, "funding_rate_update")`.
pub const NT_DATA_TYPE_FUNDING_RATE_UPDATE: &str = "FundingRateUpdate";

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
    BinaryOption(BinaryOptionInstrumentSpec),
    Spot(SpotInstrumentSpec),
}

impl CatalogInstrumentSpec {
    #[cfg(test)]
    pub(crate) fn spot_mut(&mut self) -> Option<&mut SpotInstrumentSpec> {
        match self {
            Self::Spot(spec) => Some(spec),
            Self::CryptoPerpetual(_) | Self::CryptoFuture(_) | Self::BinaryOption(_) => None,
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

/// TOML discriminator for an NT [`BinaryOption`] instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOptionInstrumentKind {
    BinaryOption,
}

/// Accepted binary-option metadata needed to build NT's [`BinaryOption`]
/// instrument.
///
/// Prediction-market archives (emitted by the parquet event-stream and
/// JSONL/tar snapshot adapters) carry an outcome-scoped binary contract rather
/// than a base/quote pair: one settlement currency holds the contract value and
/// the activation/expiration window bounds the resolvable epoch. Every field is
/// a decimal/identifier string parsed exactly like the other specs parse
/// theirs (fail-loud, never a panic); `price_precision`/`size_precision` are
/// derived from the parsed increments only, per the module's
/// single-source-of-precision rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOptionInstrumentSpec {
    pub instrument_kind: BinaryOptionInstrumentKind,
    pub nt_instrument_id: String,
    pub raw_symbol: String,
    /// NT [`AssetClass`] code, for example `ALTERNATIVE`.
    pub asset_class: String,
    /// Settlement (and quote) currency code, for example `USDC`.
    pub currency: String,
    pub activation_time_nanos: u64,
    pub expiration_time_nanos: u64,
    pub price_increment: String,
    pub size_increment: String,
    pub outcome: Option<String>,
    pub description: Option<String>,
    pub max_quantity: Option<String>,
    pub min_quantity: Option<String>,
    pub max_notional: Option<String>,
    pub min_notional: Option<String>,
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

impl CatalogInstrumentSpecSource for BinaryOptionInstrumentSpec {
    fn build_instrument_any(&self) -> Result<InstrumentAny> {
        Ok(InstrumentAny::BinaryOption(build_binary_option(self)?))
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
            Self::BinaryOption(spec) => Ok(InstrumentAny::BinaryOption(build_binary_option(spec)?)),
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
        None, // tick_scheme (NT bump): not populated by bolt
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

fn parse_asset_class(value: &str, label: &str) -> Result<AssetClass> {
    let code = value.trim();
    ensure!(!code.is_empty(), "{label} must not be empty");
    AssetClass::from_str(code)
        .map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_optional_ustr(value: Option<&str>, label: &str) -> Result<Option<Ustr>> {
    value
        .map(|value| {
            let text = value.trim();
            ensure!(!text.is_empty(), "{label} must not be empty when present");
            Ok(Ustr::from(text))
        })
        .transpose()
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
        None, // tick_scheme (NT bump): not populated by bolt
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
        None, // tick_scheme (NT bump): not populated by bolt
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

/// Build NT's [`BinaryOption`] from accepted prediction-market metadata.
///
/// Mirrors [`build_currency_pair`]'s structure: every constructor argument is
/// parsed through a checked/fail-loud helper, and price/size precision derive
/// from the parsed increments only (the module's single-source-of-precision
/// rule). The single settlement `currency` is NautilusTrader's contract
/// currency for a binary option (it has no base/quote pair); the
/// activation/expiration window bounds the resolvable epoch.
///
/// # Errors
///
/// Returns an error if accepted metadata cannot construct a checked NT binary
/// option.
pub fn build_binary_option(spec: &BinaryOptionInstrumentSpec) -> Result<BinaryOption> {
    let instrument_id = parse_instrument_id(&spec.nt_instrument_id)?;
    let raw_symbol = parse_raw_symbol(&spec.raw_symbol)?;
    let asset_class = parse_asset_class(&spec.asset_class, "asset_class")?;
    let currency = parse_venue_currency(&spec.currency, "currency")?;
    ensure!(
        spec.activation_time_nanos < spec.expiration_time_nanos,
        "activation_time_nanos must be before expiration_time_nanos"
    );
    let price_increment = parse_price(&spec.price_increment, "price_increment")?;
    let size_increment = parse_quantity(&spec.size_increment, "size_increment")?;
    let price_precision = price_increment.precision;
    let size_precision = size_increment.precision;
    // The NT catalog Arrow schema for BinaryOption does NOT encode these six
    // fields (binary_option.rs lines 412-417 in rev 6e059dc hardcode them as
    // `None` on decode). Accepting a spec that sets them would silently drop
    // the configured values on the projection round-trip, violating the
    // fail-loud rule.  Reject them here so the operator knows up front.
    for (label, value) in [
        ("max_notional", spec.max_notional.as_deref()),
        ("min_notional", spec.min_notional.as_deref()),
        ("max_price", spec.max_price.as_deref()),
        ("min_price", spec.min_price.as_deref()),
        ("margin_init", spec.margin_init.as_deref()),
        ("margin_maint", spec.margin_maint.as_deref()),
    ] {
        ensure!(
            value.is_none(),
            "{label} is not supported for BinaryOption: the NT catalog Arrow schema does not \
             persist this field (decode_binary_option_batch hardcodes it as None), so it would \
             be silently lost on the projection round-trip"
        );
    }
    let option = BinaryOption::new_checked(
        instrument_id,
        raw_symbol,
        asset_class,
        currency,
        UnixNanos::from(spec.activation_time_nanos),
        UnixNanos::from(spec.expiration_time_nanos),
        price_precision,
        size_precision,
        price_increment,
        size_increment,
        parse_optional_ustr(spec.outcome.as_deref(), "outcome")?,
        parse_optional_ustr(spec.description.as_deref(), "description")?,
        parse_optional_quantity(spec.max_quantity.as_deref(), "max_quantity")?,
        parse_optional_quantity(spec.min_quantity.as_deref(), "min_quantity")?,
        None,
        None,
        None,
        None,
        None,
        None,
        parse_optional_decimal(spec.maker_fee.as_deref(), "maker_fee")?,
        parse_optional_decimal(spec.taker_fee.as_deref(), "taker_fee")?,
        None, // tick_scheme (NT bump): not populated by bolt
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid binary option for {:?}: {error}",
            spec.nt_instrument_id
        )
    })?;
    // The six NT-non-persistable fields are passed as `None`/default above, but
    // route the constructed instrument through the single catalog-persistability
    // invariant so the SPEC path and the one-off backfill path enforce one rule.
    ensure_binary_option_catalog_persistable(&option)?;
    Ok(option)
}

/// THE one rule: a [`BinaryOption`] is catalog-persistable only when every
/// field the NT catalog Arrow schema cannot encode is at its round-trip value.
///
/// `decode_binary_option_batch` (rev 6e059dc lines 412-417) hardcodes
/// `max_price`, `min_price`, `max_notional`, `min_notional`, `margin_init`, and
/// `margin_maint` to `None` on read-back; `BinaryOption`'s constructor stores
/// the two margins as `Decimal::default()` when given `None`. So a persisted
/// instrument round-trips losslessly only when the four `Option` bounds are
/// `None` and the two margins are zero. Any other value would be silently
/// dropped on the projection round-trip (FAIL LOUD: reject it here). Both the
/// SPEC projection ([`build_binary_option`]) and the one-off backfill
/// projection share this invariant so no second production rule can disagree on
/// whether a degraded instrument is acceptable (NO DUAL PATHS).
///
/// # Errors
///
/// Returns an error naming the first field that would be lost on round-trip.
pub(crate) fn ensure_binary_option_catalog_persistable(inst: &BinaryOption) -> Result<()> {
    ensure!(
        inst.max_price.is_none(),
        "max_price would be silently lost on the NT catalog round-trip \
         (decode_binary_option_batch hardcodes it as None): {:?}",
        inst.id
    );
    ensure!(
        inst.min_price.is_none(),
        "min_price would be silently lost on the NT catalog round-trip \
         (decode_binary_option_batch hardcodes it as None): {:?}",
        inst.id
    );
    ensure!(
        inst.max_notional.is_none(),
        "max_notional would be silently lost on the NT catalog round-trip \
         (decode_binary_option_batch hardcodes it as None): {:?}",
        inst.id
    );
    ensure!(
        inst.min_notional.is_none(),
        "min_notional would be silently lost on the NT catalog round-trip \
         (decode_binary_option_batch hardcodes it as None): {:?}",
        inst.id
    );
    ensure!(
        inst.margin_init == Decimal::default(),
        "margin_init would be silently zeroed on the NT catalog round-trip \
         (decode_binary_option_batch hardcodes it as None): {:?}",
        inst.id
    );
    ensure!(
        inst.margin_maint == Decimal::default(),
        "margin_maint would be silently zeroed on the NT catalog round-trip \
         (decode_binary_option_batch hardcodes it as None): {:?}",
        inst.id
    );
    Ok(())
}

/// NT `ts_event` for a canonical row: the exchange/source event instant.
///
/// Event time is the per-row ordering clock the table's `validate()` already
/// proved positive and monotonic, so a non-positive value here is an internal
/// invariant breach — fail loud, never emit 0. This is the single owner of the
/// canonical-event-time → NT `UnixNanos` conversion for every data family, so
/// the projection seams and the runner's read-back/window gates cannot drift
/// into separate derivations (NO DUAL PATHS).
pub(crate) fn ts_event_nanos(event_time: i64, label: &str) -> Result<UnixNanos> {
    let nanos = u64::try_from(event_time)
        .with_context(|| format!("{label}: negative event time {event_time}"))?;
    ensure!(nanos > 0, "{label}: non-positive event time {event_time}");
    Ok(UnixNanos::from(nanos))
}

/// NT `ts_init` for a canonical row: when the data became available to the
/// system.
///
/// Source order is `availability_time` (the source's own availability instant)
/// when present, else `capture_time` (worker receipt). NT replays and windows
/// by `ts_init` (`HasTsInit`), so this must reflect receipt order, never the
/// exchange event clock. This NEVER falls back to event time or 0: if
/// `availability_time` is `Some` it must be valid; if it is `None`,
/// `capture_time` must be valid; otherwise fail loud so a missing receipt clock
/// can never silently become `ts_init=0` or be conflated with the event clock.
/// This is the single owner of the canonical-receipt-time → NT `UnixNanos`
/// derivation: the projection seams AND the runner's read-back/window gates call
/// it, so there is exactly one place that decides the `ts_init` precedence.
pub(crate) fn ts_init_nanos(
    availability_time: Option<i64>,
    capture_time: i64,
    label: &str,
) -> Result<UnixNanos> {
    let (raw, field) = match availability_time {
        Some(value) => (value, "availability_time"),
        None => (capture_time, "capture_time"),
    };
    let nanos = u64::try_from(raw)
        .with_context(|| format!("{label}: negative ts_init source {field}={raw}"))?;
    ensure!(
        nanos > 0,
        "{label}: non-positive ts_init source {field}={raw}"
    );
    Ok(UnixNanos::from(nanos))
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

/// Maximum decimal scale across one canonical column, after normalization so
/// trailing zeros do not count (mirrors `rescaled`'s normalize-before-check).
fn max_normalized_scale<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> Result<u32> {
    let mut max = 0u32;
    for value in values {
        let mut decimal =
            Decimal::from_str(value).with_context(|| format!("{label} decimal {value:?}"))?;
        decimal.normalize_assign();
        max = max.max(decimal.scale());
    }
    Ok(max)
}

/// Rescale a price increment to a wider decimal scale with trailing zeros.
/// The tick VALUE is unchanged; only its precision widens.
fn widened_price_increment(increment: Price, scale: u32) -> Result<Price> {
    let mut decimal = increment.as_decimal();
    decimal.rescale(scale);
    let widened = Price::from_str(&decimal.to_string()).map_err(|error| {
        anyhow::anyhow!("widen price_increment {increment} to scale {scale}: {error}")
    })?;
    ensure!(
        u32::from(widened.precision) == scale,
        "widened price_increment {widened} precision {} does not match requested scale {scale}",
        widened.precision
    );
    Ok(widened)
}

/// Rescale a size increment to a wider decimal scale with trailing zeros.
/// The step VALUE is unchanged; only its precision widens.
fn widened_size_increment(increment: Quantity, scale: u32) -> Result<Quantity> {
    let mut decimal = increment.as_decimal();
    decimal.rescale(scale);
    let widened = Quantity::from_str(&decimal.to_string()).map_err(|error| {
        anyhow::anyhow!("widen size_increment {increment} to scale {scale}: {error}")
    })?;
    ensure!(
        u32::from(widened.precision) == scale,
        "widened size_increment {widened} precision {} does not match requested scale {scale}",
        widened.precision
    );
    Ok(widened)
}

/// Read-only view over a canonical table's price-bearing and size-bearing
/// columns, used to derive the accepted data's actual decimal scale.
///
/// Each canonical family exposes its own column layout (one price/size per row
/// for trades and deltas; open/high/low/close prices plus volume for bars), so
/// the precision-widening logic depends on this view rather than on a single
/// concrete table type. Empty string cells (such as `CLEAR` delta rows) are
/// skipped by the iterator implementations so they never count toward scale.
pub(crate) trait CanonicalPriceSizeView {
    /// Iterate every non-empty price-bearing decimal string in the table.
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_>;
    /// Iterate every non-empty size-bearing decimal string in the table.
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_>;
}

impl CanonicalPriceSizeView for CanonicalTradesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.price.as_str()))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.size.as_str()))
    }
}

impl CanonicalPriceSizeView for CanonicalOrderBookDeltasTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .map(|row| row.price.as_str())
                .filter(|value| !value.is_empty()),
        )
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .map(|row| row.size.as_str())
                .filter(|value| !value.is_empty()),
        )
    }
}

impl CanonicalPriceSizeView for CanonicalBarsTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.volume.as_str()))
    }
}

impl CanonicalPriceSizeView for CanonicalQuotesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .flat_map(|row| [row.bid.as_str(), row.ask.as_str()]),
        )
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .flat_map(|row| [row.bid_size.as_str(), row.ask_size.as_str()]),
        )
    }
}

impl CanonicalPriceSizeView for CanonicalIndexPricesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.value.as_str()))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        // An index price is a point update with no size column, so the data's
        // size scale folds to 0 and `widen_instrument_precision_for_data` keeps
        // the instrument's own size precision unchanged.
        Box::new(std::iter::empty())
    }
}

impl CanonicalPriceSizeView for CanonicalMarkPricesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.value.as_str()))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        // A mark price is a point update with no size column, so the data's
        // size scale folds to 0 and `widen_instrument_precision_for_data` keeps
        // the instrument's own size precision unchanged.
        Box::new(std::iter::empty())
    }
}

/// Widen the catalog instrument's price/size precision to the accepted
/// data's actual maximum decimal scale.
///
/// Venue instrument endpoints describe the CURRENT trading rules, but
/// historical archives can carry finer prints than today's tick size (the
/// accepted object is the authority on its own scale). The projected
/// instrument must represent the accepted data exactly, so the increments
/// are rescaled with trailing zeros (tick VALUE unchanged) and precision is
/// re-derived from the widened increments — preserving this module's
/// single-source-of-precision rule. Precision is never narrowed: data
/// coarser than the venue tick keeps the venue precision.
///
/// # Errors
///
/// Returns an error if a canonical value fails to parse, a widened increment
/// cannot be represented by NautilusTrader, or the instrument kind does not
/// support widening.
fn widen_instrument_precision_for_data(
    mut instrument: InstrumentAny,
    table: &dyn CanonicalPriceSizeView,
) -> Result<InstrumentAny> {
    let data_price_scale = max_normalized_scale(table.price_values(), "price")?;
    let data_size_scale = max_normalized_scale(table.size_values(), "size")?;
    let price_scale = data_price_scale.max(u32::from(instrument.price_precision()));
    let size_scale = data_size_scale.max(u32::from(instrument.size_precision()));
    if price_scale == u32::from(instrument.price_precision())
        && size_scale == u32::from(instrument.size_precision())
    {
        return Ok(instrument);
    }
    let price_increment = widened_price_increment(instrument.price_increment(), price_scale)?;
    let size_increment = widened_size_increment(instrument.size_increment(), size_scale)?;
    match &mut instrument {
        InstrumentAny::CurrencyPair(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        InstrumentAny::CryptoPerpetual(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        InstrumentAny::CryptoFuture(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        InstrumentAny::BinaryOption(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        other => anyhow::bail!(
            "instrument kind for {} does not support data-derived precision widening",
            other.id()
        ),
    }
    Ok(instrument)
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
            let trade_id = TradeId::new_checked(&row.trade_id)
                .map_err(|error| anyhow::anyhow!("invalid trade_id {:?}: {error}", row.trade_id))?;
            let label = format!("trade {}", row.trade_id);
            let ts_event = ts_event_nanos(row.event_time, &label)?;
            let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
            Ok(TradeTick::new(
                instrument_id,
                price,
                size,
                aggressor,
                trade_id,
                ts_event,
                ts_init,
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
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data(instrument, table)?;
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

    ensure_clean_catalog_root(catalog_root)?;
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
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files =
        catalog_files_for_instruments::<TradeTick>(&catalog, catalog_root, &instrument_ids)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    catalog
        .query_typed_data::<TradeTick>(None, None, None, None, Some(files), false)
        .context("query trade ticks from catalog")
}

/// Fail closed on a dirty catalog root. NautilusTrader's `write_to_parquet`
/// skips writing when a file for the same instrument/interval already exists,
/// so projecting into a non-empty root could silently read back stale data
/// under this run's source proof and a stale catalog hash. The caller owns
/// the output lifecycle and must hand us a clean (absent or empty) root.
///
/// # Errors
///
/// Returns an error if the root exists and is non-empty, or cannot be read.
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
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;
    Ok(())
}

/// Convert canonical order-book-delta rows into NautilusTrader `OrderBookDelta`s
/// at the instrument's price/size precision.
///
/// `CLEAR` rows become `OrderBookDelta::clear` deltas carrying the row's flags;
/// `ADD`/`UPDATE`/`DELETE` rows build a price-keyed `BookOrder` (order_id from
/// the row, `0` for L2/MBP levels) under the matching `BookAction`. Flags,
/// sequence, and timestamps are carried verbatim from the canonical rows, which
/// the table's `validate()` has already proven dense and well-formed.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision, a side/action token is unknown, or an event time is negative.
pub fn canonical_rows_to_order_book_deltas<I: Instrument + ?Sized>(
    table: &CanonicalOrderBookDeltasTable,
    instrument: &I,
) -> Result<Vec<OrderBookDelta>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| {
            canonical_row_to_order_book_delta(instrument_id, row, price_precision, size_precision)
        })
        .collect()
}

fn canonical_row_to_order_book_delta(
    instrument_id: InstrumentId,
    row: &CanonicalOrderBookDeltaRow,
    price_precision: u8,
    size_precision: u8,
) -> Result<OrderBookDelta> {
    let label = format!("delta sequence {}", row.sequence);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    if row.action == DeltaAction::Clear.as_str() {
        // NautilusTrader's `clear` sets F_SNAPSHOT only; carry the canonical
        // row's full flag bitmask (F_SNAPSHOT required, optionally F_MBP and
        // F_LAST when the row closes a snapshot expansion), which validate()
        // has already enforced.
        let mut clear = OrderBookDelta::clear(instrument_id, row.sequence, ts_event, ts_init);
        clear.flags = row.flags;
        return Ok(clear);
    }
    let action = match row.action.as_str() {
        a if a == DeltaAction::Add.as_str() => BookAction::Add,
        a if a == DeltaAction::Update.as_str() => BookAction::Update,
        a if a == DeltaAction::Delete.as_str() => BookAction::Delete,
        other => anyhow::bail!("unknown delta action {other:?}"),
    };
    let side = match row.side.as_str() {
        s if s == DeltaSide::Buy.as_str() => OrderSide::Buy,
        s if s == DeltaSide::Sell.as_str() => OrderSide::Sell,
        other => anyhow::bail!("unknown delta side {other:?}"),
    };
    let price_str = rescaled(&row.price, price_precision)?;
    let price = Price::from_str(&price_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled price {price_str:?}: {error}"))?;
    let size_str = rescaled(&row.size, size_precision)?;
    let size = Quantity::from_str(&size_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled size {size_str:?}: {error}"))?;
    let order = BookOrder::new(side, price, size, row.order_id);
    OrderBookDelta::new_checked(
        instrument_id,
        action,
        order,
        row.flags,
        row.sequence,
        ts_event,
        ts_init,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid order book delta at sequence {}: {error}",
            row.sequence
        )
    })
}

/// Project a canonical order-book-delta table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `OrderBookDelta` projection. NautilusTrader writes its
/// native `data/order_book_deltas/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_order_book_deltas_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalOrderBookDeltasTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data(instrument, table)?;
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
    let deltas = canonical_rows_to_order_book_deltas(table, &instrument)?;
    let delta_count = deltas.len();

    ensure_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(deltas, None, None, None)
        .context("write order book deltas to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
        trade_count: delta_count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `OrderBookDelta` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_order_book_deltas(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<OrderBookDelta>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files =
        catalog_files_for_instruments::<OrderBookDelta>(&catalog, catalog_root, &instrument_ids)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    catalog
        .query_typed_data::<OrderBookDelta>(None, None, None, None, Some(files), false)
        .context("query order book deltas from catalog")
}

/// Convert canonical top-of-book quote rows into NautilusTrader `QuoteTick`s at
/// the instrument's price/size precision.
///
/// NT example strategies enter from `on_quote` (see the strategy examples at
/// `crates/.../strategy` @6e059dc); replaying `QuoteTick` data will drive
/// strategy `on_quote`. Keep the reference/non-traded instrument_id boundary
/// explicit at the run-spec layer (the `instrument_spec` keying at
/// `resolve_instrument_spec`): a quote on a reference instrument feeds signals,
/// a quote on the traded instrument can trigger entries.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn canonical_rows_to_quote_ticks<I: Instrument + ?Sized>(
    table: &CanonicalQuotesTable,
    instrument: &I,
) -> Result<Vec<QuoteTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| canonical_row_to_quote_tick(instrument_id, row, price_precision, size_precision))
        .collect()
}

fn canonical_row_to_quote_tick(
    instrument_id: InstrumentId,
    row: &CanonicalQuoteRow,
    price_precision: u8,
    size_precision: u8,
) -> Result<QuoteTick> {
    let label = match row.source_sequence.as_deref() {
        Some(sequence) => format!("quote {sequence}"),
        None => format!("quote {}", row.event_time),
    };
    let price_at = |value: &str, name: &str| -> Result<Price> {
        let rescaled = rescaled(value, price_precision)?;
        Price::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid rescaled {name} {rescaled:?}: {error}"))
    };
    let size_at = |value: &str, name: &str| -> Result<Quantity> {
        let rescaled = rescaled(value, size_precision)?;
        Quantity::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid rescaled {name} {rescaled:?}: {error}"))
    };
    let bid_price = price_at(&row.bid, "bid")?;
    let ask_price = price_at(&row.ask, "ask")?;
    let bid_size = size_at(&row.bid_size, "bid_size")?;
    let ask_size = size_at(&row.ask_size, "ask_size")?;
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    // `QuoteTick::new` panics only on precision inequality (bid vs ask price, or
    // bid vs ask size). Both prices rescale to the SAME instrument price
    // precision and both sizes to the SAME size precision, so equality holds;
    // the canonical table's spread validation has already proven both sides carry
    // a price. This mirrors the `TradeTick::new` template choice (rescaling
    // guarantees the invariant), so no `_checked` branch is needed here.
    Ok(QuoteTick::new(
        instrument_id,
        bid_price,
        ask_price,
        bid_size,
        ask_size,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical top-of-book quote table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `QuoteTick` projection. NautilusTrader writes its native
/// `data/quotes/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_quotes_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalQuotesTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data(instrument, table)?;
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
    let ticks = canonical_rows_to_quote_ticks(table, &instrument)?;
    let quote_count = ticks.len();

    ensure_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write quote ticks to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_QUOTE_TICK.to_string(),
        trade_count: quote_count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `QuoteTick` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_quotes(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<QuoteTick>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files =
        catalog_files_for_instruments::<QuoteTick>(&catalog, catalog_root, &instrument_ids)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    catalog
        .query_typed_data::<QuoteTick>(None, None, None, None, Some(files), false)
        .context("query quote ticks from catalog")
}

/// Convert canonical index-price rows into NautilusTrader `IndexPriceUpdate`s at
/// the instrument's price precision.
///
/// An index price is a point/reference update (the NT `IndexPriceUpdate.value`
/// is a `Price`): there is no size, aggressor, or trade id. Replaying it feeds
/// signals/reference series rather than driving strategy entries directly.
/// Timestamps route through the shared S1 receipt-clock owners
/// ([`ts_event_nanos`]/[`ts_init_nanos`]) — no new derivation here.
///
/// # Errors
///
/// Returns an error if a value cannot be represented at the instrument price
/// precision, or a timestamp source is invalid.
pub fn canonical_rows_to_index_price_updates<I: Instrument + ?Sized>(
    table: &CanonicalIndexPricesTable,
    instrument: &I,
) -> Result<Vec<IndexPriceUpdate>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    table
        .rows
        .iter()
        .map(|row| canonical_row_to_index_price_update(instrument_id, row, price_precision))
        .collect()
}

fn canonical_row_to_index_price_update(
    instrument_id: InstrumentId,
    row: &CanonicalIndexPriceRow,
    price_precision: u8,
) -> Result<IndexPriceUpdate> {
    let price_str = rescaled(&row.value, price_precision)?;
    let value = Price::from_str(&price_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled value {price_str:?}: {error}"))?;
    let label = format!("index price {}", row.event_time);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    Ok(IndexPriceUpdate::new(
        instrument_id,
        value,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical index-price table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `IndexPriceUpdate` projection. NautilusTrader writes its
/// native `data/index_prices/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_index_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalIndexPricesTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data(instrument, table)?;
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
    let updates = canonical_rows_to_index_price_updates(table, &instrument)?;
    let count = updates.len();

    ensure_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(updates, None, None, None)
        .context("write index prices to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_INDEX_PRICE_UPDATE.to_string(),
        trade_count: count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `IndexPriceUpdate` data back from `catalog_root`.
///
/// `IndexPriceUpdate` is keyed by bare instrument id under
/// `data/index_prices/<id>/` exactly like trades/deltas/quotes (not bar-type
/// keyed), so this uses the file-filter path, not the bar query path.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_index(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<IndexPriceUpdate>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files =
        catalog_files_for_instruments::<IndexPriceUpdate>(&catalog, catalog_root, &instrument_ids)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    catalog
        .query_typed_data::<IndexPriceUpdate>(None, None, None, None, Some(files), false)
        .context("query index prices from catalog")
}

/// Convert canonical mark-price rows into NautilusTrader `MarkPriceUpdate`s at
/// the instrument's price precision.
///
/// A mark price is a point/reference update (the NT `MarkPriceUpdate.value`
/// is a `Price`): there is no size, aggressor, or trade id. Replaying it feeds
/// signals/reference series rather than driving strategy entries directly.
/// Timestamps route through the shared S1 receipt-clock owners
/// ([`ts_event_nanos`]/[`ts_init_nanos`]) — no new derivation here.
///
/// # Errors
///
/// Returns an error if a value cannot be represented at the instrument price
/// precision, or a timestamp source is invalid.
pub fn canonical_rows_to_mark_price_updates<I: Instrument + ?Sized>(
    table: &CanonicalMarkPricesTable,
    instrument: &I,
) -> Result<Vec<MarkPriceUpdate>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    table
        .rows
        .iter()
        .map(|row| canonical_row_to_mark_price_update(instrument_id, row, price_precision))
        .collect()
}

fn canonical_row_to_mark_price_update(
    instrument_id: InstrumentId,
    row: &CanonicalMarkPriceRow,
    price_precision: u8,
) -> Result<MarkPriceUpdate> {
    let price_str = rescaled(&row.value, price_precision)?;
    let value = Price::from_str(&price_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled value {price_str:?}: {error}"))?;
    let label = format!("mark price {}", row.event_time);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    Ok(MarkPriceUpdate::new(
        instrument_id,
        value,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical mark-price table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `MarkPriceUpdate` projection. NautilusTrader writes its
/// native `data/mark_prices/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_mark_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalMarkPricesTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data(instrument, table)?;
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
    let updates = canonical_rows_to_mark_price_updates(table, &instrument)?;
    let count = updates.len();

    ensure_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(updates, None, None, None)
        .context("write mark prices to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_MARK_PRICE_UPDATE.to_string(),
        trade_count: count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `MarkPriceUpdate` data back from `catalog_root`.
///
/// `MarkPriceUpdate` is keyed by bare instrument id under
/// `data/mark_prices/<id>/` exactly like trades/deltas/quotes (not bar-type
/// keyed), so this uses the file-filter path, not the bar query path.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_mark(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<MarkPriceUpdate>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files =
        catalog_files_for_instruments::<MarkPriceUpdate>(&catalog, catalog_root, &instrument_ids)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    catalog
        .query_typed_data::<MarkPriceUpdate>(None, None, None, None, Some(files), false)
        .context("query mark prices from catalog")
}

/// Convert canonical funding-rate rows into NautilusTrader
/// `FundingRateUpdate`s.
///
/// Funding rate `rate` is a `Decimal`, not a price, so this conversion does not
/// use instrument price precision. Timestamps route through the shared S1
/// receipt-clock owners ([`ts_event_nanos`]/[`ts_init_nanos`]).
///
/// # Errors
///
/// Returns an error if a rate cannot be parsed or a timestamp source is invalid.
pub fn canonical_rows_to_funding_rate_updates<I: Instrument + ?Sized>(
    table: &CanonicalFundingRatesTable,
    instrument: &I,
) -> Result<Vec<FundingRateUpdate>> {
    let instrument_id = instrument.id();
    table
        .rows
        .iter()
        .map(|row| canonical_row_to_funding_rate_update(instrument_id, row))
        .collect()
}

fn canonical_row_to_funding_rate_update(
    instrument_id: InstrumentId,
    row: &CanonicalFundingRateRow,
) -> Result<FundingRateUpdate> {
    let rate = Decimal::from_str(&row.rate)
        .map_err(|error| anyhow::anyhow!("invalid funding rate {:?}: {error}", row.rate))?;
    let label = format!("funding rate {}", row.event_time);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    let next_funding_ns = row
        .next_funding_time
        .map(|value| {
            let nanos = u64::try_from(value)
                .with_context(|| format!("{label}: negative next_funding_time {value}"))?;
            ensure!(nanos > 0, "{label}: non-positive next_funding_time {value}");
            Ok(UnixNanos::from(nanos))
        })
        .transpose()?;
    Ok(FundingRateUpdate::new(
        instrument_id,
        rate,
        row.interval_minutes,
        next_funding_ns,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical funding-rate table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors the point-update projections: validate, build the instrument, assert
/// the instrument id matches the canonical rows, convert, refuse a dirty root,
/// then write the instrument and the `FundingRateUpdate` projection.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_funding_rates_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalFundingRatesTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    let instrument_id = instrument.id();
    let instrument_id_text = instrument_id.to_string();
    for (index, row) in table.rows.iter().enumerate() {
        let row_instrument_id = row
            .nt_instrument_id
            .as_deref()
            .with_context(|| format!("row {index}: canonical row missing nt_instrument_id"))?;
        ensure!(
            instrument_id_text == row_instrument_id,
            "row {index}: instrument id {instrument_id} does not match canonical rows {}",
            row_instrument_id
        );
    }
    let updates = canonical_rows_to_funding_rate_updates(table, &instrument)?;
    let count = updates.len();

    ensure_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(updates, None, None, None)
        .context("write funding rates to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_FUNDING_RATE_UPDATE.to_string(),
        trade_count: count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `FundingRateUpdate` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_funding_rates(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<FundingRateUpdate>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments::<FundingRateUpdate>(
        &catalog,
        catalog_root,
        &instrument_ids,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    catalog
        .query_typed_data::<FundingRateUpdate>(None, None, None, None, Some(files), false)
        .context("query funding rates from catalog")
}

/// Convert canonical bar rows into NautilusTrader `Bar`s under the table's
/// externally-aggregated bar type, at the instrument's price/size precision.
///
/// Each row's OHLC is parsed at the instrument's price precision and the volume
/// at its size precision; `ts_event` is the row's `close_time` (the canonical
/// close is the bar's event instant) while `ts_init` is the row's
/// `availability_time` when present, else its `capture_time` (when the bar
/// became available to the system — the clock NautilusTrader replays by). The
/// OHLC ordering invariant the table's `validate()` already enforces is
/// re-checked by NautilusTrader's `Bar::new_checked`, so any residual
/// precision-rescale edge fails loud rather than panicking.
///
/// # Errors
///
/// Returns an error if an OHLCV value cannot be represented at the instrument
/// precision, the bar specification is invalid, or a close time is negative.
pub fn canonical_rows_to_bars<I: Instrument + ?Sized>(
    table: &CanonicalBarsTable,
    instrument: &I,
) -> Result<Vec<Bar>> {
    let instrument_id = instrument.id();
    let spec = BarSpecification::new_checked(
        table.bar_spec.step,
        table.bar_spec.aggregation,
        PriceType::Last,
    )
    .map_err(|error| anyhow::anyhow!("invalid bar specification: {error}"))?;
    let bar_type = BarType::new(instrument_id, spec, AggregationSource::External);
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| canonical_row_to_bar(bar_type, row, price_precision, size_precision))
        .collect()
}

fn canonical_row_to_bar(
    bar_type: BarType,
    row: &CanonicalBarRow,
    price_precision: u8,
    size_precision: u8,
) -> Result<Bar> {
    let price_at = |value: &str, label: &str| -> Result<Price> {
        let rescaled = rescaled(value, price_precision)?;
        Price::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid rescaled {label} {rescaled:?}: {error}"))
    };
    let open = price_at(&row.open, "open")?;
    let high = price_at(&row.high, "high")?;
    let low = price_at(&row.low, "low")?;
    let close = price_at(&row.close, "close")?;
    let volume_str = rescaled(&row.volume, size_precision)?;
    let volume = Quantity::from_str(&volume_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled volume {volume_str:?}: {error}"))?;
    let label = format!("bar close_time {}", row.close_time);
    let ts_event = ts_event_nanos(row.close_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    Bar::new_checked(bar_type, open, high, low, close, volume, ts_event, ts_init)
        .context("build bar")
}

/// Project a canonical bar table into a NautilusTrader `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `Bar` projection. NautilusTrader writes its native
/// `data/bars/<bar_type>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_bars_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalBarsTable,
    spec: &S,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = spec.build_instrument_any()?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data(instrument, table)?;
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
    let bars = canonical_rows_to_bars(table, &instrument)?;
    let bar_count = bars.len();

    ensure_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![instrument])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write bars to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_BAR.to_string(),
        trade_count: bar_count,
        catalog_hash: logical_catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected `Bar`
/// data back from `catalog_root`.
///
/// NautilusTrader keys the bar catalog directory by the full bar type (not the
/// bare instrument id), so this resolves files through NautilusTrader's own
/// identifier filtering (`query_typed_data` with the instrument id) rather than
/// the instrument-directory file filter used for trades and deltas.
///
/// NautilusTrader resolves that identifier by substring match against the
/// bar-type directory name, so an instrument id that is a strict prefix of
/// another could over-collect in a shared catalog. The projectors in this
/// module never produce that shape: every projection writes exactly one bar
/// type into a clean root, so each catalog holds one bar directory.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_bars(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<Bar>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<Bar>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query bars from catalog")
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
    let instrument_ids: Vec<String> = instruments
        .iter()
        .map(|instrument| instrument.id().to_string())
        .collect();
    let trade_files = catalog_files_for_instruments::<TradeTick>(&catalog, root, &instrument_ids)?;
    let mut ticks = if trade_files.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<TradeTick>(None, None, None, None, Some(trade_files), false)
            .context("query trade ticks from catalog for logical hash")?
    };
    ticks.sort_by_key(|tick| {
        (
            tick.ts_event.as_u64(),
            tick.trade_id.to_string(),
            tick.instrument_id.to_string(),
        )
    });
    let delta_files =
        catalog_files_for_instruments::<OrderBookDelta>(&catalog, root, &instrument_ids)?;
    let mut deltas = if delta_files.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<OrderBookDelta>(None, None, None, None, Some(delta_files), false)
            .context("query order book deltas from catalog for logical hash")?
    };
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
    // NautilusTrader keys the bar catalog directory by the full bar type, not by
    // the bare instrument id, so bars are resolved through NautilusTrader's own
    // identifier filtering (instrument ids passed to `query_typed_data`) rather
    // than the instrument-directory file filter used for trades and deltas.
    let mut bars = if instrument_ids.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<Bar>(Some(instrument_ids.clone()), None, None, None, None, true)
            .context("query bars from catalog for logical hash")?
    };
    bars.sort_by_key(|bar| {
        (
            bar.ts_event.as_u64(),
            bar.bar_type.to_string(),
            bar.open.as_decimal().to_string(),
            bar.high.as_decimal().to_string(),
            bar.low.as_decimal().to_string(),
            bar.close.as_decimal().to_string(),
            bar.volume.as_decimal().to_string(),
        )
    });
    let quote_files = catalog_files_for_instruments::<QuoteTick>(&catalog, root, &instrument_ids)?;
    let mut quotes = if quote_files.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<QuoteTick>(None, None, None, None, Some(quote_files), false)
            .context("query quote ticks from catalog for logical hash")?
    };
    quotes.sort_by_key(|quote| {
        (
            quote.ts_event.as_u64(),
            quote.instrument_id.to_string(),
            quote.bid_price.as_decimal().to_string(),
            quote.ask_price.as_decimal().to_string(),
            quote.bid_size.as_decimal().to_string(),
            quote.ask_size.as_decimal().to_string(),
        )
    });
    let index_files =
        catalog_files_for_instruments::<IndexPriceUpdate>(&catalog, root, &instrument_ids)?;
    let mut index_prices = if index_files.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<IndexPriceUpdate>(None, None, None, None, Some(index_files), false)
            .context("query index prices from catalog for logical hash")?
    };
    index_prices.sort_by_key(|p| {
        (
            p.ts_event.as_u64(),
            p.instrument_id.to_string(),
            p.value.as_decimal().to_string(),
            p.ts_init.as_u64(),
        )
    });
    let mark_files =
        catalog_files_for_instruments::<MarkPriceUpdate>(&catalog, root, &instrument_ids)?;
    let mut mark_prices = if mark_files.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<MarkPriceUpdate>(None, None, None, None, Some(mark_files), false)
            .context("query mark prices from catalog for logical hash")?
    };
    mark_prices.sort_by_key(|p| {
        (
            p.ts_event.as_u64(),
            p.instrument_id.to_string(),
            p.value.as_decimal().to_string(),
            p.ts_init.as_u64(),
        )
    });
    let funding_files =
        catalog_files_for_instruments::<FundingRateUpdate>(&catalog, root, &instrument_ids)?;
    let mut funding_rates = if funding_files.is_empty() {
        Vec::new()
    } else {
        catalog
            .query_typed_data::<FundingRateUpdate>(
                None,
                None,
                None,
                None,
                Some(funding_files),
                false,
            )
            .context("query funding rates from catalog for logical hash")?
    };
    funding_rates.sort_by(|a, b| {
        a.ts_event
            .cmp(&b.ts_event)
            .then_with(|| a.instrument_id.cmp(&b.instrument_id))
            .then_with(|| a.rate.cmp(&b.rate))
            .then_with(|| a.rate.to_string().cmp(&b.rate.to_string()))
            .then_with(|| a.interval.cmp(&b.interval))
            .then_with(|| a.next_funding_ns.cmp(&b.next_funding_ns))
            .then_with(|| a.ts_init.cmp(&b.ts_init))
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
    for bar in bars {
        hasher.update([19u8]);
        hasher.update(bar.bar_type.to_string().as_bytes());
        hasher.update([20u8]);
        hasher.update(bar.open.as_decimal().to_string().as_bytes());
        hasher.update([21u8]);
        hasher.update(bar.high.as_decimal().to_string().as_bytes());
        hasher.update([22u8]);
        hasher.update(bar.low.as_decimal().to_string().as_bytes());
        hasher.update([23u8]);
        hasher.update(bar.close.as_decimal().to_string().as_bytes());
        hasher.update([24u8]);
        hasher.update(bar.volume.as_decimal().to_string().as_bytes());
        hasher.update([25u8]);
        hasher.update(bar.ts_event.as_u64().to_string().as_bytes());
        hasher.update([26u8]);
        hasher.update(bar.ts_init.as_u64().to_string().as_bytes());
    }
    // Quote loop appended AFTER the bars loop with NEW unique domain-separator
    // tags 27..33 (existing tags: 0,2..8 ticks; 9..18 deltas; 19..26 bars). The
    // existing instrument/tick/delta/bar byte stream is unperturbed, so any
    // committed reference catalog that holds zero quote files keeps hashing to
    // its recorded value — this loop emits nothing for it.
    for quote in quotes {
        hasher.update([27u8]);
        hasher.update(quote.instrument_id.to_string().as_bytes());
        hasher.update([28u8]);
        hasher.update(quote.bid_price.as_decimal().to_string().as_bytes());
        hasher.update([29u8]);
        hasher.update(quote.ask_price.as_decimal().to_string().as_bytes());
        hasher.update([30u8]);
        hasher.update(quote.bid_size.as_decimal().to_string().as_bytes());
        hasher.update([31u8]);
        hasher.update(quote.ask_size.as_decimal().to_string().as_bytes());
        hasher.update([32u8]);
        hasher.update(quote.ts_event.as_u64().to_string().as_bytes());
        hasher.update([33u8]);
        hasher.update(quote.ts_init.as_u64().to_string().as_bytes());
    }
    // Index-price loop appended AFTER the quote loop with NEW unique
    // domain-separator tags 34..37 (existing tags end at 33 for quotes). Reusing
    // any earlier tag would let two different families hash equal; these are
    // fresh, so the committed reference catalog (which holds zero index files)
    // keeps hashing to its recorded value — this loop emits nothing for it.
    for index_price in index_prices {
        hasher.update([34u8]);
        hasher.update(index_price.instrument_id.to_string().as_bytes());
        hasher.update([35u8]);
        hasher.update(index_price.value.as_decimal().to_string().as_bytes());
        hasher.update([36u8]);
        hasher.update(index_price.ts_event.as_u64().to_string().as_bytes());
        hasher.update([37u8]);
        hasher.update(index_price.ts_init.as_u64().to_string().as_bytes());
    }
    // Mark-price loop appended AFTER the index loop with NEW unique
    // domain-separator tags 38..41 (existing tags end at 37 for index prices).
    // Reusing any earlier tag would let two different families hash equal; these
    // are fresh, so the committed reference catalog (which holds zero mark files)
    // keeps hashing to its recorded value — this loop emits nothing for it.
    for mark_price in mark_prices {
        hasher.update([38u8]);
        hasher.update(mark_price.instrument_id.to_string().as_bytes());
        hasher.update([39u8]);
        hasher.update(mark_price.value.as_decimal().to_string().as_bytes());
        hasher.update([40u8]);
        hasher.update(mark_price.ts_event.as_u64().to_string().as_bytes());
        hasher.update([41u8]);
        hasher.update(mark_price.ts_init.as_u64().to_string().as_bytes());
    }
    // Funding-rate loop appended AFTER the mark loop with NEW unique
    // domain-separator tags 42..47 (existing tags end at 41 for mark prices).
    // Empty funding catalogs emit nothing, preserving existing reference hashes.
    for funding_rate in funding_rates {
        hasher.update([42u8]);
        hasher.update(funding_rate.instrument_id.to_string().as_bytes());
        hasher.update([43u8]);
        hasher.update(funding_rate.rate.to_string().as_bytes());
        hasher.update([44u8]);
        if let Some(value) = funding_rate.interval {
            hasher.update(value.to_string().as_bytes());
        } else {
            hasher.update(b"<none>");
        }
        hasher.update([45u8]);
        if let Some(value) = funding_rate.next_funding_ns {
            hasher.update(value.as_u64().to_string().as_bytes());
        } else {
            hasher.update(b"<none>");
        }
        hasher.update([46u8]);
        hasher.update(funding_rate.ts_event.as_u64().to_string().as_bytes());
        hasher.update([47u8]);
        hasher.update(funding_rate.ts_init.as_u64().to_string().as_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

fn catalog_files_for_instruments<T: CatalogPathPrefix>(
    catalog: &ParquetDataCatalog,
    catalog_root: &Path,
    instrument_ids: &[String],
) -> Result<Vec<String>> {
    if instrument_ids.is_empty() {
        return Ok(Vec::new());
    }
    let safe_instrument_ids: HashSet<String> = instrument_ids
        .iter()
        .map(|id| urisafe_instrument_id(id))
        .collect();
    let files = catalog
        .query_files(T::path_prefix(), None, None, None)
        .with_context(|| format!("query {} files from catalog", T::path_prefix()))?;
    Ok(files
        .into_iter()
        .filter(|file| {
            file.rsplit('/').nth(1).is_some_and(|directory| {
                let decoded = urlencoding::decode(directory)
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| directory.to_string());
                let safe_directory = urisafe_instrument_id(&decoded);
                safe_instrument_ids.contains(&safe_directory)
            })
        })
        .map(|file| datafusion_catalog_file_path(catalog_root, &file))
        .collect())
}

fn datafusion_catalog_file_path(catalog_root: &Path, catalog_file: &str) -> String {
    if catalog_file.contains("://") || Path::new(catalog_file).is_absolute() {
        catalog_file.to_string()
    } else {
        catalog_root
            .join(catalog_file)
            .to_string_lossy()
            .to_string()
    }
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
        canonical_market_data::CanonicalBarSpec,
        canonical_trades::{CanonicalInstrumentIdentity, normalize_sample_spot_tick_trades},
        source_proof::{
            AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
            IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus,
            RequiredCheck, RequiredChecks, SourceCandidateClass, SourceProofClaimLimit,
            SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
            TimeRange, select_accepted_dataset,
        },
    };
    use nautilus_model::enums::BarAggregation;

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

    /// Same venue spec as `spec()` but with `price_increment = "0.01"` (precision
    /// 2). Used by ts_init/capture-clock validation tests whose table data values
    /// carry 2 decimal places (e.g. "617.05"). The canonical projection path
    /// always widens instrument precision to the data before calling
    /// `canonical_rows_to_*`; tests that call the conversion function directly
    /// must supply a pre-widened instrument so the precision gate in `rescaled`
    /// does not fire before the ts_init validation under test.
    fn spec_precision2() -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            price_increment: "0.01".to_string(),
            ..spec()
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

    fn binary_option_spec() -> CatalogInstrumentSpec {
        // max_notional, min_notional, max_price, min_price, margin_init, and
        // margin_maint are intentionally absent: the NT catalog Arrow schema
        // does not persist them for BinaryOption (decode_binary_option_batch
        // rev 6e059dc lines 412-417 hardcodes all six as None), so
        // build_binary_option rejects specs that set them to avoid silent
        // data loss on the projection round-trip.
        CatalogInstrumentSpec::BinaryOption(BinaryOptionInstrumentSpec {
            instrument_kind: BinaryOptionInstrumentKind::BinaryOption,
            nt_instrument_id: "YES.TESTVENUE".to_string(),
            raw_symbol: "YES".to_string(),
            asset_class: "ALTERNATIVE".to_string(),
            currency: "USDC".to_string(),
            activation_time_nanos: 1_700_000_000_000_000_000,
            expiration_time_nanos: 1_700_086_400_000_000_000,
            price_increment: "0.01".to_string(),
            size_increment: "0.001".to_string(),
            outcome: Some("Yes".to_string()),
            description: Some("Bounded binary option fixture".to_string()),
            // Distinct values so a max/min swap fails the assertion.
            max_quantity: Some("1000000".to_string()),
            min_quantity: Some("1".to_string()),
            max_notional: None,
            min_notional: None,
            max_price: None,
            min_price: None,
            margin_init: None,
            margin_maint: None,
            // Distinct values so a maker/taker swap fails the assertion.
            maker_fee: Some("0.001".to_string()),
            taker_fee: Some("0.002".to_string()),
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
    fn ts_event_nanos_rejects_non_positive_event_time() {
        // Event time is the per-row ordering clock validate() proved positive, so a
        // non-positive value here is an internal invariant breach: fail loud, never 0.
        let err = ts_event_nanos(0, "trade x").unwrap_err();
        assert!(err.to_string().contains("non-positive event time"), "{err}");
        let err = ts_event_nanos(-1, "trade x").unwrap_err();
        assert!(err.to_string().contains("negative event time"), "{err}");
    }

    #[test]
    fn ts_init_nanos_uses_capture_time_when_availability_none() {
        // No availability instant -> the worker receipt clock governs ts_init.
        assert_eq!(ts_init_nanos(None, 42, "trade x").unwrap().as_u64(), 42);
    }

    #[test]
    fn ts_init_nanos_prefers_availability_time_when_some() {
        // availability_time wins over capture_time when present (source order).
        assert_eq!(ts_init_nanos(Some(7), 42, "trade x").unwrap().as_u64(), 7);
    }

    #[test]
    fn ts_init_nanos_fails_loud_when_capture_invalid_and_no_availability() {
        // No availability and a non-positive capture clock must fail loud and name
        // the offending field, never fall back to the event clock or emit 0.
        let err = ts_init_nanos(None, 0, "trade x").unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
        let err = ts_init_nanos(None, -5, "trade x").unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn ts_init_nanos_fails_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must error rather than silently
        // fall back to capture_time.
        let err = ts_init_nanos(Some(0), 42, "trade x").unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
        let err = ts_init_nanos(Some(-1), 42, "trade x").unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn trade_ticks_ts_init_uses_capture_time_when_availability_none() {
        // canonical_table() rows carry availability_time=None and capture_time=42.
        // Every projected tick must stamp ts_init=42 (the receipt clock) while
        // ts_event stays the row's event_time (the event clock is preserved).
        let table = canonical_table();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("project trades");
        assert!(!ticks.is_empty(), "fixture must produce trades");
        for (tick, row) in ticks.iter().zip(table.rows.iter()) {
            assert_eq!(row.availability_time, None);
            assert_eq!(row.capture_time, 42);
            assert_eq!(tick.ts_init.as_u64(), 42);
            assert_eq!(
                tick.ts_event.as_u64(),
                u64::try_from(row.event_time).expect("positive event_time")
            );
        }
    }

    #[test]
    fn trade_ticks_ts_init_prefers_availability_time_when_some() {
        // With a source availability instant present, ts_init follows it over the
        // capture clock, while ts_event still preserves the event clock.
        let mut table = canonical_table();
        table.rows[0].availability_time = Some(7);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("project trades");
        assert_eq!(ticks[0].ts_init.as_u64(), 7);
        assert_eq!(
            ticks[0].ts_event.as_u64(),
            u64::try_from(table.rows[0].event_time).expect("positive event_time")
        );
    }

    #[test]
    fn trade_ticks_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming the
        // capture_time field rather than silently stamping ts_init=0.
        let mut table = canonical_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_trade_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn trade_ticks_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to the
        // (valid) capture clock.
        let mut table = canonical_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_trade_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
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

    fn binary_option_inner() -> BinaryOptionInstrumentSpec {
        let CatalogInstrumentSpec::BinaryOption(spec) = binary_option_spec() else {
            panic!("expected BinaryOption fixture");
        };
        spec
    }

    #[test]
    fn builds_binary_option_from_accepted_spec() {
        let instrument = build_catalog_instrument(&binary_option_spec()).expect("instrument");
        let InstrumentAny::BinaryOption(option) = instrument else {
            panic!("expected BinaryOption");
        };
        assert_eq!(option.id().to_string(), "YES.TESTVENUE");
        assert_eq!(option.raw_symbol.to_string(), "YES");
        assert_eq!(option.asset_class, AssetClass::Alternative);
        // Binary options carry one settlement/quote currency, not a base/quote
        // pair.
        assert_eq!(option.currency.to_string(), "USDC");
        assert_eq!(option.base_currency(), None);
        assert_eq!(option.quote_currency().to_string(), "USDC");
        assert_eq!(option.settlement_currency().to_string(), "USDC");
        assert_eq!(option.activation_ns.as_u64(), 1_700_000_000_000_000_000);
        assert_eq!(option.expiration_ns.as_u64(), 1_700_086_400_000_000_000);
        // Precision derives from the increments only (single-source-of-precision).
        assert_eq!(option.price_precision(), 2);
        assert_eq!(option.size_precision(), 3);
        assert_eq!(
            option.outcome.map(|o| o.to_string()),
            Some("Yes".to_string())
        );
        assert_eq!(
            option.description.map(|d| d.to_string()),
            Some("Bounded binary option fixture".to_string())
        );
        // Distinct max/min so a quantity swap would fail.
        assert_eq!(option.max_quantity, Some(Quantity::from("1000000")));
        assert_eq!(option.min_quantity, Some(Quantity::from("1")));
        // NT catalog Arrow schema does not persist these six fields for
        // BinaryOption (rev 6e059dc lines 412-417); build_binary_option
        // rejects specs that set them.
        assert_eq!(option.max_notional, None);
        assert_eq!(option.min_notional, None);
        assert_eq!(option.max_price(), None);
        assert_eq!(option.min_price(), None);
        assert_eq!(option.margin_init, Decimal::ZERO);
        assert_eq!(option.margin_maint, Decimal::ZERO);
        // Distinct maker/taker so a fee swap would fail.
        assert_eq!(option.maker_fee, Decimal::from_str("0.001").unwrap());
        assert_eq!(option.taker_fee, Decimal::from_str("0.002").unwrap());
    }

    #[test]
    fn build_binary_option_omits_optional_fields() {
        // Every Option<String> field absent must build a valid instrument: NT's
        // BinaryOption constructor accepts None for outcome/description, the
        // quantity/notional/price bounds, the margins, and the fees.
        let mut spec = binary_option_inner();
        spec.outcome = None;
        spec.description = None;
        spec.max_quantity = None;
        spec.min_quantity = None;
        spec.max_notional = None;
        spec.min_notional = None;
        spec.max_price = None;
        spec.min_price = None;
        spec.margin_init = None;
        spec.margin_maint = None;
        spec.maker_fee = None;
        spec.taker_fee = None;
        let option = build_binary_option(&spec).expect("optional fields default cleanly");
        assert_eq!(option.outcome, None);
        assert_eq!(option.description, None);
        assert_eq!(option.max_quantity, None);
        assert_eq!(option.min_notional, None);
    }

    #[test]
    fn build_binary_option_honours_trailing_zero_increment() {
        // Precision must agree with the increment's own precision, or NT's
        // BinaryOption precision-equality check would reject the instrument.
        let mut spec = binary_option_inner();
        spec.price_increment = "0.010".to_string();
        let option = build_binary_option(&spec).expect("trailing-zero increment");
        assert_eq!(option.price_precision(), 3);
    }

    #[test]
    fn build_binary_option_rejects_malformed_decimal() {
        let mut spec = binary_option_inner();
        spec.price_increment = "not-a-number".to_string();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_blank_raw_symbol() {
        let mut spec = binary_option_inner();
        spec.raw_symbol = String::new();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_blank_currency() {
        let mut spec = binary_option_inner();
        spec.currency = "   ".to_string();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_unknown_asset_class() {
        let mut spec = binary_option_inner();
        spec.asset_class = "NOT_AN_ASSET_CLASS".to_string();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_expiration_not_after_activation() {
        // The resolvable epoch must be a forward-bounded window, mirroring the
        // crypto-future activation/expiration ordering check.
        let mut spec = binary_option_inner();
        spec.expiration_time_nanos = spec.activation_time_nanos;
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_out_of_range_notional() {
        // The NT-schema guard fires before the Money parse, so max_notional
        // is_err() whether the value is out-of-range or merely present.
        let mut spec = binary_option_inner();
        spec.max_notional = Some("1e40".to_string());
        assert!(build_binary_option(&spec).is_err());
    }

    // Fix 1a — one negative test per NT-unsupported field.  Each confirms that
    // build_binary_option rejects a spec where the field is set, with an error
    // message naming the field and citing the NT catalog round-trip limitation.
    #[test]
    fn build_binary_option_rejects_max_notional() {
        let mut spec = binary_option_inner();
        spec.max_notional = Some("100000".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("max_notional"),
            "error must name the field: {msg}"
        );
        assert!(
            msg.contains("NT catalog Arrow schema does not persist"),
            "error must cite the round-trip reason: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_min_notional() {
        let mut spec = binary_option_inner();
        spec.min_notional = Some("1".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("min_notional"),
            "error must name the field: {msg}"
        );
        assert!(
            msg.contains("NT catalog Arrow schema does not persist"),
            "error must cite the round-trip reason: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_max_price() {
        let mut spec = binary_option_inner();
        spec.max_price = Some("1.00".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("max_price"),
            "error must name the field: {msg}"
        );
        assert!(
            msg.contains("NT catalog Arrow schema does not persist"),
            "error must cite the round-trip reason: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_min_price() {
        let mut spec = binary_option_inner();
        spec.min_price = Some("0.01".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("min_price"),
            "error must name the field: {msg}"
        );
        assert!(
            msg.contains("NT catalog Arrow schema does not persist"),
            "error must cite the round-trip reason: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_margin_init() {
        let mut spec = binary_option_inner();
        spec.margin_init = Some("0.05".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("margin_init"),
            "error must name the field: {msg}"
        );
        assert!(
            msg.contains("NT catalog Arrow schema does not persist"),
            "error must cite the round-trip reason: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_margin_maint() {
        let mut spec = binary_option_inner();
        spec.margin_maint = Some("0.03".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("margin_maint"),
            "error must name the field: {msg}"
        );
        assert!(
            msg.contains("NT catalog Arrow schema does not persist"),
            "error must cite the round-trip reason: {msg}"
        );
    }

    // Fix F5 — the shared catalog-persistability invariant is the single
    // production rule both projection paths enforce. A constructed BinaryOption
    // carrying any NT-non-persistable field must be rejected, and a clean one
    // must pass.
    #[test]
    fn ensure_binary_option_catalog_persistable_rejects_each_lost_field() {
        let clean = build_binary_option(&binary_option_inner()).expect("clean instrument");
        ensure_binary_option_catalog_persistable(&clean).expect("clean instrument is persistable");

        let with_price_bound = BinaryOption {
            max_price: Some(Price::from("0.999")),
            ..clean.clone()
        };
        let err = ensure_binary_option_catalog_persistable(&with_price_bound).unwrap_err();
        assert!(
            err.to_string().contains("max_price"),
            "error must name the lost field: {err}"
        );

        let with_margin = BinaryOption {
            margin_init: Decimal::new(5, 2),
            ..clean
        };
        let err = ensure_binary_option_catalog_persistable(&with_margin).unwrap_err();
        assert!(
            err.to_string().contains("margin_init"),
            "error must name the lost field: {err}"
        );
    }

    // Fix 4 — parse_optional_ustr empty-when-present rejection.
    #[test]
    fn build_binary_option_rejects_empty_outcome() {
        let mut spec = binary_option_inner();
        spec.outcome = Some(String::new());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("outcome must not be empty when present"),
            "error must name the field and state the rule: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_whitespace_only_description() {
        let mut spec = binary_option_inner();
        spec.description = Some("   ".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("description must not be empty when present"),
            "error must name the field and state the rule: {msg}"
        );
    }

    #[test]
    fn catalog_instrument_spec_deserializes_binary_option_shape() {
        let parsed: CatalogInstrumentSpec = toml::from_str(
            r#"
instrument_kind = "binary_option"
nt_instrument_id = "YES.TESTVENUE"
raw_symbol = "YES"
asset_class = "ALTERNATIVE"
currency = "USDC"
activation_time_nanos = 1700000000000000000
expiration_time_nanos = 1700086400000000000
price_increment = "0.01"
size_increment = "0.001"
"#,
        )
        .expect("binary option spec parses");
        assert!(matches!(parsed, CatalogInstrumentSpec::BinaryOption(_)));
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

    fn quote_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        bid: &str,
        ask: &str,
        bid_size: &str,
        ask_size: &str,
    ) -> CanonicalQuoteRow {
        CanonicalQuoteRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BNBUSDC".to_string(),
            canonical_instrument_key: "bybit/spot/BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: Some("BNBUSDC.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            bid: bid.to_string(),
            ask: ask.to_string(),
            bid_size: bid_size.to_string(),
            ask_size: ask_size.to_string(),
        }
    }

    // Distinct capture_time != event_time (and availability_time=None) so the
    // ts_init==capture_time proof is non-vacuous: it actually distinguishes the
    // receipt clock from the event clock.
    fn canonical_quotes_table() -> CanonicalQuotesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            quote_row(event_time, capture_time, None, "617.0", "617.1", "10", "12"),
            quote_row(
                event_time + 1,
                capture_time + 1,
                None,
                "617.1",
                "617.2",
                "8",
                "0",
            ),
        ];
        CanonicalQuotesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "spot".to_string(),
                product_category: "spot".to_string(),
                instrument_id: "BNBUSDC".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::QuoteReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_quote_ticks() {
        let table = canonical_quotes_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_quotes_to_catalog(&table, &spec(), dir.path()).expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_QUOTE_TICK);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_quotes(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        let mut loaded = loaded;
        loaded.sort_by_key(|quote| quote.ts_event.as_u64());
        for (quote, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(quote.instrument_id.to_string(), "BNBUSDC.BYBIT");
            assert_eq!(
                quote.bid_price.as_decimal(),
                Decimal::from_str(&row.bid).unwrap()
            );
            assert_eq!(
                quote.ask_price.as_decimal(),
                Decimal::from_str(&row.ask).unwrap()
            );
            assert_eq!(
                quote.bid_size.as_decimal(),
                Decimal::from_str(&row.bid_size).unwrap()
            );
            assert_eq!(
                quote.ask_size.as_decimal(),
                Decimal::from_str(&row.ask_size).unwrap()
            );
            // ts_event preserves the event clock; ts_init is the receipt clock.
            assert_eq!(
                quote.ts_event.as_u64(),
                u64::try_from(row.event_time).unwrap()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                quote.ts_init.as_u64(),
                u64::try_from(row.capture_time).unwrap(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn quote_ticks_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming
        // the capture_time field, never silently stamping ts_init=0.
        let mut table = canonical_quotes_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_quote_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn quote_ticks_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to
        // the (valid) capture clock.
        let mut table = canonical_quotes_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_quote_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn quote_table_validate_rejects_crossed_book() {
        let mut table = canonical_quotes_table();
        table.rows[0].ask = "0.40".to_string();
        let err = table.validate().expect_err("crossed book rejected");
        assert!(err.to_string().contains("below bid"), "{err}");
    }

    #[test]
    fn quote_catalog_hash_matches_projection() {
        // Proves the new quote query-back + hash block is wired into the logical
        // digest: recomputing over the written catalog reproduces the hash the
        // projection recorded.
        let table = canonical_quotes_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_quotes_to_catalog(&table, &spec(), dir.path()).unwrap();
        assert_eq!(
            projection.catalog_hash,
            logical_catalog_hash(dir.path()).unwrap(),
            "quote catalog hash must describe the logical quote catalog contents"
        );
    }

    #[test]
    fn quote_catalog_hash_is_deterministic_across_roots() {
        // No committed reference quote catalog exists, so determinism across two
        // independent roots is the pin: the same quote data must hash identically.
        let table = canonical_quotes_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_quotes_to_catalog(&table, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_quotes_to_catalog(&table, &spec(), dir_b.path()).unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same quote data must hash identically regardless of root"
        );
    }

    fn index_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        value: &str,
    ) -> CanonicalIndexPriceRow {
        CanonicalIndexPriceRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BNBUSDC".to_string(),
            canonical_instrument_key: "bybit/spot/BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: Some("BNBUSDC.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            value: value.to_string(),
        }
    }

    // Distinct capture_time != event_time (availability None) so the
    // ts_init==capture_time proof is non-vacuous. The values carry 2 decimals,
    // finer than the spec()'s 1-decimal (0.1) tick, so projection widens the
    // instrument price precision (exercising the index price_values view); the
    // empty size_values view leaves size precision unchanged.
    fn canonical_index_prices_table() -> CanonicalIndexPricesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            index_row(event_time, capture_time, None, "617.05"),
            index_row(event_time + 1, capture_time + 1, None, "617.15"),
        ];
        CanonicalIndexPricesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "spot".to_string(),
                product_category: "spot".to_string(),
                instrument_id: "BNBUSDC".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::IndexReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_index_prices() {
        let table = canonical_index_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_index_to_catalog(&table, &spec(), dir.path()).expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_INDEX_PRICE_UPDATE);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_index(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        let mut loaded = loaded;
        loaded.sort_by_key(|p| p.ts_event.as_u64());
        for (update, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(update.instrument_id.to_string(), "BNBUSDC.BYBIT");
            assert_eq!(
                update.value.as_decimal(),
                Decimal::from_str(&row.value).unwrap()
            );
            let label = format!("index price {}", row.event_time);
            // ts_event preserves the event clock; ts_init is the receipt clock.
            assert_eq!(
                update.ts_event.as_u64(),
                ts_event_nanos(row.event_time, &label).unwrap().as_u64()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                update.ts_init.as_u64(),
                ts_init_nanos(row.availability_time, row.capture_time, &label)
                    .unwrap()
                    .as_u64(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn index_projection_widens_price_precision_and_keeps_size_precision() {
        // The 2-decimal index values are finer than spec()'s 1-decimal (0.1)
        // tick, so projection widens price precision; index carries no size, so
        // the empty size_values view leaves the instrument size precision intact.
        let table = canonical_index_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_index_to_catalog(&table, &spec(), dir.path()).expect("project");
        let loaded = read_back_index(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert!(!loaded.is_empty());
        for update in &loaded {
            assert_eq!(
                update.value.precision, 2,
                "index value precision must widen to the data's 2 decimals"
            );
        }
        // spec()'s size_increment is 0.0001 (precision 4) and must be unchanged
        // by an index projection that contributes no size column.
        let instrument = build_currency_pair(&spec()).expect("instrument");
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn index_prices_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming
        // the capture_time field, never silently stamping ts_init=0.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_index_prices_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_index_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn index_prices_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to
        // the (valid) capture clock.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_index_prices_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_index_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn index_projection_refuses_dirty_catalog_root() {
        let table = canonical_index_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_index_to_catalog(&table, &spec(), dir.path())
            .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn index_catalog_hash_is_deterministic_across_roots() {
        // No committed reference index catalog exists, so determinism across two
        // independent roots is the pin: the same index data must hash identically.
        let table = canonical_index_prices_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_index_to_catalog(&table, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_index_to_catalog(&table, &spec(), dir_b.path()).unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same index data must hash identically regardless of root"
        );
    }

    #[test]
    fn index_catalog_hash_changes_with_data_content() {
        // Two index tables differing only in one row's value must hash
        // differently, proving the new [34..37] tag block covers index value
        // bytes (not just file paths).
        let table_a = canonical_index_prices_table();
        let mut table_b = canonical_index_prices_table();
        table_b.rows[0].value = "618.05".to_string();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_index_to_catalog(&table_a, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_index_to_catalog(&table_b, &spec(), dir_b.path()).unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different index value must change the catalog hash"
        );
    }

    fn mark_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        value: &str,
    ) -> CanonicalMarkPriceRow {
        CanonicalMarkPriceRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BNBUSDC".to_string(),
            canonical_instrument_key: "bybit/spot/BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: Some("BNBUSDC.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            value: value.to_string(),
        }
    }

    // Distinct capture_time != event_time (availability None) so the
    // ts_init==capture_time proof is non-vacuous. The values carry 2 decimals,
    // finer than the spec()'s 1-decimal (0.1) tick, so projection widens the
    // instrument price precision (exercising the mark price_values view); the
    // empty size_values view leaves size precision unchanged.
    fn canonical_mark_prices_table() -> CanonicalMarkPricesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            mark_row(event_time, capture_time, None, "617.05"),
            mark_row(event_time + 1, capture_time + 1, None, "617.15"),
        ];
        CanonicalMarkPricesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "spot".to_string(),
                product_category: "spot".to_string(),
                instrument_id: "BNBUSDC".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::MarkReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_mark_prices() {
        let table = canonical_mark_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_mark_to_catalog(&table, &spec(), dir.path()).expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_MARK_PRICE_UPDATE);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_mark(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        let mut loaded = loaded;
        loaded.sort_by_key(|p| p.ts_event.as_u64());
        for (update, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(update.instrument_id.to_string(), "BNBUSDC.BYBIT");
            assert_eq!(
                update.value.as_decimal(),
                Decimal::from_str(&row.value).unwrap()
            );
            let label = format!("mark price {}", row.event_time);
            // ts_event preserves the event clock; ts_init is the receipt clock.
            assert_eq!(
                update.ts_event.as_u64(),
                ts_event_nanos(row.event_time, &label).unwrap().as_u64()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                update.ts_init.as_u64(),
                ts_init_nanos(row.availability_time, row.capture_time, &label)
                    .unwrap()
                    .as_u64(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn mark_projection_widens_price_precision_and_keeps_size_precision() {
        // The 2-decimal mark values are finer than spec()'s 1-decimal (0.1)
        // tick, so projection widens price precision; mark carries no size, so
        // the empty size_values view leaves the instrument size precision intact.
        let table = canonical_mark_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_mark_to_catalog(&table, &spec(), dir.path()).expect("project");
        let loaded = read_back_mark(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert!(!loaded.is_empty());
        for update in &loaded {
            assert_eq!(
                update.value.precision, 2,
                "mark value precision must widen to the data's 2 decimals"
            );
        }
        // spec()'s size_increment is 0.0001 (precision 4) and must be unchanged
        // by a mark projection that contributes no size column.
        let instrument = build_currency_pair(&spec()).expect("instrument");
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn mark_prices_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming
        // the capture_time field, never silently stamping ts_init=0.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_mark_prices_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_mark_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn mark_prices_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to
        // the (valid) capture clock.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_mark_prices_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_mark_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn mark_projection_refuses_dirty_catalog_root() {
        let table = canonical_mark_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_mark_to_catalog(&table, &spec(), dir.path())
            .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn mark_catalog_hash_is_deterministic_across_roots() {
        // No committed reference mark catalog exists, so determinism across two
        // independent roots is the pin: the same mark data must hash identically.
        let table = canonical_mark_prices_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_mark_to_catalog(&table, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_mark_to_catalog(&table, &spec(), dir_b.path()).unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same mark data must hash identically regardless of root"
        );
    }

    #[test]
    fn mark_catalog_hash_changes_with_data_content() {
        // Two mark tables differing only in one row's value must hash
        // differently, proving the new [38..41] tag block covers mark value
        // bytes (not just file paths).
        let table_a = canonical_mark_prices_table();
        let mut table_b = canonical_mark_prices_table();
        table_b.rows[0].value = "618.05".to_string();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_mark_to_catalog(&table_a, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_mark_to_catalog(&table_b, &spec(), dir_b.path()).unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different mark value must change the catalog hash"
        );
    }

    fn funding_rate_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        rate: &str,
        interval_minutes: Option<u16>,
        next_funding_time: Option<i64>,
    ) -> CanonicalFundingRateRow {
        CanonicalFundingRateRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "perpetual".to_string(),
            product_category: "linear-perp".to_string(),
            instrument_id: "BTCUSDT".to_string(),
            canonical_instrument_key: "bybit/perpetual/BTCUSDT".to_string(),
            venue_symbol: "BTCUSDT".to_string(),
            nt_instrument_id: Some("BTCUSDT.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            rate: rate.to_string(),
            interval_minutes,
            next_funding_time,
        }
    }

    fn canonical_funding_rates_table() -> CanonicalFundingRatesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            funding_rate_row(
                event_time,
                capture_time,
                None,
                "-0.000100",
                Some(480),
                Some(event_time + 28_800_000_000_000),
            ),
            funding_rate_row(
                event_time + 1,
                capture_time + 1,
                None,
                "0.000250",
                Some(480),
                Some(event_time + 28_800_000_000_000),
            ),
        ];
        CanonicalFundingRatesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "perpetual".to_string(),
                product_category: "linear-perp".to_string(),
                instrument_id: "BTCUSDT".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::FundingReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_funding_rates() {
        let table = canonical_funding_rates_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
        )
        .expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_FUNDING_RATE_UPDATE);
        assert_eq!(projection.nt_instrument_id, "BTCUSDT.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_funding_rates(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        let mut loaded = loaded;
        loaded.sort_by_key(|p| p.ts_event.as_u64());
        for (update, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(update.instrument_id.to_string(), "BTCUSDT.BYBIT");
            assert_eq!(update.rate, Decimal::from_str(&row.rate).unwrap());
            assert_eq!(update.interval, row.interval_minutes);
            assert_eq!(
                update.next_funding_ns.map(|ts| ts.as_u64()),
                row.next_funding_time.map(|ts| u64::try_from(ts).unwrap())
            );
            let label = format!("funding rate {}", row.event_time);
            assert_eq!(
                update.ts_event.as_u64(),
                ts_event_nanos(row.event_time, &label).unwrap().as_u64()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                update.ts_init.as_u64(),
                ts_init_nanos(row.availability_time, row.capture_time, &label)
                    .unwrap()
                    .as_u64(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn read_back_funding_rates_returns_empty_when_catalog_has_no_funding_files() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let loaded = read_back_funding_rates(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert!(loaded.is_empty());
    }

    #[test]
    fn funding_projection_requires_nt_instrument_id() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].nt_instrument_id = None;
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
        )
        .expect_err("missing nt_instrument_id rejected");

        assert!(
            err.to_string().contains("missing nt_instrument_id"),
            "{err}"
        );
    }

    #[test]
    fn funding_projection_rejects_later_missing_nt_instrument_id() {
        let mut table = canonical_funding_rates_table();
        table.rows[1].nt_instrument_id = None;
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
        )
        .expect_err("later missing nt_instrument_id rejected");

        assert!(err.to_string().contains("row 1"), "{err}");
        assert!(
            err.to_string().contains("missing nt_instrument_id"),
            "{err}"
        );
    }

    #[test]
    fn funding_projection_rejects_nt_instrument_id_mismatch() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].nt_instrument_id = Some("ETHUSDT.BYBIT".to_string());
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
        )
        .expect_err("nt_instrument_id mismatch rejected");

        assert!(
            err.to_string().contains("does not match canonical rows"),
            "{err}"
        );
    }

    #[test]
    fn funding_projection_rejects_later_nt_instrument_id_mismatch() {
        let mut table = canonical_funding_rates_table();
        table.rows[1].nt_instrument_id = Some("ETHUSDT.BYBIT".to_string());
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
        )
        .expect_err("later nt_instrument_id mismatch rejected");

        assert!(err.to_string().contains("row 1"), "{err}");
        assert!(
            err.to_string().contains("does not match canonical rows"),
            "{err}"
        );
    }

    #[test]
    fn funding_rates_fail_loud_when_rate_is_malformed() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].rate = "not-a-decimal".to_string();
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("invalid funding rate"), "{err}");
    }

    #[test]
    fn funding_rates_fail_loud_when_next_funding_time_is_negative() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].next_funding_time = Some(-1);
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(
            err.to_string().contains("negative next_funding_time"),
            "{err}"
        );
    }

    #[test]
    fn funding_rates_fail_loud_when_next_funding_time_is_zero() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].next_funding_time = Some(0);
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(
            err.to_string().contains("non-positive next_funding_time"),
            "{err}"
        );
    }

    #[test]
    fn funding_rates_fail_loud_when_capture_invalid_and_no_availability() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn funding_rates_fail_loud_when_availability_some_but_invalid() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn funding_projection_refuses_dirty_catalog_root() {
        let table = canonical_funding_rates_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
        )
        .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn funding_catalog_hash_is_deterministic_across_roots() {
        let table = canonical_funding_rates_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir_a.path(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir_b.path(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same funding data must hash identically regardless of root"
        );
    }

    #[test]
    fn funding_catalog_hash_changes_with_data_content() {
        let table_a = canonical_funding_rates_table();
        let mut table_b = canonical_funding_rates_table();
        table_b.rows[0].rate = "-0.000200".to_string();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different funding rate must change the catalog hash"
        );
    }

    #[test]
    fn funding_catalog_hash_changes_with_interval_content() {
        let table_a = canonical_funding_rates_table();
        let mut table_b = canonical_funding_rates_table();
        table_b.rows[0].interval_minutes = Some(240);
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different funding interval must change the catalog hash"
        );
    }

    #[test]
    fn funding_catalog_hash_changes_with_next_funding_time_content() {
        let table_a = canonical_funding_rates_table();
        let mut table_b = canonical_funding_rates_table();
        table_b.rows[0].next_funding_time = Some(table_b.rows[0].event_time + 57_600_000_000_000);
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different next funding time must change the catalog hash"
        );
    }

    #[test]
    fn mark_section_does_not_change_trade_only_catalog_hash() {
        // The mark loop is appended AFTER the index loop with fresh tags 38..41
        // and emits nothing for an empty mark set, so a trade-only catalog must
        // still hash to expected_logical_catalog_hash (which hashes only the
        // instrument + ticks, no mark bytes). This protects the committed PMXT
        // hash pin against mark-section byte-tag drift.
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(&table, &spec(), dir.path()).unwrap();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("ticks");
        assert_eq!(
            projection.catalog_hash,
            expected_logical_catalog_hash(&instrument, &ticks),
            "an empty mark section must add zero bytes to a trade-only catalog hash"
        );
    }

    // Synthetic, token-agnostic fixtures for the precision-widening tests.
    // The behaviour under test is data-driven and must not be tied to any
    // real token, venue, or incident value (same precedent as the
    // `YES.TESTVENUE` binary-option fixture below).
    fn synthetic_spot_spec() -> SpotInstrumentSpec {
        let mut spec = spec();
        spec.nt_instrument_id = "BASEQUOTE.TESTVENUE".to_string();
        spec.raw_symbol = "BASEQUOTE".to_string();
        spec.base_currency = "BASE".to_string();
        spec.quote_currency = "QUOTE".to_string();
        spec
    }

    fn synthetic_perpetual_spec() -> CatalogInstrumentSpec {
        let CatalogInstrumentSpec::CryptoPerpetual(mut spec) = linear_perpetual_spec() else {
            panic!("expected CryptoPerpetual fixture");
        };
        spec.nt_instrument_id = "BASEQUOTE-PERP.TESTVENUE".to_string();
        spec.raw_symbol = "BASEQUOTE-PERP".to_string();
        spec.base_currency = "BASE".to_string();
        spec.quote_currency = "QUOTE".to_string();
        spec.settlement_currency = "QUOTE".to_string();
        CatalogInstrumentSpec::CryptoPerpetual(spec)
    }

    fn synthetic_identity(
        instrument_id: &str,
        nt_instrument_id: &str,
    ) -> CanonicalInstrumentIdentity {
        CanonicalInstrumentIdentity {
            instrument_id: instrument_id.to_string(),
            venue_symbol: instrument_id.to_string(),
            nt_instrument_id: nt_instrument_id.to_string(),
        }
    }

    fn synthetic_table(
        csv: &str,
        instrument_id: &str,
        nt_instrument_id: &str,
    ) -> CanonicalTradesTable {
        normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &synthetic_identity(instrument_id, nt_instrument_id),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize")
    }

    #[test]
    fn projection_widens_precision_when_archive_prints_are_finer_than_tick() {
        // Regression class: a venue's live instrument endpoint describes the
        // CURRENT tick (0.1 here), but the historical archive carries finer
        // prints (price scale 2, size scale 5 vs size precision 4). The
        // projection must widen the instrument to the accepted data's actual
        // scale instead of rejecting the accepted object.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.34,0.30001,buy,0\n\
            2,1772323312219,12.3,0.1456,sell,0\n";
        let table = synthetic_table(csv, "BASEQUOTE", "BASEQUOTE.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_trades_to_catalog(&table, &synthetic_spot_spec(), dir.path())
                .expect("projection widens precision instead of rejecting accepted data");
        assert_eq!(projection.trade_count, 2);

        // Read-back preserves the exact archived values.
        let loaded = read_back_trade_ticks(dir.path(), "BASEQUOTE.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("12.34"));
        assert_eq!(loaded[0].size, Quantity::from("0.30001"));

        // The catalog instrument carries the widened precision, with the tick
        // VALUE unchanged (0.1 -> 0.10, 0.0001 -> 0.00010).
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["BASEQUOTE.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments.len(), 1);
        assert_eq!(instruments[0].price_precision(), 2);
        assert_eq!(instruments[0].size_precision(), 5);
        assert_eq!(instruments[0].price_increment(), Price::from("0.10"));
        assert_eq!(instruments[0].size_increment(), Quantity::from("0.00010"));
    }

    #[test]
    fn projection_keeps_venue_precision_for_coarser_data() {
        // Coarse prints must NOT narrow the venue precision: a day of
        // whole-number trades keeps tick 0.1 / size precision 4 unchanged.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12,0.3,buy,0\n";
        let table = synthetic_table(csv, "BASEQUOTE", "BASEQUOTE.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &synthetic_spot_spec(), dir.path())
            .expect("project");
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["BASEQUOTE.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments[0].price_precision(), 1);
        assert_eq!(instruments[0].size_precision(), 4);
        assert_eq!(instruments[0].price_increment(), Price::from("0.1"));
        assert_eq!(instruments[0].size_increment(), Quantity::from("0.0001"));
    }

    #[test]
    fn widening_ignores_trailing_zeros_in_source_values() {
        // A source value like "12.30" normalizes to scale 1 — trailing zeros
        // must not force a widening (mirrors `rescaled`'s
        // normalize-before-check behaviour).
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.30,0.3000,buy,0\n";
        let table = synthetic_table(csv, "BASEQUOTE", "BASEQUOTE.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &synthetic_spot_spec(), dir.path())
            .expect("project");
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["BASEQUOTE.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments[0].price_precision(), 1);
        let loaded = read_back_trade_ticks(dir.path(), "BASEQUOTE.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("12.3"));
    }

    #[test]
    fn projection_widens_derivative_precision_when_data_is_finer() {
        // The widening is instrument-kind-agnostic: a CryptoPerpetual spec at
        // tick 0.1 / size precision 3 must also accept finer archived prints.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.34,0.3001,buy,0\n";
        let table = synthetic_table(csv, "BASEQUOTE-PERP", "BASEQUOTE-PERP.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &synthetic_perpetual_spec(), dir.path())
            .expect("derivative projection widens precision");
        let loaded =
            read_back_trade_ticks(dir.path(), "BASEQUOTE-PERP.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("12.34"));
        assert_eq!(loaded[0].size, Quantity::from("0.3001"));
    }

    #[test]
    fn projection_widens_binary_option_precision_when_data_is_finer() {
        // The widening arm covers binary options too: the YES.TESTVENUE fixture
        // is tick 0.01 / size precision 3, but a prediction-market archive can
        // carry finer prints (price scale 3, size scale 4). The projection must
        // widen the instrument to the data's actual scale instead of rejecting
        // the accepted object.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,0.491,10.0001,buy,0\n\
            2,1772323312219,0.512,12.5,sell,0\n";
        let table = synthetic_table(csv, "YES", "YES.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &binary_option_spec(), dir.path())
            .expect("binary option projection widens precision");

        let loaded = read_back_trade_ticks(dir.path(), "YES.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("0.491"));
        assert_eq!(loaded[0].size, Quantity::from("10.0001"));

        // The catalog instrument carries the widened precision, tick VALUE
        // unchanged (0.01 -> 0.010, 0.001 -> 0.0010).
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["YES.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments.len(), 1);
        assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));
        assert_eq!(instruments[0].price_precision(), 3);
        assert_eq!(instruments[0].size_precision(), 4);
        assert_eq!(instruments[0].price_increment(), Price::from("0.010"));
        assert_eq!(instruments[0].size_increment(), Quantity::from("0.0010"));

        // Fix 1b — per-field round-trip assertions for fields the NT catalog
        // Arrow schema DOES persist for BinaryOption (rev 6e059dc lines 408-419).
        // All six NT-unsupported fields (max_notional, min_notional, max_price,
        // min_price, margin_init, margin_maint) are rejected by build_binary_option
        // before the instrument is written, so they can never enter the catalog; the
        // round-trip assertion for them is the rejection test, not a read-back check.
        // Full InstrumentAny equality cannot pin field values here because
        // BinaryOption::PartialEq compares only `id` (rev 6e059dc line 248-254),
        // so per-field assertions are required.
        let InstrumentAny::BinaryOption(option) = &instruments[0] else {
            panic!("expected BinaryOption after catalog round-trip");
        };
        assert_eq!(
            option.outcome.map(|o| o.to_string()),
            Some("Yes".to_string()),
            "outcome must survive catalog round-trip"
        );
        assert_eq!(
            option.description.map(|d| d.to_string()),
            Some("Bounded binary option fixture".to_string()),
            "description must survive catalog round-trip"
        );
        assert_eq!(
            option.max_quantity,
            Some(Quantity::from("1000000")),
            "max_quantity must survive catalog round-trip"
        );
        assert_eq!(
            option.min_quantity,
            Some(Quantity::from("1")),
            "min_quantity must survive catalog round-trip"
        );
        assert_eq!(
            option.maker_fee,
            Decimal::from_str("0.001").unwrap(),
            "maker_fee must survive catalog round-trip"
        );
        assert_eq!(
            option.taker_fee,
            Decimal::from_str("0.002").unwrap(),
            "taker_fee must survive catalog round-trip"
        );
        // These six are rejected by build_binary_option (and therefore can never
        // reach the catalog write path), so they must decode as None here.
        assert_eq!(option.max_notional, None);
        assert_eq!(option.min_notional, None);
        assert_eq!(option.max_price(), None);
        assert_eq!(option.min_price(), None);
        assert_eq!(option.margin_init, Decimal::ZERO);
        assert_eq!(option.margin_maint, Decimal::ZERO);
    }

    fn binary_option_bar_row(
        open_time: i64,
        open: &str,
        high: &str,
        low: &str,
        close: &str,
        volume: &str,
    ) -> CanonicalBarRow {
        CanonicalBarRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-binary-option-bars".to_string(),
            source_binding: "kalshi-official-historical-api".to_string(),
            venue: "TESTVENUE".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            open_time,
            close_time: open_time + 60_000_000_000,
            capture_time: open_time + 60_000_000_500,
            availability_time: None,
            source_sequence: Some(open_time.to_string()),
            raw_payload_id: "kalshi-bars-sample-1".to_string(),
            source_proof_id:
                "source-proof-kalshi-official-historical-binary-option-pending-2026-06-08"
                    .to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            open: open.to_string(),
            high: high.to_string(),
            low: low.to_string(),
            close: close.to_string(),
            volume: volume.to_string(),
        }
    }

    fn binary_option_bars_table() -> CanonicalBarsTable {
        let base = 1_700_000_000_000_000_000;
        CanonicalBarsTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "TESTVENUE".to_string(),
                product_family: "prediction-market".to_string(),
                product_category: "binary".to_string(),
                instrument_id: "YES".to_string(),
                dt: "2023-11-14".to_string(),
            },
            source_proof_id:
                "source-proof-kalshi-official-historical-binary-option-pending-2026-06-08"
                    .to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            bar_spec: CanonicalBarSpec {
                step: 1,
                aggregation: BarAggregation::Minute,
            },
            rows: vec![
                binary_option_bar_row(base, "0.49", "0.55", "0.48", "0.52", "100"),
                binary_option_bar_row(
                    base + 60_000_000_000,
                    "0.52",
                    "0.58",
                    "0.51",
                    "0.57",
                    "120.5",
                ),
            ],
        }
    }

    #[test]
    fn binary_option_bar_catalog_projection_round_trips_through_nt_catalog() {
        let table = binary_option_bars_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_bars_to_catalog(&table, &binary_option_spec(), dir.path())
                .expect("project binary-option bars");
        assert_eq!(projection.trade_count, table.rows.len());
        assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
        assert_eq!(projection.nt_instrument_id, "YES.TESTVENUE");
        assert_eq!(
            projection.fidelity_class,
            SourceProofFidelityClass::TradeBarReplay
        );
        assert!(!projection.catalog_hash.is_empty());

        let mut loaded = read_back_bars(dir.path(), "YES.TESTVENUE").expect("read back bars");
        loaded.sort_by_key(|bar| bar.ts_event.as_u64());
        assert_eq!(loaded.len(), table.rows.len());
        assert_eq!(loaded[0].instrument_id().to_string(), "YES.TESTVENUE");
        assert_eq!(loaded[0].open, Price::from("0.49"));
        assert_eq!(loaded[0].high, Price::from("0.55"));
        assert_eq!(loaded[0].low, Price::from("0.48"));
        assert_eq!(loaded[0].close, Price::from("0.52"));
        assert_eq!(loaded[0].volume, Quantity::from("100.000"));
        assert_eq!(loaded[0].ts_event.as_u64(), 1_700_000_060_000_000_000);
        assert_eq!(loaded[0].ts_init.as_u64(), 1_700_000_060_000_000_500);

        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["YES.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments.len(), 1);
        assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));
    }

    #[test]
    fn projection_hashes_url_encoded_non_ascii_instrument_catalog() {
        let mut spec = synthetic_spot_spec();
        spec.nt_instrument_id = "币安人生USDC.BINANCE".to_string();
        spec.raw_symbol = "币安人生USDC".to_string();
        spec.base_currency = "币安人生".to_string();
        spec.quote_currency = "USDC".to_string();
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.34,0.3001,buy,0\n";
        let table = synthetic_table(csv, "币安人生USDC", "币安人生USDC.BINANCE");
        let dir = tempfile::TempDir::new().expect("temp dir");

        let projection = project_canonical_trades_to_catalog(&table, &spec, dir.path())
            .expect("project non-ASCII catalog path");
        let loaded = read_back_trade_ticks(dir.path(), "币安人生USDC.BINANCE").expect("read back");

        assert_eq!(projection.trade_count, 1);
        assert_eq!(projection.nt_instrument_id, "币安人生USDC.BINANCE");
        assert!(!projection.catalog_hash.is_empty());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].instrument_id.to_string(), "币安人生USDC.BINANCE");
    }

    #[test]
    fn datafusion_catalog_file_path_resolves_all_path_shapes() {
        // NT's resolve_path_for_datafusion passes absolute native paths and
        // full URIs through verbatim but URL-joins anything relative, which
        // percent-decodes encoded instrument directories into nonexistent
        // paths. Pin all three input shapes so the passthrough/join split
        // cannot silently regress.
        let root = Path::new("/catalog/root");

        assert_eq!(
            datafusion_catalog_file_path(root, "s3://bucket/data/trades/X.V/part-0.parquet"),
            "s3://bucket/data/trades/X.V/part-0.parquet"
        );
        assert_eq!(
            datafusion_catalog_file_path(root, "/elsewhere/data/trades/X.V/part-0.parquet"),
            "/elsewhere/data/trades/X.V/part-0.parquet"
        );
        assert_eq!(
            datafusion_catalog_file_path(root, "data/trades/X.V/part-0.parquet"),
            "/catalog/root/data/trades/X.V/part-0.parquet"
        );
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
                None, // tick_scheme (NT bump)
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
    fn projection_rejects_empty_canonical_table() {
        // Reproduction pin for the zero-row vacuous-pass concern: an empty
        // canonical table must fail loud at validate() before any catalog
        // write, so read-back can never compare 0 == 0 against an accepted
        // record.
        let mut table = canonical_table();
        table.rows.clear();
        let error = table.validate().expect_err("empty table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical trades table is empty")
        );
    }

    #[test]
    fn logical_catalog_hash_reproduces_committed_pmxt_reference_catalog_hash() {
        // Hash-invariance regression pin: the committed PMXT reference catalog
        // hash was recorded under the pre-explicit-file-list query mechanics.
        // Recomputing over the committed bytes must keep producing the
        // recorded value, or committed ledger records silently stop
        // verifying against their catalogs.
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root");
        let run_dir = repo_root.join(
            "specs/023-nt-research-analytics-platform/reference/pmxt-polymarket-selected-source-conversion/backtests/pmxt-run",
        );
        let metadata: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(run_dir.join("catalog-metadata.json"))
                .expect("read committed catalog metadata"),
        )
        .expect("parse committed catalog metadata");
        let recorded = metadata["catalog_hash"]
            .as_str()
            .expect("catalog_hash present in committed metadata");
        let recomputed =
            logical_catalog_hash(&run_dir.join("nt-catalog")).expect("recompute logical hash");
        assert_eq!(recomputed, recorded);
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
