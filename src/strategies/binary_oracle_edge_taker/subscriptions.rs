use std::str::FromStr;

use anyhow::Result;
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
        SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM, STRIKE_FETCH_INSTRUMENT_ID_PARAM,
        STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM, resolution_strike_fetch_request_data_type,
    },
    bolt_v3_reference_price::{
        ReferencePriceSubscriptionRequest,
        reference_price_subscription_requests as build_reference_price_subscription_requests,
    },
    bolt_v3_timestamp_domain::{NtStrategyClockMs, VenueEventMs},
    bolt_v3_trade_flow::SignedTradeFlow,
};

use super::{BinaryOracleEdgeTaker, COUNTER_INCREMENT_U64, signed_trade_flow_config};

impl BinaryOracleEdgeTaker {
    pub(super) fn retry_missing_live_input_subscriptions_at(&mut self, now_ms: u64) {
        self.refresh_realized_volatility_snapshot_at(now_ms);
        let signal_missing =
            self.config.signal_new_risk_available && self.pricing.spot_price().is_none();
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
            if let Err(error) = self.unsubscribe_reference_prices() {
                log::error!(
                    "binary_oracle_edge_taker failed to unsubscribe reference_current_price retry subscriptions: {error}; strategy_id={}",
                    self.config.strategy_id,
                );
            }
            if let Err(error) = self.subscribe_reference_prices() {
                log::error!(
                    "binary_oracle_edge_taker failed to subscribe reference_current_price retry subscriptions: {error}; strategy_id={}",
                    self.config.strategy_id,
                );
            }
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
        let Some((interval_start_ms, interval_end_ms)) = self
            .active
            .interval_start_ms
            .zip(self.active.interval_end_ms)
        else {
            return false;
        };
        self.initialize_reference_price_runtime_state();
        let Some(reference_price_selector) = self.reference_price_selector.as_ref() else {
            return false;
        };
        let quotes = self
            .reference_price_quotes
            .values()
            .cloned()
            .collect::<Vec<_>>();

        !reference_price_selector
            .source_liveness_quorum_at(
                VenueEventMs::new(interval_start_ms),
                VenueEventMs::new(interval_end_ms),
                NtStrategyClockMs::new(now_ms),
                &quotes,
            )
            .is_satisfied()
    }

    pub(super) fn signal_instrument_id(&self) -> Option<InstrumentId> {
        self.config
            .signal_instrument_id
            .as_deref()
            .and_then(|instrument_id| InstrumentId::from_str(instrument_id).ok())
    }

    pub(super) fn ensure_startup_subscription_derivations(&self) -> Result<()> {
        let reference_requests = self.reference_price_subscription_requests()?;
        if self.reference_current_price_declares_subscription_sources()
            && reference_requests.is_empty()
        {
            return Err(anyhow::anyhow!(
                "binary_oracle_edge_taker reference_current_price declares subscription sources but derived zero subscription requests: strategy_id={}",
                self.config.strategy_id,
            ));
        }

        if self.config.signal_new_risk_available
            && self.signal_subscription_declares_source()
            && self.signal_instrument_id().is_none()
        {
            return Err(anyhow::anyhow!(
                "binary_oracle_edge_taker signal subscription derived zero requests from configured signal_instrument_id: strategy_id={}",
                self.config.strategy_id,
            ));
        }

        let surface_id = self.config.realized_volatility_surface_id.as_str();
        let quote_requests = self
            .context
            .realized_volatility_quote_subscription_requests_for_surface(surface_id);
        let trade_requests = self
            .context
            .realized_volatility_trade_subscription_requests_for_surface(surface_id);
        let index_requests = self
            .context
            .realized_volatility_index_subscription_requests_for_surface(surface_id);
        if quote_requests.is_empty()
            && trade_requests.is_empty()
            && index_requests.is_empty()
            && !self
                .context
                .realized_volatility_surface_subscriptions_blocked_only_by_provider_capability(
                    surface_id,
                )
        {
            return Err(anyhow::anyhow!(
                "binary_oracle_edge_taker realized_volatility surface `{surface_id}` derived zero subscription requests: strategy_id={}",
                self.config.strategy_id,
            ));
        }

        Ok(())
    }

    fn reference_current_price_declares_subscription_sources(&self) -> bool {
        self.config
            .reference_current_price
            .as_ref()
            .is_some_and(|reference_price| {
                !reference_price.source_order.is_empty() || !reference_price.sources.is_empty()
            })
    }

    fn signal_subscription_declares_source(&self) -> bool {
        self.config.signal_instrument_id.is_some()
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
        if !self.config.signal_new_risk_available {
            return;
        }
        if let Some(instrument_id) = self.signal_instrument_id() {
            let client_id = self.signal_client_id();
            self.subscribe_quotes(instrument_id, client_id, None);
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
            self.subscribe_quotes(instrument_id, client_id, None);
        }
        for (instrument_id, client_id) in trade_requests {
            self.subscribe_trades(instrument_id, client_id, None);
        }
        for (instrument_id, client_id) in index_requests {
            self.subscribe_index_prices(instrument_id, client_id, None);
        }
    }

    pub(super) fn unsubscribe_signal_quotes(&mut self) {
        if !self.config.signal_new_risk_available {
            return;
        }
        if let Some(instrument_id) = self.signal_instrument_id() {
            let client_id = self.signal_client_id();
            self.unsubscribe_quotes(instrument_id, client_id, None);
        }
    }

    pub(super) fn subscribe_reference_prices(&mut self) -> Result<()> {
        for subscription in self.reference_price_subscription_requests()? {
            self.subscribe_data(
                subscription.data_type.clone(),
                Some(subscription.client_id),
                Some(subscription.params.clone()),
            );
            self.record_reference_price_subscribe_event(ReferencePriceSubscribeEvent::subscribe(
                &subscription,
            ));
        }
        Ok(())
    }

    pub(super) fn unsubscribe_reference_prices(&mut self) -> Result<()> {
        for subscription in self.reference_price_subscription_requests()? {
            self.unsubscribe_data(
                subscription.data_type.clone(),
                Some(subscription.client_id),
                Some(subscription.params.clone()),
            );
            self.record_reference_price_subscribe_event(ReferencePriceSubscribeEvent::unsubscribe(
                &subscription,
            ));
        }
        Ok(())
    }

    pub(super) fn reference_price_subscription_requests(
        &self,
    ) -> Result<Vec<ReferencePriceSubscriptionRequest>> {
        let Some(reference_price) = &self.config.reference_current_price else {
            return Ok(Vec::new());
        };
        build_reference_price_subscription_requests(reference_price).map_err(|error| {
            anyhow::anyhow!(
                "binary_oracle_edge_taker reference_current_price subscription derivation failed: {error}; strategy_id={}",
                self.config.strategy_id,
            )
        })
    }

    pub(super) fn unsubscribe_realized_volatility_sources(&mut self) {
        let surface_id = self.config.realized_volatility_surface_id.clone();
        for (instrument_id, client_id) in self
            .context
            .realized_volatility_quote_subscription_requests_for_surface(&surface_id)
        {
            self.unsubscribe_quotes(instrument_id, client_id, None);
        }
        for (instrument_id, client_id) in self
            .context
            .realized_volatility_trade_subscription_requests_for_surface(&surface_id)
        {
            self.unsubscribe_trades(instrument_id, client_id, None);
        }
        for (instrument_id, client_id) in self
            .context
            .realized_volatility_index_subscription_requests_for_surface(&surface_id)
        {
            self.unsubscribe_index_prices(instrument_id, client_id, None);
        }
    }

    /// Fetches the live resolution strike for the current market interval.
    ///
    /// The first call keeps one durable index-price subscription open so NT
    /// will route the provider's [`IndexPriceUpdate`] into `on_index_price`.
    /// Later retry ticks use provider-owned custom fetch commands with unique
    /// data types, avoiding NT's per-instrument index subscribe dedup.
    pub(super) fn subscribe_resolution_strike(&mut self) {
        let Some(interval_start_ms) = self.active.interval_start_ms else {
            return;
        };
        let _ = self.subscribe_resolution_report_boundary(
            ResolutionStrikeReportBoundary::WindowOpen,
            STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM,
            interval_start_ms / MILLIS_PER_SECOND_U64,
        );
    }

    pub(super) fn subscribe_resolution_settlement_close(
        &mut self,
        interval_end_ms: u64,
    ) -> ResolutionReportSubscriptionOutcome {
        self.subscribe_resolution_report_boundary(
            ResolutionStrikeReportBoundary::WindowClose,
            SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM,
            interval_end_ms / MILLIS_PER_SECOND_U64,
        )
    }

    fn subscribe_resolution_report_boundary(
        &mut self,
        report_boundary: ResolutionStrikeReportBoundary,
        boundary_param: &'static str,
        boundary_unix_seconds: u64,
    ) -> ResolutionReportSubscriptionOutcome {
        let (Some(resolution_instrument_id), Some(resolution_client_id)) =
            (self.resolution_instrument_id(), self.resolution_client_id())
        else {
            return ResolutionReportSubscriptionOutcome::MissingRoute;
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
            return ResolutionReportSubscriptionOutcome::AssetBindingRejected;
        }
        let mut params = Params::new();
        params.insert(
            boundary_param.to_string(),
            serde_json::json!(boundary_unix_seconds),
        );
        let current_subscription_matches = self
            .resolution_report_boundary_subscriptions
            .get(&report_boundary)
            .is_some_and(|subscription| {
                subscription.instrument_id == resolution_instrument_id
                    && subscription.boundary_unix_seconds == boundary_unix_seconds
            });
        let use_durable_index_subscription = report_boundary.uses_durable_index_subscription();
        if !current_subscription_matches {
            let previous_subscription = self.resolution_report_boundary_subscriptions.insert(
                report_boundary,
                ResolutionReportBoundarySubscriptionState {
                    instrument_id: resolution_instrument_id,
                    boundary_unix_seconds,
                    durable_index_subscription: use_durable_index_subscription,
                    custom_subscription: None,
                },
            );
            let previous_custom_subscription = previous_subscription
                .as_ref()
                .and_then(|subscription| subscription.custom_subscription.clone());
            if let Some(data_type) = previous_custom_subscription {
                self.unsubscribe_data(data_type, Some(resolution_client_id), None);
            }

            let previous_index_subscription = previous_subscription.and_then(|subscription| {
                subscription
                    .durable_index_subscription
                    .then_some(subscription.instrument_id)
            });
            if let Some(instrument_id) = previous_index_subscription {
                self.unsubscribe_index_prices(instrument_id, Some(resolution_client_id), None);
            }

            if use_durable_index_subscription {
                self.subscribe_index_prices(
                    resolution_instrument_id,
                    Some(resolution_client_id),
                    Some(params.clone()),
                );
                self.record_resolution_strike_subscribe_event(
                    ResolutionStrikeSubscribeEvent::durable_index(
                        report_boundary,
                        resolution_instrument_id,
                        boundary_unix_seconds,
                    ),
                );
                return ResolutionReportSubscriptionOutcome::Dispatched;
            }
        }

        let mut params = Params::new();
        params.insert(
            boundary_param.to_string(),
            serde_json::json!(boundary_unix_seconds),
        );
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
            .resolution_report_boundary_subscriptions
            .get_mut(&report_boundary)
            .and_then(|subscription| subscription.custom_subscription.replace(data_type.clone()));
        if let Some(previous_data_type) = previous_custom_subscription {
            self.unsubscribe_data(previous_data_type, Some(resolution_client_id), None);
        }
        self.subscribe_data(data_type.clone(), Some(resolution_client_id), Some(params));
        self.record_resolution_strike_subscribe_event(
            ResolutionStrikeSubscribeEvent::custom_fetch(
                report_boundary,
                resolution_instrument_id,
                boundary_unix_seconds,
                self.resolution_strike_fetch_sequence,
                data_type,
            ),
        );
        ResolutionReportSubscriptionOutcome::Dispatched
    }

    pub(super) fn unsubscribe_resolution_strike(&mut self) {
        let Some(resolution_client_id) = self.resolution_client_id() else {
            self.resolution_report_boundary_subscriptions.clear();
            return;
        };
        let subscriptions = std::mem::take(&mut self.resolution_report_boundary_subscriptions);
        for subscription in subscriptions.into_values() {
            if let Some(data_type) = subscription.custom_subscription {
                self.unsubscribe_data(data_type, Some(resolution_client_id), None);
            }
            if subscription.durable_index_subscription {
                self.unsubscribe_index_prices(
                    subscription.instrument_id,
                    Some(resolution_client_id),
                    None,
                );
            }
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
    pub(super) report_boundary: ResolutionStrikeReportBoundary,
    pub(super) trigger: ResolutionStrikeFetchTrigger,
    pub(super) instrument_id: InstrumentId,
    pub(super) boundary_unix_seconds: u64,
    pub(super) request_sequence: Option<u64>,
    pub(super) custom_data_type: Option<DataType>,
}

const RESOLUTION_STRIKE_SUBSCRIBE_ACTION: &str = stringify!(subscribe);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolutionStrikeFetchTrigger {
    DurableIndex,
    CustomFetch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolutionReportSubscriptionOutcome {
    Dispatched,
    MissingRoute,
    AssetBindingRejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolutionReportBoundarySubscriptionState {
    instrument_id: InstrumentId,
    boundary_unix_seconds: u64,
    durable_index_subscription: bool,
    custom_subscription: Option<DataType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ResolutionStrikeReportBoundary {
    WindowOpen,
    WindowClose,
}

impl ResolutionStrikeReportBoundary {
    fn uses_durable_index_subscription(self) -> bool {
        matches!(self, Self::WindowOpen)
    }
}

impl ResolutionStrikeSubscribeEvent {
    fn durable_index(
        report_boundary: ResolutionStrikeReportBoundary,
        instrument_id: InstrumentId,
        boundary_unix_seconds: u64,
    ) -> Self {
        Self {
            action: RESOLUTION_STRIKE_SUBSCRIBE_ACTION,
            report_boundary,
            trigger: ResolutionStrikeFetchTrigger::DurableIndex,
            instrument_id,
            boundary_unix_seconds,
            request_sequence: None,
            custom_data_type: None,
        }
    }

    fn custom_fetch(
        report_boundary: ResolutionStrikeReportBoundary,
        instrument_id: InstrumentId,
        boundary_unix_seconds: u64,
        request_sequence: u64,
        custom_data_type: DataType,
    ) -> Self {
        Self {
            action: RESOLUTION_STRIKE_SUBSCRIBE_ACTION,
            report_boundary,
            trigger: ResolutionStrikeFetchTrigger::CustomFetch,
            instrument_id,
            boundary_unix_seconds,
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
