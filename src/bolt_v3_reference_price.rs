use std::{collections::BTreeMap, sync::Arc};

use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    data::{CustomData, DataType},
    identifiers::ClientId,
};
use nautilus_persistence_macros::custom_data;

use crate::{
    bolt_v3_config::{
        ReferencePriceBlock, ReferencePriceDriftPolicy, ReferencePriceProvider,
        ReferencePriceSourceBlock,
    },
    bolt_v3_numeric::NANOS_PER_MILLI_U64,
    bolt_v3_providers::reference_price_provider_supports_asset,
};

const REFERENCE_PRICE_UPDATE_TYPE: &str = "BoltV3ReferencePriceUpdate";
const REFERENCE_PRICE_ASSET_METADATA_FIELD: &str = "asset";
const REFERENCE_PRICE_SOURCE_KEY_METADATA_FIELD: &str = "source_key";
const REFERENCE_PRICE_PROVIDER_METADATA_FIELD: &str = "provider";

pub const REFERENCE_PRICE_ASSET_PARAM: &str = REFERENCE_PRICE_ASSET_METADATA_FIELD;
pub const REFERENCE_PRICE_SOURCE_KEY_PARAM: &str = REFERENCE_PRICE_SOURCE_KEY_METADATA_FIELD;
pub const REFERENCE_PRICE_PROVIDER_PARAM: &str = REFERENCE_PRICE_PROVIDER_METADATA_FIELD;
pub const REFERENCE_PRICE_INSTRUMENT_ID_PARAM: &str = "instrument_id";
pub const REFERENCE_PRICE_SYMBOL_PARAM: &str = "symbol";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePriceSubscriptionRequest {
    pub source_id: String,
    pub provider: String,
    pub client_id: ClientId,
    pub data_type: DataType,
    pub params: Params,
}

pub fn reference_price_subscription_requests(
    reference_price: &ReferencePriceBlock,
) -> Result<Vec<ReferencePriceSubscriptionRequest>, String> {
    let mut subscriptions = Vec::new();
    for source_id in &reference_price.source_order {
        let Some(source) = reference_price.sources.get(source_id) else {
            continue;
        };
        if !reference_price_source_is_runtime_available(reference_price, source) {
            continue;
        }

        let provider = source.provider.as_str();
        let data_type = ReferencePriceUpdate::data_type_for(
            &reference_price.asset,
            source_id.as_str(),
            provider,
        )?;
        let mut params = Params::new();
        params.insert(
            REFERENCE_PRICE_ASSET_PARAM.to_string(),
            serde_json::json!(reference_price.asset),
        );
        params.insert(
            REFERENCE_PRICE_SOURCE_KEY_PARAM.to_string(),
            serde_json::json!(source_id),
        );
        params.insert(
            REFERENCE_PRICE_PROVIDER_PARAM.to_string(),
            serde_json::json!(provider),
        );
        if let Some(instrument_id) = &source.instrument_id {
            params.insert(
                REFERENCE_PRICE_INSTRUMENT_ID_PARAM.to_string(),
                serde_json::json!(instrument_id),
            );
        }
        if let Some(symbol) = &source.symbol {
            params.insert(
                REFERENCE_PRICE_SYMBOL_PARAM.to_string(),
                serde_json::json!(symbol),
            );
        }
        subscriptions.push(ReferencePriceSubscriptionRequest {
            source_id: source_id.clone(),
            provider: provider.to_string(),
            client_id: source.client_id,
            data_type,
            params,
        });
    }
    Ok(subscriptions)
}

pub(crate) fn reference_price_source_is_runtime_available(
    reference_price: &ReferencePriceBlock,
    source: &ReferencePriceSourceBlock,
) -> bool {
    source.enabled && !reference_price_source_is_unsupported(reference_price, source)
}

pub(crate) fn reference_price_source_is_unsupported(
    reference_price: &ReferencePriceBlock,
    source: &ReferencePriceSourceBlock,
) -> bool {
    !reference_price_provider_supports_asset(source.provider.as_str(), &reference_price.asset)
}

#[custom_data]
pub struct ReferencePriceUpdate {
    asset: String,
    source_id: String,
    provider: String,
    provider_instrument: String,
    price: f64,
    #[custom_data_field(serde)]
    bid: Option<f64>,
    #[custom_data_field(serde)]
    ask: Option<f64>,
    observed_ts_ms: u64,
    received_ts_ms: u64,
    #[custom_data_field(serde)]
    provenance: ReferenceQuoteProvenance,
    ts_event: UnixNanos,
    ts_init: UnixNanos,
}

impl ReferencePriceUpdate {
    #[expect(
        clippy::too_many_arguments,
        reason = "normalized reference-price custom data carries explicit provider, price, and timing fields"
    )]
    pub fn try_new(
        asset: impl Into<String>,
        source_id: impl Into<String>,
        provider: impl Into<String>,
        provider_instrument: impl Into<String>,
        price: f64,
        bid: Option<f64>,
        ask: Option<f64>,
        observed_ts_ms: u64,
        received_ts_ms: u64,
    ) -> Result<Self, String> {
        Self::try_new_with_provenance(
            asset,
            source_id,
            provider,
            provider_instrument,
            price,
            bid,
            ask,
            observed_ts_ms,
            received_ts_ms,
            ReferenceQuoteProvenance::empty(),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "normalized reference-price custom data carries explicit provider, price, timing, and provenance fields"
    )]
    pub fn try_new_with_provenance(
        asset: impl Into<String>,
        source_id: impl Into<String>,
        provider: impl Into<String>,
        provider_instrument: impl Into<String>,
        price: f64,
        bid: Option<f64>,
        ask: Option<f64>,
        observed_ts_ms: u64,
        received_ts_ms: u64,
        provenance: ReferenceQuoteProvenance,
    ) -> Result<Self, String> {
        let asset = asset.into();
        let source_id = source_id.into();
        let provider = provider.into();
        let provider_instrument = provider_instrument.into();
        validate_reference_identity_field(&asset, "asset")?;
        validate_reference_identity_field(&source_id, "source_id")?;
        validate_reference_identity_field(&provider, "provider")?;
        validate_reference_identity_field(&provider_instrument, "provider_instrument")?;
        validate_reference_values(price, bid, ask, observed_ts_ms, received_ts_ms)?;
        let ts_event = reference_timestamp_ms_to_unix_nanos(observed_ts_ms, "observed_ts_ms")?;
        let ts_init = reference_timestamp_ms_to_unix_nanos(received_ts_ms, "received_ts_ms")?;

        Ok(Self {
            asset,
            source_id,
            provider,
            provider_instrument,
            price,
            bid,
            ask,
            observed_ts_ms,
            received_ts_ms,
            provenance,
            ts_event,
            ts_init,
        })
    }

    pub fn data_type_for(asset: &str, source_id: &str, provider: &str) -> Result<DataType, String> {
        validate_reference_identity_field(asset, "asset")?;
        validate_reference_identity_field(source_id, "source_id")?;
        validate_reference_identity_field(provider, "provider")?;
        let asset = asset.to_string();
        let mut metadata = Params::new();
        metadata.insert(
            REFERENCE_PRICE_ASSET_METADATA_FIELD.to_string(),
            serde_json::json!(asset.clone()),
        );
        metadata.insert(
            REFERENCE_PRICE_SOURCE_KEY_METADATA_FIELD.to_string(),
            serde_json::json!(source_id),
        );
        metadata.insert(
            REFERENCE_PRICE_PROVIDER_METADATA_FIELD.to_string(),
            serde_json::json!(provider),
        );
        Ok(DataType::new(
            REFERENCE_PRICE_UPDATE_TYPE,
            Some(metadata),
            Some(asset),
        ))
    }

    pub fn data_type(&self) -> DataType {
        Self::data_type_for(&self.asset, &self.source_id, &self.provider)
            .expect("reference price update identity was validated at construction")
    }

    pub fn to_custom_data(&self) -> CustomData {
        CustomData::new(Arc::new(self.clone()), self.data_type())
    }

    pub fn from_custom_data(custom: &CustomData) -> Option<&Self> {
        custom.data.as_any().downcast_ref::<Self>()
    }

    pub fn to_reference_quote(&self) -> Result<ReferenceQuote, String> {
        let provider = ReferencePriceProvider::new(self.provider.clone())?;
        ReferenceQuote::try_new_with_provenance(
            self.asset.as_str(),
            self.source_id.as_str(),
            provider,
            self.provider_instrument.as_str(),
            self.price,
            self.bid,
            self.ask,
            self.observed_ts_ms,
            self.received_ts_ms,
            self.provenance.clone(),
        )
    }

    pub fn asset(&self) -> &str {
        self.asset.as_str()
    }

    pub fn source_id(&self) -> &str {
        self.source_id.as_str()
    }

    pub fn provider(&self) -> &str {
        self.provider.as_str()
    }

    pub fn provider_instrument(&self) -> &str {
        self.provider_instrument.as_str()
    }

    pub const fn price(&self) -> f64 {
        self.price
    }

    pub const fn observed_ts_ms(&self) -> u64 {
        self.observed_ts_ms
    }

    pub const fn received_ts_ms(&self) -> u64 {
        self.received_ts_ms
    }

    pub const fn provenance(&self) -> &ReferenceQuoteProvenance {
        &self.provenance
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReferenceQuoteProvenance {
    fields: BTreeMap<String, String>,
}

impl ReferenceQuoteProvenance {
    pub const fn empty() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    pub fn try_from_fields(fields: BTreeMap<String, String>) -> Result<Self, String> {
        for (key, value) in &fields {
            validate_provenance_component(key, "key")?;
            validate_provenance_component(value, "value")?;
            let key_lower = key.to_ascii_lowercase();
            if provenance_component_is_secret_bearing_key(&key_lower)
                || provenance_component_is_secret_bearing_value(value)
            {
                return Err(
                    "reference quote provenance must not contain secret-bearing fields".to_string(),
                );
            }
        }
        Ok(Self { fields })
    }

    pub fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferencePriceSelection {
    source_id: String,
    price: f64,
    failed_over: bool,
}

impl ReferencePriceSelection {
    pub fn selected(source_id: impl Into<String>, price: f64, failed_over: bool) -> Self {
        Self {
            source_id: source_id.into(),
            price,
            failed_over,
        }
    }

    pub fn source_id(&self) -> &str {
        self.source_id.as_str()
    }

    pub const fn price(&self) -> f64 {
        self.price
    }

    pub const fn failed_over(&self) -> bool {
        self.failed_over
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferencePriceSourceStatus {
    Available,
    Disabled,
    UnsupportedSymbol,
    AuthRejected,
    SubscriptionRejected,
    Silent,
    Stale,
    MalformedFrame,
    Disconnected,
    DriftExceeded,
}

impl ReferencePriceSourceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Disabled => "disabled",
            Self::UnsupportedSymbol => "unsupported_symbol",
            Self::AuthRejected => "auth_rejected",
            Self::SubscriptionRejected => "subscription_rejected",
            Self::Silent => "silent",
            Self::Stale => "stale",
            Self::MalformedFrame => "malformed_frame",
            Self::Disconnected => "disconnected",
            Self::DriftExceeded => "drift_exceeded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePriceSourceHealth {
    source_id: String,
    provider: ReferencePriceProvider,
    status: ReferencePriceSourceStatus,
    observed_ts_ms: Option<u64>,
    received_ts_ms: Option<u64>,
}

impl ReferencePriceSourceHealth {
    pub fn new(
        source_id: impl Into<String>,
        provider: ReferencePriceProvider,
        status: ReferencePriceSourceStatus,
        observed_ts_ms: Option<u64>,
        received_ts_ms: Option<u64>,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            provider,
            status,
            observed_ts_ms,
            received_ts_ms,
        }
    }

    pub fn available(quote: &ReferenceQuote) -> Self {
        Self::new(
            quote.source_id.as_str(),
            quote.provider.clone(),
            ReferencePriceSourceStatus::Available,
            Some(quote.observed_ts_ms),
            Some(quote.received_ts_ms),
        )
    }

    pub fn update(
        &mut self,
        status: ReferencePriceSourceStatus,
        observed_ts_ms: Option<u64>,
        received_ts_ms: Option<u64>,
    ) {
        self.status = status;
        self.observed_ts_ms = observed_ts_ms;
        self.received_ts_ms = received_ts_ms;
    }

    pub fn source_id(&self) -> &str {
        self.source_id.as_str()
    }

    pub fn provider(&self) -> &ReferencePriceProvider {
        &self.provider
    }

    pub const fn status(&self) -> ReferencePriceSourceStatus {
        self.status
    }

    pub const fn observed_ts_ms(&self) -> Option<u64> {
        self.observed_ts_ms
    }

    pub const fn received_ts_ms(&self) -> Option<u64> {
        self.received_ts_ms
    }
}

#[derive(Debug, Clone)]
pub struct ReferencePriceSelector {
    asset: String,
    sources: Vec<String>,
    required_sources: Vec<String>,
    min_valid_sources: usize,
    max_source_staleness_ms: u64,
    max_cross_source_drift_bps: u32,
    drift_policy: ReferencePriceDriftPolicy,
    last_cross_source_drift_bps: Option<f64>,
    interval_start_ms: Option<u64>,
    interval_end_ms: Option<u64>,
    selected_source_id: Option<String>,
    failover_used: bool,
}

impl ReferencePriceSelector {
    pub fn new(
        asset: impl Into<String>,
        sources: impl Into<Vec<String>>,
        min_valid_sources: usize,
        max_source_staleness_ms: u64,
        max_cross_source_drift_bps: u32,
    ) -> Result<Self, String> {
        let source_specs = sources
            .into()
            .into_iter()
            .map(ReferencePriceSourceSpec::optional)
            .collect::<Vec<_>>();
        Self::new_with_source_specs_and_drift_policy(
            asset,
            source_specs,
            min_valid_sources,
            max_source_staleness_ms,
            max_cross_source_drift_bps,
            ReferencePriceDriftPolicy::Observe,
        )
    }

    pub fn new_with_drift_policy(
        asset: impl Into<String>,
        sources: impl Into<Vec<String>>,
        min_valid_sources: usize,
        max_source_staleness_ms: u64,
        max_cross_source_drift_bps: u32,
        drift_policy: ReferencePriceDriftPolicy,
    ) -> Result<Self, String> {
        let source_specs = sources
            .into()
            .into_iter()
            .map(ReferencePriceSourceSpec::optional)
            .collect::<Vec<_>>();
        Self::new_with_source_specs_and_drift_policy(
            asset,
            source_specs,
            min_valid_sources,
            max_source_staleness_ms,
            max_cross_source_drift_bps,
            drift_policy,
        )
    }

    pub fn new_with_source_specs(
        asset: impl Into<String>,
        source_specs: impl Into<Vec<ReferencePriceSourceSpec>>,
        min_valid_sources: usize,
        max_source_staleness_ms: u64,
        max_cross_source_drift_bps: u32,
    ) -> Result<Self, String> {
        Self::new_with_source_specs_and_drift_policy(
            asset,
            source_specs,
            min_valid_sources,
            max_source_staleness_ms,
            max_cross_source_drift_bps,
            ReferencePriceDriftPolicy::Observe,
        )
    }

    pub fn new_with_source_specs_and_drift_policy(
        asset: impl Into<String>,
        source_specs: impl Into<Vec<ReferencePriceSourceSpec>>,
        min_valid_sources: usize,
        max_source_staleness_ms: u64,
        max_cross_source_drift_bps: u32,
        drift_policy: ReferencePriceDriftPolicy,
    ) -> Result<Self, String> {
        let asset = asset.into();
        if asset.trim().is_empty()
            || asset.trim() != asset
            || asset.chars().any(char::is_whitespace)
        {
            return Err("reference price selector asset is invalid".to_string());
        }

        let source_specs = source_specs.into();
        if source_specs.is_empty() {
            return Err("reference price selector sources must not be empty".to_string());
        }
        let sources = source_specs
            .iter()
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        let required_sources = source_specs
            .iter()
            .filter(|source| source.required)
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        if min_valid_sources == 0 || min_valid_sources > sources.len() {
            return Err("reference price selector min_valid_sources is invalid".to_string());
        }

        Ok(Self {
            asset,
            sources,
            required_sources,
            min_valid_sources,
            max_source_staleness_ms,
            max_cross_source_drift_bps,
            drift_policy,
            last_cross_source_drift_bps: None,
            interval_start_ms: None,
            interval_end_ms: None,
            selected_source_id: None,
            failover_used: false,
        })
    }

    pub fn select(
        &mut self,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
        quotes: &[ReferenceQuote],
    ) -> Option<ReferencePriceSelection> {
        if self.interval_start_ms != Some(interval_start_ms)
            || self.interval_end_ms != Some(interval_end_ms)
        {
            self.interval_start_ms = Some(interval_start_ms);
            self.interval_end_ms = Some(interval_end_ms);
            self.selected_source_id = None;
            self.failover_used = false;
        }

        let valid_quotes =
            self.valid_quotes_by_order(interval_start_ms, interval_end_ms, now_ms, quotes);
        self.last_cross_source_drift_bps = Self::max_cross_source_drift_bps(&valid_quotes);
        if valid_quotes.len() < self.min_valid_sources {
            return None;
        }
        if self.required_sources.iter().any(|required| {
            !valid_quotes
                .iter()
                .any(|quote| quote.source_id == *required)
        }) {
            return None;
        }
        if self.drift_policy == ReferencePriceDriftPolicy::Block
            && self
                .last_cross_source_drift_bps
                .is_some_and(|drift_bps| drift_bps > f64::from(self.max_cross_source_drift_bps))
        {
            return None;
        }

        if let Some(selected_source_id) = self.selected_source_id.clone()
            && let Some(quote) = valid_quotes
                .iter()
                .find(|quote| quote.source_id == selected_source_id)
        {
            return Some(ReferencePriceSelection::selected(
                selected_source_id,
                quote.price,
                self.failover_used,
            ));
        }

        let next = valid_quotes.first()?;
        let failed_over =
            self.selected_source_id.is_some() || self.sources.first() != Some(&next.source_id);
        self.selected_source_id = Some(next.source_id.clone());
        if failed_over {
            self.failover_used = true;
        }

        Some(ReferencePriceSelection::selected(
            next.source_id.clone(),
            next.price,
            self.failover_used,
        ))
    }

    fn valid_quotes_by_order<'a>(
        &self,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
        quotes: &'a [ReferenceQuote],
    ) -> Vec<&'a ReferenceQuote> {
        self.sources
            .iter()
            .filter_map(|source_id| {
                self.valid_quote_for_source(
                    source_id,
                    interval_start_ms,
                    interval_end_ms,
                    now_ms,
                    quotes,
                )
            })
            .collect()
    }

    pub(crate) fn valid_quote_for_source<'a>(
        &self,
        source_id: &str,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
        quotes: &'a [ReferenceQuote],
    ) -> Option<&'a ReferenceQuote> {
        quotes
            .iter()
            .filter(|quote| {
                quote.asset == self.asset
                    && quote.source_id == source_id
                    && quote.observed_ts_ms >= interval_start_ms
                    && quote.observed_ts_ms <= interval_end_ms
                    && quote.observed_ts_ms <= now_ms
                    && now_ms.saturating_sub(quote.observed_ts_ms) <= self.max_source_staleness_ms
            })
            .max_by_key(|quote| quote.observed_ts_ms)
    }

    pub const fn last_cross_source_drift_bps(&self) -> Option<f64> {
        self.last_cross_source_drift_bps
    }

    fn max_cross_source_drift_bps(quotes: &[&ReferenceQuote]) -> Option<f64> {
        let mut max_drift_bps: Option<f64> = None;
        for (index, lhs) in quotes.iter().enumerate() {
            for rhs in quotes.iter().skip(index + 1) {
                let denominator = lhs.price.min(rhs.price);
                let drift_bps = ((lhs.price - rhs.price).abs() / denominator) * 10_000.0;
                max_drift_bps = Some(max_drift_bps.map_or(drift_bps, |max| max.max(drift_bps)));
            }
        }
        max_drift_bps
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePriceSourceSpec {
    source_id: String,
    required: bool,
}

impl ReferencePriceSourceSpec {
    pub fn optional(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            required: false,
        }
    }

    pub fn required(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            required: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceQuote {
    asset: String,
    source_id: String,
    provider: ReferencePriceProvider,
    provider_instrument: String,
    price: f64,
    bid: Option<f64>,
    ask: Option<f64>,
    observed_ts_ms: u64,
    received_ts_ms: u64,
    provenance: ReferenceQuoteProvenance,
}

impl ReferenceQuote {
    #[expect(
        clippy::too_many_arguments,
        reason = "normalized reference quote constructor mirrors the provider contract fields"
    )]
    pub fn try_new(
        asset: impl Into<String>,
        source_id: impl Into<String>,
        provider: ReferencePriceProvider,
        provider_instrument: impl Into<String>,
        price: f64,
        bid: Option<f64>,
        ask: Option<f64>,
        observed_ts_ms: u64,
        received_ts_ms: u64,
    ) -> Result<Self, String> {
        Self::try_new_with_provenance(
            asset,
            source_id,
            provider,
            provider_instrument,
            price,
            bid,
            ask,
            observed_ts_ms,
            received_ts_ms,
            ReferenceQuoteProvenance::empty(),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "normalized reference quote constructor mirrors the provider contract fields including provenance"
    )]
    pub fn try_new_with_provenance(
        asset: impl Into<String>,
        source_id: impl Into<String>,
        provider: ReferencePriceProvider,
        provider_instrument: impl Into<String>,
        price: f64,
        bid: Option<f64>,
        ask: Option<f64>,
        observed_ts_ms: u64,
        received_ts_ms: u64,
        provenance: ReferenceQuoteProvenance,
    ) -> Result<Self, String> {
        let asset = asset.into();
        let source_id = source_id.into();
        let provider_instrument = provider_instrument.into();
        validate_reference_identity_field(&asset, "asset")?;
        validate_reference_identity_field(&source_id, "source_id")?;
        validate_reference_identity_field(&provider_instrument, "provider_instrument")?;
        validate_reference_values(price, bid, ask, observed_ts_ms, received_ts_ms)?;

        Ok(Self {
            asset,
            source_id,
            provider,
            provider_instrument,
            price,
            bid,
            ask,
            observed_ts_ms,
            received_ts_ms,
            provenance,
        })
    }

    pub fn asset(&self) -> &str {
        self.asset.as_str()
    }

    pub fn source_id(&self) -> &str {
        self.source_id.as_str()
    }

    pub fn provider(&self) -> &ReferencePriceProvider {
        &self.provider
    }

    pub fn provider_instrument(&self) -> &str {
        self.provider_instrument.as_str()
    }

    pub const fn price(&self) -> f64 {
        self.price
    }

    pub const fn bid(&self) -> Option<f64> {
        self.bid
    }

    pub const fn ask(&self) -> Option<f64> {
        self.ask
    }

    pub const fn observed_ts_ms(&self) -> u64 {
        self.observed_ts_ms
    }

    pub const fn received_ts_ms(&self) -> u64 {
        self.received_ts_ms
    }

    pub const fn provenance(&self) -> &ReferenceQuoteProvenance {
        &self.provenance
    }
}

fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn validate_reference_identity_field(value: &str, field: &'static str) -> Result<(), String> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(format!("reference price {field} is invalid"));
    }
    Ok(())
}

fn validate_provenance_component(value: &str, field: &'static str) -> Result<(), String> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(format!("reference quote provenance {field} is invalid"));
    }
    Ok(())
}

fn provenance_component_is_secret_bearing_key(key_lower: &str) -> bool {
    key_lower == "auth"
        || key_lower.contains("secret")
        || key_lower.contains("authorization")
        || key_lower.contains("auth_header")
        || key_lower.contains("credential")
        || key_lower.contains("api_key")
        || key_lower.contains("apikey")
        || key_lower.contains("token")
        || key_lower.contains("password")
}

fn provenance_component_is_secret_bearing_value(value: &str) -> bool {
    let value_lower = value.to_ascii_lowercase();
    value_lower.starts_with("bearer ")
        || value_lower.starts_with("basic ")
        || value_lower.starts_with("apikey ")
        || value_lower.starts_with("api-key ")
        || value_lower.starts_with("token ")
}

fn validate_reference_values(
    price: f64,
    bid: Option<f64>,
    ask: Option<f64>,
    observed_ts_ms: u64,
    received_ts_ms: u64,
) -> Result<(), String> {
    if !is_positive_finite(price) {
        return Err("reference quote price must be positive and finite".to_string());
    }

    if !bid.is_none_or(is_positive_finite) {
        return Err("reference quote bid must be positive and finite".to_string());
    }

    if !ask.is_none_or(is_positive_finite) {
        return Err("reference quote ask must be positive and finite".to_string());
    }

    if let (Some(bid), Some(ask)) = (bid, ask)
        && bid > ask
    {
        return Err("reference quote bid must not exceed ask".to_string());
    }

    if observed_ts_ms == 0 {
        return Err("reference quote observed_ts_ms must be positive".to_string());
    }

    if received_ts_ms == 0 {
        return Err("reference quote received_ts_ms must be positive".to_string());
    }

    Ok(())
}

fn reference_timestamp_ms_to_unix_nanos(
    timestamp_ms: u64,
    field: &'static str,
) -> Result<UnixNanos, String> {
    timestamp_ms
        .checked_mul(NANOS_PER_MILLI_U64)
        .map(UnixNanos::from)
        .ok_or_else(|| format!("reference quote {field} cannot convert to UnixNanos"))
}
