use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use nautilus_common::msgbus::{
    TypedHandler, subscribe_account_state, subscribe_order_events, subscribe_portfolio_snapshot,
    subscribe_position_events, unsubscribe_account_state, unsubscribe_order_events,
    unsubscribe_portfolio_snapshot, unsubscribe_position_events,
};
use nautilus_model::{
    enums::OrderSide,
    events::{AccountState, OrderEventAny, OrderFilled, PortfolioSnapshot, PositionEvent},
    identifiers::AccountId,
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_capital_admission::ProductAdmissionSnapshot,
    bolt_v3_capital_admission_state::{
        OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
        VenueSpendabilitySnapshot,
    },
    bolt_v3_observed_dedupe::prune_observed_dedupe_entries,
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderSide, BoltV3SubmitAdmissionState, BoltV3SubmitPositionSizingFillUpdate,
        BoltV3SubmitPositionSizingLifecycleDecision, BoltV3SubmitPositionSizingNtComponents,
    },
    nt_runtime_capture::{
        account_states_pattern, order_events_pattern, portfolio_snapshots_pattern,
        position_events_pattern,
    },
};

const CAPITAL_ADMISSION_ORDER_TERMINAL_SOURCE: &str = stringify!(nt_order_terminal_event);
const NT_ACCOUNT_STATE_PORTFOLIO_SOURCE: &str = stringify!(nt_account_state);
const NT_ACCOUNT_CACHE_PORTFOLIO_SOURCE: &str = "nt_account_cache";
const NT_ACCOUNT_FREE_COLLATERAL_SPENDABILITY_SOURCE: &str = "nt_account_free_collateral";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PositionFillTradeKey {
    instrument_id: String,
    trade_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionRuntimeFeedConfig {
    pub venue_id: String,
    pub account_id: AccountId,
    pub collateral_currency: String,
    pub product_state: ProductAdmissionSnapshot,
    pub startup_observed_at_ns: u64,
    pub dedupe_retention_ns: u64,
}

#[derive(Debug)]
pub struct CapitalAdmissionRuntimeFeed {
    config: CapitalAdmissionRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    component_builder: CapitalAdmissionRuntimeComponentBuilder,
    latest_terminal_observed_at_ns: Option<u64>,
    seen_known_position_fill_trade_ids: BTreeMap<PositionFillTradeKey, u64>,
    seen_external_position_fill_trade_ids: BTreeMap<PositionFillTradeKey, u64>,
    external_position_fill_trade_id_retention_exhausted: bool,
}

pub struct CapitalAdmissionRuntimeFeedSubscription {
    order_events: Option<TypedHandler<OrderEventAny>>,
    position_events: Option<TypedHandler<PositionEvent>>,
    account_states: Option<TypedHandler<AccountState>>,
    portfolio_snapshots: Option<TypedHandler<PortfolioSnapshot>>,
}

#[derive(Debug, Clone)]
struct CapitalAdmissionRuntimeComponentBuilder {
    latest_account_free_collateral: Option<(Decimal, u64)>,
    latest_portfolio: Option<PortfolioCapitalAdmissionSnapshot>,
    latest_venue_spendability: Option<VenueSpendabilitySnapshot>,
    live_order_attribution: BTreeMap<String, bool>,
    terminal_order_ids_seen: BTreeMap<String, u64>,
    order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot,
    product_state: ProductAdmissionSnapshot,
}

#[must_use]
pub fn subscribe_capital_admission_runtime_feed(
    feed: Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
) -> CapitalAdmissionRuntimeFeedSubscription {
    let order_feed = Arc::clone(&feed);
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        order_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_order_event(event);
    });
    subscribe_order_events(order_events_pattern(), order_events.clone(), None);
    let position_feed = Arc::clone(&feed);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        position_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_position_event(event);
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);
    let account_feed = Arc::clone(&feed);
    let account_states = TypedHandler::from(move |event: &AccountState| {
        account_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_account_state(event);
    });
    subscribe_account_state(account_states_pattern(), account_states.clone(), None);
    let portfolio_feed = Arc::clone(&feed);
    let portfolio_snapshots = TypedHandler::from(move |event: &PortfolioSnapshot| {
        portfolio_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_portfolio_snapshot(event);
    });
    subscribe_portfolio_snapshot(
        portfolio_snapshots_pattern(),
        portfolio_snapshots.clone(),
        None,
    );

    CapitalAdmissionRuntimeFeedSubscription {
        order_events: Some(order_events),
        position_events: Some(position_events),
        account_states: Some(account_states),
        portfolio_snapshots: Some(portfolio_snapshots),
    }
}

impl CapitalAdmissionRuntimeFeedSubscription {
    pub fn unsubscribe_all(&mut self) {
        if let Some(order_events) = self.order_events.take() {
            unsubscribe_order_events(order_events_pattern(), &order_events);
        }
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(position_events_pattern(), &position_events);
        }
        if let Some(account_states) = self.account_states.take() {
            unsubscribe_account_state(account_states_pattern(), &account_states);
        }
        if let Some(portfolio_snapshots) = self.portfolio_snapshots.take() {
            unsubscribe_portfolio_snapshot(portfolio_snapshots_pattern(), &portfolio_snapshots);
        }
    }
}

impl Drop for CapitalAdmissionRuntimeFeedSubscription {
    fn drop(&mut self) {
        self.unsubscribe_all();
    }
}

impl CapitalAdmissionRuntimeFeed {
    #[must_use]
    pub fn new(
        config: CapitalAdmissionRuntimeFeedConfig,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
    ) -> Self {
        let component_builder = CapitalAdmissionRuntimeComponentBuilder::new(&config);
        Self {
            config,
            submit_admission,
            component_builder,
            latest_terminal_observed_at_ns: None,
            seen_known_position_fill_trade_ids: BTreeMap::new(),
            seen_external_position_fill_trade_ids: BTreeMap::new(),
            external_position_fill_trade_id_retention_exhausted: false,
        }
    }

    pub fn on_account_state(
        &mut self,
        account_state: &AccountState,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        if account_state.account_id != self.config.account_id {
            return None;
        }
        let balance = account_state
            .balances
            .iter()
            .find(|balance| balance.currency.code.as_str() == self.config.collateral_currency)?;
        let free_collateral = balance.free.as_decimal();
        let total_equity = balance.total.as_decimal();
        self.component_builder.latest_account_free_collateral =
            Some((free_collateral, account_state.ts_event.as_u64()));
        self.component_builder.record_account_state_portfolio(
            &self.config,
            free_collateral,
            total_equity,
            account_state.ts_event.as_u64(),
        );
        self.component_builder.record_nt_account_spendability(
            &self.config,
            free_collateral,
            account_state.ts_event.as_u64(),
        );
        self.publish_components_if_ready()
    }

    pub fn on_portfolio_snapshot(
        &mut self,
        portfolio_snapshot: &PortfolioSnapshot,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        if portfolio_snapshot.account_id != self.config.account_id {
            return None;
        }
        let total_equity = portfolio_snapshot
            .total_equity
            .iter()
            .find(|money| money.currency.code.as_str() == self.config.collateral_currency)
            .map(|money| money.as_decimal())?;
        self.component_builder.latest_portfolio = Some(PortfolioCapitalAdmissionSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: portfolio_snapshot.ts_event.as_u64(),
            venue_id: self.config.venue_id.clone(),
            account_id: self.config.account_id.to_string(),
            collateral_currency: self.config.collateral_currency.clone(),
            free_collateral: Decimal::ZERO,
            total_equity,
        });
        self.publish_components_if_ready()
    }

    pub fn on_venue_spendability_snapshot(
        &mut self,
        snapshot: VenueSpendabilitySnapshot,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        self.component_builder
            .record_venue_spendability(&self.config, snapshot);
        self.publish_components_if_ready()
    }

    pub fn on_position_event(&mut self, _event: &PositionEvent) -> Option<()> {
        None
    }

    #[must_use]
    pub const fn configured_account_id(&self) -> AccountId {
        self.config.account_id
    }

    pub fn configured_collateral_currency(&self) -> String {
        self.config.collateral_currency.clone()
    }

    pub fn seed_open_order_cache<I>(
        &mut self,
        client_order_ids: I,
        observed_at_ns: u64,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents>
    where
        I: IntoIterator<Item = String>,
    {
        self.seed_cache_snapshot(
            client_order_ids,
            Decimal::ZERO,
            Decimal::ZERO,
            observed_at_ns,
        )
    }

    pub fn seed_cache_snapshot<I>(
        &mut self,
        client_order_ids: I,
        yes_position: Decimal,
        no_position: Decimal,
        observed_at_ns: u64,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents>
    where
        I: IntoIterator<Item = String>,
    {
        self.component_builder.seed_cache_snapshot(
            client_order_ids,
            yes_position,
            no_position,
            observed_at_ns,
            self.config.dedupe_retention_ns,
        );
        self.seen_known_position_fill_trade_ids.clear();
        self.seen_external_position_fill_trade_ids.clear();
        self.external_position_fill_trade_id_retention_exhausted = false;
        self.publish_components_if_ready()
    }

    pub fn seed_account_portfolio_snapshot(
        &mut self,
        free_collateral: Decimal,
        total_equity: Decimal,
        observed_at_ns: u64,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        self.component_builder.seed_account_portfolio_snapshot(
            &self.config,
            free_collateral,
            total_equity,
            observed_at_ns,
        );
        self.publish_components_if_ready()
    }

    #[must_use]
    pub fn configured_binary_instrument_ids(&self) -> Option<(String, String)> {
        match &self.config.product_state {
            ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => Some((
                snapshot.yes_instrument_id.clone(),
                snapshot.no_instrument_id.clone(),
            )),
        }
    }

    pub fn on_order_event(
        &mut self,
        event: &OrderEventAny,
    ) -> Option<BoltV3SubmitPositionSizingLifecycleDecision> {
        if let OrderEventAny::Filled(fill) = event {
            return self.on_fill_event(fill);
        }
        if is_live_order_event(event) {
            let account_id = event.account_id()?;
            if account_id != self.config.account_id {
                return None;
            }
            let client_order_id = event.client_order_id().to_string();
            let submit_owned = self
                .submit_admission
                .position_sizer_has_live_reservation(&client_order_id);
            self.component_builder.record_live_order_event(
                client_order_id,
                submit_owned,
                event.ts_event().as_u64(),
                self.config.dedupe_retention_ns,
            );
            self.publish_components_if_ready();
            return None;
        }
        if !is_terminal_order_event(event) {
            return None;
        }
        if event.account_id().is_none() && !matches!(event, OrderEventAny::Denied(_)) {
            return None;
        }
        if let Some(account_id) = event.account_id()
            && account_id != self.config.account_id
        {
            return None;
        }

        let observed_at_ns = event.ts_event().as_u64();
        self.component_builder.record_terminal_order_event(
            event.client_order_id().to_string(),
            observed_at_ns,
            self.config.dedupe_retention_ns,
        );
        self.publish_components_if_ready();
        let decision = self
            .submit_admission
            .apply_position_sizing_terminal_order_event(
                event.client_order_id().to_string(),
                observed_at_ns,
                CAPITAL_ADMISSION_ORDER_TERMINAL_SOURCE.to_string(),
            );
        if decision.unknown_reservation {
            return None;
        }
        self.latest_terminal_observed_at_ns = Some(observed_at_ns);
        Some(decision)
    }

    fn on_fill_event(
        &mut self,
        fill: &OrderFilled,
    ) -> Option<BoltV3SubmitPositionSizingLifecycleDecision> {
        if fill.account_id != self.config.account_id {
            return None;
        }
        let instrument_id = fill.instrument_id.to_string();
        let (yes_instrument_id, no_instrument_id) = self.configured_binary_instrument_ids()?;
        if instrument_id != yes_instrument_id && instrument_id != no_instrument_id {
            return None;
        }
        let side = match fill.order_side {
            OrderSide::Buy => BoltV3CompiledOrderSide::Buy,
            OrderSide::Sell => BoltV3CompiledOrderSide::Sell,
            _ => return None,
        };
        let observed_at_ns = fill.ts_event.as_u64();
        let client_order_id = fill.client_order_id.to_string();
        let trade_id = fill.trade_id.to_string();
        let submit_owned = self
            .submit_admission
            .position_sizer_has_live_reservation(&client_order_id);
        let fill_quantity = fill.last_qty.as_decimal();
        let fill_trade_key = PositionFillTradeKey {
            instrument_id: instrument_id.clone(),
            trade_id: trade_id.clone(),
        };
        let decision = self.submit_admission.apply_position_sizing_fill_update(
            BoltV3SubmitPositionSizingFillUpdate {
                client_order_id: client_order_id.clone(),
                trade_id: trade_id.clone(),
                instrument_id: instrument_id.clone(),
                side,
                fill_quantity,
                observed_at_ns,
                reconciliation: fill.reconciliation,
                evidence_label: "nt_order_fill".to_string(),
            },
            observed_at_ns,
        );
        let fill_changes_position = decision.accepted
            && matches!(
                decision.action,
                crate::bolt_v3_capital_admission::CapitalAdmissionLifecycleAction::Revalued
                    | crate::bolt_v3_capital_admission::CapitalAdmissionLifecycleAction::Released
            );
        if decision.accepted
            && !decision.unknown_reservation
            && !trade_id.trim().is_empty()
            && fill_quantity > Decimal::ZERO
        {
            self.record_seen_position_fill_trade_id(fill_trade_key.clone(), observed_at_ns);
        }
        let unknown_external_fill_changes_position = decision.unknown_reservation
            && !submit_owned
            && !fill.reconciliation
            && !trade_id.trim().is_empty()
            && fill_quantity > Decimal::ZERO
            && self
                .component_builder
                .product_observed_at_ns()
                .is_none_or(|product_observed_at_ns| observed_at_ns >= product_observed_at_ns)
            && self.record_new_position_fill_trade_id(fill_trade_key, observed_at_ns);
        if fill_changes_position || unknown_external_fill_changes_position {
            self.component_builder.record_fill_position_delta(
                &instrument_id,
                side,
                fill_quantity,
                observed_at_ns,
            );
        }
        if decision.action
            == crate::bolt_v3_capital_admission::CapitalAdmissionLifecycleAction::Released
        {
            self.component_builder.record_terminal_order_event(
                client_order_id,
                observed_at_ns,
                self.config.dedupe_retention_ns,
            );
            self.latest_terminal_observed_at_ns = Some(observed_at_ns);
        }
        if fill_changes_position || unknown_external_fill_changes_position {
            self.publish_components_if_ready();
        }
        if decision.unknown_reservation {
            return None;
        }
        Some(decision)
    }

    #[must_use]
    pub const fn latest_terminal_observed_at_ns(&self) -> Option<u64> {
        self.latest_terminal_observed_at_ns
    }

    fn record_seen_position_fill_trade_id(
        &mut self,
        key: PositionFillTradeKey,
        observed_at_ns: u64,
    ) {
        self.prune_seen_known_position_fill_trade_ids(observed_at_ns);
        self.seen_known_position_fill_trade_ids
            .insert(key, observed_at_ns);
    }

    fn record_new_position_fill_trade_id(
        &mut self,
        key: PositionFillTradeKey,
        observed_at_ns: u64,
    ) -> bool {
        self.prune_seen_external_position_fill_trade_ids(observed_at_ns);
        if self.external_position_fill_trade_id_retention_exhausted
            || self.known_position_fill_trade_id_blocks_external(&key, observed_at_ns)
            || self
                .seen_external_position_fill_trade_ids
                .contains_key(&key)
        {
            return false;
        }
        self.seen_external_position_fill_trade_ids
            .insert(key, observed_at_ns)
            .is_none()
    }

    fn known_position_fill_trade_id_blocks_external(
        &mut self,
        key: &PositionFillTradeKey,
        observed_at_ns: u64,
    ) -> bool {
        let Some(previous_observed_at_ns) = self.seen_known_position_fill_trade_ids.get_mut(key)
        else {
            return false;
        };
        if observed_at_ns.saturating_sub(*previous_observed_at_ns) > self.config.dedupe_retention_ns
        {
            *previous_observed_at_ns = observed_at_ns;
        }
        true
    }

    fn prune_seen_known_position_fill_trade_ids(&mut self, observed_at_ns: u64) {
        prune_observed_dedupe_entries(
            &mut self.seen_known_position_fill_trade_ids,
            observed_at_ns,
            self.config.dedupe_retention_ns,
        );
    }

    fn prune_seen_external_position_fill_trade_ids(&mut self, observed_at_ns: u64) {
        let len_before = self.seen_external_position_fill_trade_ids.len();
        prune_observed_dedupe_entries(
            &mut self.seen_external_position_fill_trade_ids,
            observed_at_ns,
            self.config.dedupe_retention_ns,
        );
        if self.seen_external_position_fill_trade_ids.len() < len_before {
            self.external_position_fill_trade_id_retention_exhausted = true;
        }
    }

    fn publish_components_if_ready(&mut self) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        let submit_admission = Arc::clone(&self.submit_admission);
        self.component_builder
            .refresh_live_order_attribution(|client_order_id| {
                submit_admission.position_sizer_has_live_reservation(client_order_id)
            });
        let components = self.component_builder.components(&self.config)?;
        self.submit_admission
            .update_position_sizing_nt_components(components.clone());
        Some(components)
    }
}

impl CapitalAdmissionRuntimeComponentBuilder {
    fn new(config: &CapitalAdmissionRuntimeFeedConfig) -> Self {
        Self {
            latest_account_free_collateral: None,
            latest_portfolio: None,
            latest_venue_spendability: None,
            live_order_attribution: BTreeMap::new(),
            terminal_order_ids_seen: BTreeMap::new(),
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_order_lifecycle_seed".to_string(),
                observed_at_ns: config.startup_observed_at_ns,
                open_order_count: 0,
                all_open_orders_attributed: false,
            },
            product_state: config.product_state.clone(),
        }
    }

    fn seed_cache_snapshot<I>(
        &mut self,
        client_order_ids: I,
        yes_position: Decimal,
        no_position: Decimal,
        observed_at_ns: u64,
        dedupe_retention_ns: u64,
    ) where
        I: IntoIterator<Item = String>,
    {
        let cache_open_ids = client_order_ids.into_iter().collect::<BTreeSet<_>>();
        self.prune_terminal_order_ids_seen(observed_at_ns, dedupe_retention_ns);
        let existing_live_order_ids = self
            .live_order_attribution
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let terminal_order_ids_seen = self
            .terminal_order_ids_seen
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let merged_live_order_ids = cache_open_ids
            .union(&existing_live_order_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        self.live_order_attribution = merged_live_order_ids
            .difference(&terminal_order_ids_seen)
            .map(|client_order_id| {
                let attributed = self
                    .live_order_attribution
                    .get(client_order_id)
                    .copied()
                    .unwrap_or(false);
                (client_order_id.clone(), attributed)
            })
            .collect();
        self.order_lifecycle = OrderLifecycleCapitalAdmissionSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns,
            open_order_count: self.live_order_attribution.len(),
            all_open_orders_attributed: self.all_live_orders_attributed(),
        };
        match &mut self.product_state {
            ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => {
                snapshot.source = "nt_position_cache".to_string();
                snapshot.observed_at_ns = observed_at_ns;
                snapshot.yes_position = yes_position;
                snapshot.no_position = no_position;
            }
        }
    }

    fn seed_account_portfolio_snapshot(
        &mut self,
        config: &CapitalAdmissionRuntimeFeedConfig,
        free_collateral: Decimal,
        total_equity: Decimal,
        observed_at_ns: u64,
    ) {
        self.latest_account_free_collateral = Some((free_collateral, observed_at_ns));
        self.latest_portfolio = Some(PortfolioCapitalAdmissionSnapshot {
            source: NT_ACCOUNT_CACHE_PORTFOLIO_SOURCE.to_string(),
            observed_at_ns,
            venue_id: config.venue_id.clone(),
            account_id: config.account_id.to_string(),
            collateral_currency: config.collateral_currency.clone(),
            free_collateral,
            total_equity,
        });
        self.record_nt_account_spendability(config, free_collateral, observed_at_ns);
    }

    fn record_account_state_portfolio(
        &mut self,
        config: &CapitalAdmissionRuntimeFeedConfig,
        free_collateral: Decimal,
        total_equity: Decimal,
        observed_at_ns: u64,
    ) {
        match self.latest_portfolio.as_mut() {
            Some(current)
                if current.source != NT_ACCOUNT_STATE_PORTFOLIO_SOURCE
                    && current.source != NT_ACCOUNT_CACHE_PORTFOLIO_SOURCE => {}
            Some(current) => {
                current.observed_at_ns = observed_at_ns;
                current.free_collateral = free_collateral;
                current.total_equity = total_equity;
            }
            None => {
                self.latest_portfolio = Some(PortfolioCapitalAdmissionSnapshot {
                    source: NT_ACCOUNT_STATE_PORTFOLIO_SOURCE.to_string(),
                    observed_at_ns,
                    venue_id: config.venue_id.clone(),
                    account_id: config.account_id.to_string(),
                    collateral_currency: config.collateral_currency.clone(),
                    free_collateral,
                    total_equity,
                });
            }
        }
    }

    fn record_venue_spendability(
        &mut self,
        config: &CapitalAdmissionRuntimeFeedConfig,
        snapshot: VenueSpendabilitySnapshot,
    ) {
        let matches_config = snapshot.venue_id == config.venue_id
            && snapshot.account_id == config.account_id.to_string()
            && snapshot.collateral_currency == config.collateral_currency;
        if !matches_config {
            return;
        }
        if self
            .latest_venue_spendability
            .as_ref()
            .is_some_and(|current| current.observed_at_ns > snapshot.observed_at_ns)
        {
            return;
        }
        self.latest_venue_spendability = Some(snapshot);
    }

    fn record_nt_account_spendability(
        &mut self,
        config: &CapitalAdmissionRuntimeFeedConfig,
        free_collateral: Decimal,
        observed_at_ns: u64,
    ) {
        match self.latest_venue_spendability.as_mut() {
            Some(current) if current.source != NT_ACCOUNT_FREE_COLLATERAL_SPENDABILITY_SOURCE => {}
            Some(current) => {
                current.observed_at_ns = observed_at_ns;
                current.spendable_collateral = free_collateral;
                current.collateral_allowance = free_collateral;
            }
            None => {
                self.latest_venue_spendability = Some(VenueSpendabilitySnapshot {
                    source: NT_ACCOUNT_FREE_COLLATERAL_SPENDABILITY_SOURCE.to_string(),
                    observed_at_ns,
                    venue_id: config.venue_id.clone(),
                    account_id: config.account_id.to_string(),
                    collateral_currency: config.collateral_currency.clone(),
                    spendable_collateral: free_collateral,
                    collateral_allowance: free_collateral,
                });
            }
        }
    }

    fn record_live_order_event(
        &mut self,
        client_order_id: String,
        attributed: bool,
        observed_at_ns: u64,
        dedupe_retention_ns: u64,
    ) {
        self.prune_terminal_order_ids_seen(observed_at_ns, dedupe_retention_ns);
        if !self.terminal_order_ids_seen.contains_key(&client_order_id) {
            self.live_order_attribution
                .entry(client_order_id)
                .and_modify(|existing| *existing = *existing || attributed)
                .or_insert(attributed);
        }
        self.refresh_order_lifecycle_from_event(observed_at_ns);
    }

    fn refresh_live_order_attribution<F>(&mut self, mut has_live_reservation: F)
    where
        F: FnMut(&str) -> bool,
    {
        let mut changed = false;
        for (client_order_id, attributed) in &mut self.live_order_attribution {
            if !*attributed && has_live_reservation(client_order_id) {
                *attributed = true;
                changed = true;
            }
        }
        if changed {
            self.order_lifecycle.open_order_count = self.live_order_attribution.len();
            self.order_lifecycle.all_open_orders_attributed = self.all_live_orders_attributed();
        }
    }

    fn record_terminal_order_event(
        &mut self,
        client_order_id: String,
        observed_at_ns: u64,
        dedupe_retention_ns: u64,
    ) {
        self.prune_terminal_order_ids_seen(observed_at_ns, dedupe_retention_ns);
        self.terminal_order_ids_seen
            .insert(client_order_id.clone(), observed_at_ns);
        self.live_order_attribution.remove(&client_order_id);
        self.refresh_order_lifecycle_from_event(observed_at_ns);
    }

    fn prune_terminal_order_ids_seen(&mut self, now_ns: u64, dedupe_retention_ns: u64) {
        prune_observed_dedupe_entries(
            &mut self.terminal_order_ids_seen,
            now_ns,
            dedupe_retention_ns,
        );
    }

    fn refresh_order_lifecycle_from_event(&mut self, observed_at_ns: u64) {
        self.order_lifecycle = OrderLifecycleCapitalAdmissionSnapshot {
            source: "nt_order_event".to_string(),
            observed_at_ns,
            open_order_count: self.live_order_attribution.len(),
            all_open_orders_attributed: self.all_live_orders_attributed(),
        };
    }

    fn all_live_orders_attributed(&self) -> bool {
        self.live_order_attribution
            .values()
            .all(|attributed| *attributed)
    }

    fn record_fill_position_delta(
        &mut self,
        instrument_id: &str,
        side: BoltV3CompiledOrderSide,
        fill_quantity: Decimal,
        observed_at_ns: u64,
    ) {
        let ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) = &mut self.product_state;
        let outcome_position = if instrument_id == snapshot.yes_instrument_id {
            &mut snapshot.yes_position
        } else if instrument_id == snapshot.no_instrument_id {
            &mut snapshot.no_position
        } else {
            return;
        };
        match side {
            BoltV3CompiledOrderSide::Buy => {
                *outcome_position += fill_quantity;
                snapshot.conditional_token_allowance += fill_quantity;
            }
            BoltV3CompiledOrderSide::Sell => {
                *outcome_position = outcome_position
                    .checked_sub(fill_quantity)
                    .filter(|position| *position > Decimal::ZERO)
                    .unwrap_or(Decimal::ZERO);
                snapshot.conditional_token_allowance = snapshot
                    .conditional_token_allowance
                    .checked_sub(fill_quantity)
                    .filter(|allowance| *allowance > Decimal::ZERO)
                    .unwrap_or(Decimal::ZERO);
            }
        }
        snapshot.source = "nt_order_fill".to_string();
        snapshot.observed_at_ns = observed_at_ns;
    }

    fn product_observed_at_ns(&self) -> Option<u64> {
        match &self.product_state {
            ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => {
                Some(snapshot.observed_at_ns)
            }
        }
    }

    fn components(
        &self,
        _config: &CapitalAdmissionRuntimeFeedConfig,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        let (free_collateral, account_observed_at_ns) = self.latest_account_free_collateral?;
        let mut portfolio = self.latest_portfolio.clone()?;
        let venue_spendability = self.latest_venue_spendability.clone()?;
        portfolio.free_collateral = free_collateral;
        let mut product_state = self.product_state.clone();
        let product_observed_at_ns = match &mut product_state {
            ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => {
                // NT free collateral, venue spendability, and transfer allowance are independent constraints.
                snapshot.collateral_allowance = free_collateral
                    .min(venue_spendability.spendable_collateral)
                    .min(venue_spendability.collateral_allowance);
                snapshot.observed_at_ns = snapshot
                    .observed_at_ns
                    .max(account_observed_at_ns)
                    .max(venue_spendability.observed_at_ns);
                snapshot.observed_at_ns
            }
        };
        let observed_at_ns = account_observed_at_ns
            .max(portfolio.observed_at_ns)
            .max(venue_spendability.observed_at_ns)
            .max(self.order_lifecycle.observed_at_ns)
            .max(product_observed_at_ns);
        Some(BoltV3SubmitPositionSizingNtComponents {
            source: "nt_position_sizer_runtime_components".to_string(),
            observed_at_ns,
            portfolio,
            venue_spendability,
            order_lifecycle: self.order_lifecycle.clone(),
            product_state,
            loss_snapshot: None,
        })
    }
}

fn is_terminal_order_event(event: &OrderEventAny) -> bool {
    matches!(
        event,
        OrderEventAny::Denied(_)
            | OrderEventAny::Rejected(_)
            | OrderEventAny::Canceled(_)
            | OrderEventAny::Expired(_)
    )
}

fn is_live_order_event(event: &OrderEventAny) -> bool {
    matches!(
        event,
        OrderEventAny::Submitted(_) | OrderEventAny::Accepted(_)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::PositionFillTradeKey;
    use crate::bolt_v3_observed_dedupe::prune_observed_dedupe_entries;

    #[test]
    fn observed_dedupe_pruning_removes_only_entries_older_than_retention() {
        let mut entries = BTreeMap::from([
            (
                PositionFillTradeKey {
                    instrument_id: "instrument-old".to_string(),
                    trade_id: "trade-old".to_string(),
                },
                100,
            ),
            (
                PositionFillTradeKey {
                    instrument_id: "instrument-recent".to_string(),
                    trade_id: "trade-recent".to_string(),
                },
                175,
            ),
            (
                PositionFillTradeKey {
                    instrument_id: "instrument-future".to_string(),
                    trade_id: "trade-future".to_string(),
                },
                250,
            ),
        ]);

        prune_observed_dedupe_entries(&mut entries, 200, 50);

        assert_eq!(entries.len(), 2);
        assert!(entries.keys().any(|key| key.trade_id == "trade-recent"));
        assert!(entries.keys().any(|key| key.trade_id == "trade-future"));
        assert!(!entries.keys().any(|key| key.trade_id == "trade-old"));
    }
}
