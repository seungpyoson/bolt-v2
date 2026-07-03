use std::str::FromStr;

#[cfg(not(test))]
use nautilus_common::actor::DataActor;
use nautilus_core::Params;
#[cfg(not(test))]
use nautilus_model::enums::BookType;
use nautilus_model::{
    data::DataType,
    identifiers::{ClientId, InstrumentId},
};

use crate::{
    bolt_v3_book_sizing::OutcomeBookSubscriptions,
    bolt_v3_numeric::MILLIS_PER_SECOND_U64,
    bolt_v3_providers::{
        STRIKE_FETCH_INSTRUMENT_ID_PARAM, STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM,
        resolution_strike_fetch_request_data_type,
    },
    bolt_v3_reference_price::{
        ReferencePriceSubscriptionRequest, reference_price_source_is_runtime_available,
        reference_price_subscription_requests as build_reference_price_subscription_requests,
    },
    bolt_v3_trade_flow::SignedTradeFlow,
};

use super::{BinaryOracleEdgeTaker, COUNTER_INCREMENT_U64, signed_trade_flow_config};

impl BinaryOracleEdgeTaker {
    pub(super) fn retry_missing_live_input_subscriptions_at(&mut self, now_ms: u64) {
        self.refresh_realized_volatility_snapshot_at(now_ms);
        let signal_missing = self.pricing.spot_price().is_none();
        let reference_missing = self.reference_current_price_live_input_missing_at(now_ms);
        let realized_volatility_missing = self
            .pricing
            .latest_realized_vol_snapshot_for_surface(&self.config.realized_volatility_surface_id)
            .is_none();

        if !(signal_missing || reference_missing || realized_volatility_missing) {
            return;
        }

        log::info!(
            "binary_oracle_edge_taker retrying missing live input subscriptions: strategy_id={} signal_missing={} reference_missing={} realized_volatility_missing={}",
            self.config.strategy_id,
            signal_missing,
            reference_missing,
            realized_volatility_missing,
        );
        self.record_live_input_subscription_retry_event(LiveInputSubscriptionRetryEvent {
            signal_missing,
            reference_missing,
            realized_volatility_missing,
        });

        if reference_missing {
            self.unsubscribe_reference_prices();
            self.subscribe_reference_prices();
        }
        if signal_missing {
            self.unsubscribe_signal_quotes();
            self.subscribe_signal_quotes();
        }
        if realized_volatility_missing {
            self.unsubscribe_realized_volatility_sources();
            self.subscribe_realized_volatility_sources();
        }
    }

    fn reference_current_price_live_input_missing_at(&mut self, now_ms: u64) -> bool {
        if self.config.reference_current_price.is_none() {
            return false;
        }
        self.ensure_reference_price_runtime_state();
        let Some(reference_price) = &self.config.reference_current_price else {
            return false;
        };
        let Some(reference_price_selector) = self.reference_price_selector.as_ref() else {
            return false;
        };
        let active_interval = self
            .active
            .interval_start_ms
            .zip(self.active.interval_end_ms);
        let quotes = self
            .reference_price_quotes
            .values()
            .cloned()
            .collect::<Vec<_>>();

        let mut runtime_source_count = usize::MIN;
        let mut current_source_count = usize::MIN;
        let mut required_source_missing = false;

        for source_id in &reference_price.source_order {
            let Some(source) = reference_price.sources.get(source_id) else {
                continue;
            };
            if !reference_price_source_is_runtime_available(reference_price, source) {
                continue;
            }
            runtime_source_count =
                runtime_source_count.saturating_add(COUNTER_INCREMENT_U64 as usize);
            let source_current =
                active_interval.is_some_and(|(interval_start_ms, interval_end_ms)| {
                    reference_price_selector
                        .valid_quote_for_source(
                            source_id,
                            interval_start_ms,
                            interval_end_ms,
                            now_ms,
                            &quotes,
                        )
                        .is_some()
                });
            if source_current {
                current_source_count =
                    current_source_count.saturating_add(COUNTER_INCREMENT_U64 as usize);
            } else if source.required {
                required_source_missing = true;
            }
        }

        runtime_source_count != usize::MIN
            && (required_source_missing || current_source_count < reference_price.min_valid_sources)
    }

    pub(super) fn signal_instrument_id(&self) -> Option<InstrumentId> {
        self.config
            .signal_instrument_id
            .as_deref()
            .and_then(|instrument_id| InstrumentId::from_str(instrument_id).ok())
    }

    pub(super) fn signal_client_id(&self) -> Option<ClientId> {
        self.config.signal_venue.as_deref().map(ClientId::from)
    }

    pub(super) fn resolution_instrument_id(&self) -> Option<InstrumentId> {
        self.config
            .resolution_instrument_id
            .as_deref()
            .and_then(|instrument_id| InstrumentId::from_str(instrument_id).ok())
    }

    pub(super) fn resolution_client_id(&self) -> Option<ClientId> {
        self.config
            .resolution_client_id
            .as_deref()
            .map(ClientId::from)
    }

    /// True when the configured resolution instrument resolves THIS instance's
    /// `underlying_asset`: its leading symbol segment (before the `-USD` quote)
    /// must equal `underlying_asset` (e.g. `BTC-USD.CHAINLINK` for a `BTC`
    /// instance). One strategy instance trades exactly one asset, so this is the
    /// fail-closed binding that stops a wrong-asset feed (or a wrapped-asset
    /// variant) from ever supplying the strike.
    fn resolution_instrument_resolves_underlying_asset(&self, instrument_id: InstrumentId) -> bool {
        instrument_id
            .symbol
            .as_str()
            .split('-')
            .next()
            .is_some_and(|asset| asset.eq_ignore_ascii_case(self.config.underlying_asset.as_str()))
    }

    pub(super) fn subscribe_signal_quotes(&mut self) {
        if let Some(instrument_id) = self.signal_instrument_id() {
            let client_id = self.signal_client_id();
            #[cfg(not(test))]
            self.subscribe_quotes(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
    }

    pub(super) fn subscribe_realized_volatility_sources(&mut self) {
        let surface_id = self.config.realized_volatility_surface_id.clone();
        let quote_requests = self
            .context
            .realized_volatility_quote_subscription_requests_for_surface(&surface_id);
        let trade_requests = self
            .context
            .realized_volatility_trade_subscription_requests_for_surface(&surface_id);
        let index_requests = self
            .context
            .realized_volatility_index_subscription_requests_for_surface(&surface_id);

        // Defense-in-depth: make a zero-subscription configured surface observable. For a
        // validated config this is typically unreachable because policy requires at least one
        // enabled quorum source, but it still catches a validation regression or
        // no-ready-source edge that would otherwise leave pricing silently
        // `RealizedVolNotReady`. Pricing fails closed regardless; this warning is the only
        // operator signal.
        if quote_requests.is_empty() && trade_requests.is_empty() && index_requests.is_empty() {
            log::warn!(
                "binary_oracle_edge_taker configured RV surface `{}` has no enabled subscribable sources; pricing will stay RealizedVolNotReady (strategy_id={})",
                surface_id,
                self.config.strategy_id
            );
        }

        for (instrument_id, client_id) in quote_requests {
            #[cfg(not(test))]
            self.subscribe_quotes(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
        for (instrument_id, client_id) in trade_requests {
            #[cfg(not(test))]
            self.subscribe_trades(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
        for (instrument_id, client_id) in index_requests {
            #[cfg(not(test))]
            self.subscribe_index_prices(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
    }

    pub(super) fn unsubscribe_signal_quotes(&mut self) {
        if let Some(instrument_id) = self.signal_instrument_id() {
            let client_id = self.signal_client_id();
            #[cfg(not(test))]
            self.unsubscribe_quotes(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
    }

    pub(super) fn subscribe_reference_prices(&mut self) {
        for subscription in self.reference_price_subscription_requests() {
            #[cfg(not(test))]
            self.subscribe_data(
                subscription.data_type.clone(),
                Some(subscription.client_id),
                Some(subscription.params.clone()),
            );
            self.record_reference_price_subscribe_event(ReferencePriceSubscribeEvent::subscribe(
                &subscription,
            ));
        }
    }

    pub(super) fn unsubscribe_reference_prices(&mut self) {
        for subscription in self.reference_price_subscription_requests() {
            #[cfg(not(test))]
            self.unsubscribe_data(
                subscription.data_type.clone(),
                Some(subscription.client_id),
                Some(subscription.params.clone()),
            );
            self.record_reference_price_subscribe_event(ReferencePriceSubscribeEvent::unsubscribe(
                &subscription,
            ));
        }
    }

    fn reference_price_subscription_requests(&self) -> Vec<ReferencePriceSubscriptionRequest> {
        let Some(reference_price) = &self.config.reference_current_price else {
            return Vec::new();
        };
        match build_reference_price_subscription_requests(reference_price) {
            Ok(subscriptions) => subscriptions,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker invalid reference price subscription request: {error}; strategy_id={}",
                    self.config.strategy_id,
                );
                Vec::new()
            }
        }
    }

    pub(super) fn unsubscribe_realized_volatility_sources(&mut self) {
        let surface_id = self.config.realized_volatility_surface_id.clone();
        for (instrument_id, client_id) in self
            .context
            .realized_volatility_quote_subscription_requests_for_surface(&surface_id)
        {
            #[cfg(not(test))]
            self.unsubscribe_quotes(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
        for (instrument_id, client_id) in self
            .context
            .realized_volatility_trade_subscription_requests_for_surface(&surface_id)
        {
            #[cfg(not(test))]
            self.unsubscribe_trades(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
        for (instrument_id, client_id) in self
            .context
            .realized_volatility_index_subscription_requests_for_surface(&surface_id)
        {
            #[cfg(not(test))]
            self.unsubscribe_index_prices(instrument_id, client_id, None);
            #[cfg(test)]
            let _ = (instrument_id, client_id);
        }
    }

    /// Fetches the live resolution strike for the current market interval.
    ///
    /// The first call keeps one durable index-price subscription open so NT
    /// will route the provider's [`IndexPriceUpdate`] into `on_index_price`.
    /// Later retry ticks use provider-owned custom fetch commands with unique
    /// data types, avoiding NT's per-instrument index subscribe dedup.
    pub(super) fn subscribe_resolution_strike(&mut self) {
        let (Some(resolution_instrument_id), Some(resolution_client_id), Some(interval_start_ms)) = (
            self.resolution_instrument_id(),
            self.resolution_client_id(),
            self.active.interval_start_ms,
        ) else {
            return;
        };
        // Fail-closed asset binding: refuse to subscribe a resolution instrument
        // that does not resolve this instance's underlying asset, so a
        // misconfigured (wrong-asset or wrapped-variant) feed can never bind the
        // strike. The entry gate stays blocked while price_to_beat is None.
        if !self.resolution_instrument_resolves_underlying_asset(resolution_instrument_id) {
            log::error!(
                "binary_oracle_edge_taker resolution instrument {} does not resolve underlying asset {}; refusing live strike subscribe (fail-closed): strategy_id={}",
                resolution_instrument_id,
                self.config.underlying_asset,
                self.config.strategy_id,
            );
            return;
        }
        let window_open_unix_seconds = interval_start_ms / MILLIS_PER_SECOND_U64;
        let mut params = Params::new();
        params.insert(
            STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM.to_string(),
            serde_json::json!(window_open_unix_seconds),
        );
        if self.resolution_strike_index_subscription.as_ref() != Some(&resolution_instrument_id) {
            let previous_custom_subscription = self.resolution_strike_custom_subscription.take();
            #[cfg(not(test))]
            if let Some(data_type) = previous_custom_subscription {
                self.unsubscribe_data(data_type, Some(resolution_client_id), None);
            }
            #[cfg(test)]
            let _ = previous_custom_subscription;

            let previous_index_subscription = self
                .resolution_strike_index_subscription
                .replace(resolution_instrument_id);
            #[cfg(not(test))]
            if let Some(instrument_id) = previous_index_subscription {
                self.unsubscribe_index_prices(instrument_id, Some(resolution_client_id), None);
            }
            #[cfg(test)]
            let _ = previous_index_subscription;

            #[cfg(not(test))]
            self.subscribe_index_prices(
                resolution_instrument_id,
                Some(resolution_client_id),
                Some(params.clone()),
            );
            #[cfg(test)]
            let _ = (resolution_client_id, params);
            self.record_resolution_strike_subscribe_event(
                ResolutionStrikeSubscribeEvent::durable_index(
                    resolution_instrument_id,
                    window_open_unix_seconds,
                ),
            );
            return;
        }

        params.insert(
            STRIKE_FETCH_INSTRUMENT_ID_PARAM.to_string(),
            serde_json::json!(resolution_instrument_id.to_string()),
        );
        self.resolution_strike_fetch_sequence = self
            .resolution_strike_fetch_sequence
            .wrapping_add(COUNTER_INCREMENT_U64);
        let data_type = resolution_strike_fetch_request_data_type(
            resolution_instrument_id,
            self.resolution_strike_fetch_sequence,
        );
        let previous_custom_subscription = self
            .resolution_strike_custom_subscription
            .replace(data_type.clone());
        #[cfg(not(test))]
        {
            if let Some(previous_data_type) = previous_custom_subscription {
                self.unsubscribe_data(previous_data_type, Some(resolution_client_id), None);
            }
            self.subscribe_data(data_type.clone(), Some(resolution_client_id), Some(params));
        }
        #[cfg(test)]
        let _ = (resolution_client_id, previous_custom_subscription, params);
        self.record_resolution_strike_subscribe_event(
            ResolutionStrikeSubscribeEvent::custom_fetch(
                resolution_instrument_id,
                window_open_unix_seconds,
                self.resolution_strike_fetch_sequence,
                data_type,
            ),
        );
    }

    pub(super) fn unsubscribe_resolution_strike(&mut self) {
        let Some(resolution_client_id) = self.resolution_client_id() else {
            self.resolution_strike_index_subscription = None;
            self.resolution_strike_custom_subscription = None;
            return;
        };
        #[cfg(test)]
        let _ = resolution_client_id;
        if let Some(data_type) = self.resolution_strike_custom_subscription.take() {
            #[cfg(not(test))]
            self.unsubscribe_data(data_type, Some(resolution_client_id), None);
            #[cfg(test)]
            let _ = data_type;
        }
        if let Some(instrument_id) = self.resolution_strike_index_subscription.take() {
            #[cfg(not(test))]
            self.unsubscribe_index_prices(instrument_id, Some(resolution_client_id), None);
            #[cfg(test)]
            let _ = instrument_id;
        }
    }

    pub(super) fn replace_book_subscriptions(&mut self, next: OutcomeBookSubscriptions) {
        let current = self.book_subscriptions.clone();
        unsubscribe_missing_books(self, &current, &next);
        subscribe_new_books(self, &current, &next);
        self.book_subscriptions = next;
    }
}

fn unsubscribe_missing_books(
    strategy: &mut BinaryOracleEdgeTaker,
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) {
    if let Some(instrument_id) = current.up_instrument_id
        && next.up_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        #[cfg(not(test))]
        strategy.unsubscribe_trades(instrument_id, None, None);
        strategy.active.trade_flow.remove(&instrument_id);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
    if let Some(instrument_id) = current.down_instrument_id
        && next.down_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        #[cfg(not(test))]
        strategy.unsubscribe_trades(instrument_id, None, None);
        strategy.active.trade_flow.remove(&instrument_id);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
    if let Some(instrument_id) = current.tracked_position_instrument_id
        && next.tracked_position_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        #[cfg(not(test))]
        strategy.unsubscribe_trades(instrument_id, None, None);
        strategy.active.trade_flow.remove(&instrument_id);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
}

fn subscribe_new_books(
    strategy: &mut BinaryOracleEdgeTaker,
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) {
    if let Some(instrument_id) = next.up_instrument_id
        && current.up_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        #[cfg(not(test))]
        strategy.subscribe_trades(instrument_id, None, None);
        let trade_flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&strategy.config));
        strategy.active.trade_flow.insert(instrument_id, trade_flow);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
    if let Some(instrument_id) = next.down_instrument_id
        && current.down_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        #[cfg(not(test))]
        strategy.subscribe_trades(instrument_id, None, None);
        let trade_flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&strategy.config));
        strategy.active.trade_flow.insert(instrument_id, trade_flow);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
    if let Some(instrument_id) = next.tracked_position_instrument_id
        && current.tracked_position_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        #[cfg(not(test))]
        strategy.subscribe_trades(instrument_id, None, None);
        let trade_flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&strategy.config));
        strategy.active.trade_flow.insert(instrument_id, trade_flow);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BookSubscriptionEvent {
    pub(super) action: &'static str,
    pub(super) instrument_id: InstrumentId,
}

const BOOK_SUBSCRIBE_ACTION: &str = stringify!(subscribe);
const BOOK_UNSUBSCRIBE_ACTION: &str = stringify!(unsubscribe);

impl BookSubscriptionEvent {
    pub(super) fn subscribe(instrument_id: InstrumentId) -> Self {
        Self {
            action: BOOK_SUBSCRIBE_ACTION,
            instrument_id,
        }
    }

    pub(super) fn unsubscribe(instrument_id: InstrumentId) -> Self {
        Self {
            action: BOOK_UNSUBSCRIBE_ACTION,
            instrument_id,
        }
    }
}

impl BinaryOracleEdgeTaker {
    fn record_book_subscription_event(&mut self, event: BookSubscriptionEvent) {
        #[cfg(test)]
        self.book_subscription_events.push(event);
        #[cfg(not(test))]
        let _ = event;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LiveInputSubscriptionRetryEvent {
    pub(super) signal_missing: bool,
    pub(super) reference_missing: bool,
    pub(super) realized_volatility_missing: bool,
}

impl BinaryOracleEdgeTaker {
    fn record_live_input_subscription_retry_event(
        &mut self,
        event: LiveInputSubscriptionRetryEvent,
    ) {
        #[cfg(test)]
        self.live_input_subscription_retry_events.push(event);
        #[cfg(not(test))]
        let _ = event;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReferencePriceSubscribeEvent {
    pub(super) action: &'static str,
    pub(super) source_id: String,
    pub(super) provider: String,
    pub(super) client_id: ClientId,
    pub(super) data_type: DataType,
    pub(super) params: Params,
}

pub(super) const REFERENCE_PRICE_SUBSCRIBE_ACTION: &str = stringify!(subscribe);
pub(super) const REFERENCE_PRICE_UNSUBSCRIBE_ACTION: &str = stringify!(unsubscribe);

impl ReferencePriceSubscribeEvent {
    fn subscribe(subscription: &ReferencePriceSubscriptionRequest) -> Self {
        Self::from_subscription(REFERENCE_PRICE_SUBSCRIBE_ACTION, subscription)
    }

    fn unsubscribe(subscription: &ReferencePriceSubscriptionRequest) -> Self {
        Self::from_subscription(REFERENCE_PRICE_UNSUBSCRIBE_ACTION, subscription)
    }

    fn from_subscription(
        action: &'static str,
        subscription: &ReferencePriceSubscriptionRequest,
    ) -> Self {
        Self {
            action,
            source_id: subscription.source_id.clone(),
            provider: subscription.provider.clone(),
            client_id: subscription.client_id,
            data_type: subscription.data_type.clone(),
            params: subscription.params.clone(),
        }
    }
}

impl BinaryOracleEdgeTaker {
    fn record_reference_price_subscribe_event(&mut self, event: ReferencePriceSubscribeEvent) {
        #[cfg(test)]
        self.reference_price_subscribe_events.push(event);
        #[cfg(not(test))]
        let _ = event;
    }
}

/// One recorded resolution-strike fetch trigger. Constructed unconditionally
/// so the production ordering is the same code the test observes; only the
/// storage is test-only (see
/// [`BinaryOracleEdgeTaker::record_resolution_strike_subscribe_event`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolutionStrikeSubscribeEvent {
    pub(super) action: &'static str,
    pub(super) trigger: ResolutionStrikeFetchTrigger,
    pub(super) instrument_id: InstrumentId,
    pub(super) window_open_unix_seconds: u64,
    pub(super) request_sequence: Option<u64>,
    pub(super) custom_data_type: Option<DataType>,
}

const RESOLUTION_STRIKE_SUBSCRIBE_ACTION: &str = stringify!(subscribe);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolutionStrikeFetchTrigger {
    DurableIndex,
    CustomFetch,
}

impl ResolutionStrikeSubscribeEvent {
    fn durable_index(instrument_id: InstrumentId, window_open_unix_seconds: u64) -> Self {
        Self {
            action: RESOLUTION_STRIKE_SUBSCRIBE_ACTION,
            trigger: ResolutionStrikeFetchTrigger::DurableIndex,
            instrument_id,
            window_open_unix_seconds,
            request_sequence: None,
            custom_data_type: None,
        }
    }

    fn custom_fetch(
        instrument_id: InstrumentId,
        window_open_unix_seconds: u64,
        request_sequence: u64,
        custom_data_type: DataType,
    ) -> Self {
        Self {
            action: RESOLUTION_STRIKE_SUBSCRIBE_ACTION,
            trigger: ResolutionStrikeFetchTrigger::CustomFetch,
            instrument_id,
            window_open_unix_seconds,
            request_sequence: Some(request_sequence),
            custom_data_type: Some(custom_data_type),
        }
    }
}

impl BinaryOracleEdgeTaker {
    fn record_resolution_strike_subscribe_event(&mut self, event: ResolutionStrikeSubscribeEvent) {
        #[cfg(test)]
        self.resolution_strike_subscribe_events.push(event);
        #[cfg(not(test))]
        let _ = event;
    }

    #[cfg(test)]
    pub(super) fn resolution_strike_subscribe_count(&self) -> u32 {
        u32::try_from(
            self.resolution_strike_subscribe_events
                .iter()
                .filter(|event| event.action == RESOLUTION_STRIKE_SUBSCRIBE_ACTION)
                .count(),
        )
        .unwrap_or(u32::MAX)
    }
}
