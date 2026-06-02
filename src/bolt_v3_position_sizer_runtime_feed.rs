use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::Arc,
};

use nautilus_common::msgbus::{
    TypedHandler, subscribe_account_state, subscribe_order_events, subscribe_portfolio_snapshot,
    subscribe_position_events, unsubscribe_account_state, unsubscribe_order_events,
    unsubscribe_portfolio_snapshot, unsubscribe_position_events,
};
use nautilus_model::{
    enums::{OrderSide, PositionSide},
    events::{AccountState, OrderEventAny, OrderFilled, PortfolioSnapshot, PositionEvent},
    identifiers::AccountId,
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_position_sizer::ProductSizingSnapshot,
    bolt_v3_sizing_state::{OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot},
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderSide, BoltV3SubmitAdmissionState, BoltV3SubmitPositionSizingFillUpdate,
        BoltV3SubmitPositionSizingLifecycleDecision, BoltV3SubmitPositionSizingNtComponents,
    },
    nt_runtime_capture::{
        account_states_pattern, order_events_pattern, portfolio_snapshots_pattern,
        position_events_pattern,
    },
};

const POSITION_SIZER_ORDER_TERMINAL_SOURCE: &str = stringify!(nt_order_terminal_event);
const POSITION_SIZER_POSITION_EVENT_SOURCE: &str = stringify!(nt_position_event);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSizerRuntimeFeedConfig {
    pub venue_id: String,
    pub account_id: AccountId,
    pub collateral_currency: String,
    pub product_state: ProductSizingSnapshot,
    pub startup_observed_at_ns: u64,
    pub max_snapshot_age_ns: u64,
}

#[derive(Debug)]
pub struct PositionSizerRuntimeFeed {
    config: PositionSizerRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    component_builder: PositionSizerRuntimeComponentBuilder,
    latest_terminal_observed_at_ns: Option<u64>,
    fill_position_updates_seen: BTreeMap<(String, String, String), u64>,
}

pub struct PositionSizerRuntimeFeedSubscription {
    order_events: Option<TypedHandler<OrderEventAny>>,
    position_events: Option<TypedHandler<PositionEvent>>,
    account_states: Option<TypedHandler<AccountState>>,
    portfolio_snapshots: Option<TypedHandler<PortfolioSnapshot>>,
}

#[derive(Debug, Clone)]
struct PositionSizerRuntimeComponentBuilder {
    latest_account_free_collateral: Option<(Decimal, u64)>,
    latest_portfolio: Option<PortfolioSizingSnapshot>,
    live_order_attribution: BTreeMap<String, bool>,
    terminal_order_ids_seen: BTreeMap<String, u64>,
    order_lifecycle: OrderLifecycleSizingSnapshot,
    product_state: ProductSizingSnapshot,
    max_snapshot_age_ns: u64,
}

#[must_use]
pub fn subscribe_position_sizer_runtime_feed(
    feed: Rc<RefCell<PositionSizerRuntimeFeed>>,
) -> PositionSizerRuntimeFeedSubscription {
    let order_feed = Rc::clone(&feed);
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        order_feed.borrow_mut().on_order_event(event);
    });
    subscribe_order_events(order_events_pattern(), order_events.clone(), None);
    let position_feed = Rc::clone(&feed);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        position_feed.borrow_mut().on_position_event(event);
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);
    let account_feed = Rc::clone(&feed);
    let account_states = TypedHandler::from(move |event: &AccountState| {
        account_feed.borrow_mut().on_account_state(event);
    });
    subscribe_account_state(account_states_pattern(), account_states.clone(), None);
    let portfolio_feed = Rc::clone(&feed);
    let portfolio_snapshots = TypedHandler::from(move |event: &PortfolioSnapshot| {
        portfolio_feed.borrow_mut().on_portfolio_snapshot(event);
    });
    subscribe_portfolio_snapshot(
        portfolio_snapshots_pattern(),
        portfolio_snapshots.clone(),
        None,
    );

    PositionSizerRuntimeFeedSubscription {
        order_events: Some(order_events),
        position_events: Some(position_events),
        account_states: Some(account_states),
        portfolio_snapshots: Some(portfolio_snapshots),
    }
}

impl PositionSizerRuntimeFeedSubscription {
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

impl Drop for PositionSizerRuntimeFeedSubscription {
    fn drop(&mut self) {
        self.unsubscribe_all();
    }
}

impl PositionSizerRuntimeFeed {
    #[must_use]
    pub fn new(
        config: PositionSizerRuntimeFeedConfig,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
    ) -> Self {
        let component_builder = PositionSizerRuntimeComponentBuilder::new(&config);
        Self {
            config,
            submit_admission,
            component_builder,
            latest_terminal_observed_at_ns: None,
            fill_position_updates_seen: BTreeMap::new(),
        }
    }

    pub fn on_account_state(
        &mut self,
        account_state: &AccountState,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        if account_state.account_id != self.config.account_id {
            return None;
        }
        let free_collateral = account_state
            .balances
            .iter()
            .find(|balance| balance.currency.code.as_str() == self.config.collateral_currency)
            .map(|balance| balance.free.as_decimal())?;
        self.component_builder.latest_account_free_collateral =
            Some((free_collateral, account_state.ts_event.as_u64()));
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
        self.component_builder.latest_portfolio = Some(PortfolioSizingSnapshot {
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

    pub fn on_position_event(
        &mut self,
        event: &PositionEvent,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        if event.account_id() != self.config.account_id {
            return None;
        }
        let (quantity, observed_at_ns) = position_event_inventory(event)?;
        let instrument_id = event.instrument_id().to_string();
        if !self
            .component_builder
            .record_position_event(&instrument_id, quantity, observed_at_ns)
        {
            return None;
        }
        self.publish_components_if_ready()
    }

    #[must_use]
    pub const fn configured_account_id(&self) -> AccountId {
        self.config.account_id
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
        );
        self.publish_components_if_ready()
    }

    #[must_use]
    pub fn configured_binary_instrument_ids(&self) -> Option<(String, String)> {
        match &self.config.product_state {
            ProductSizingSnapshot::PredictionMarketBinary(snapshot) => Some((
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
        self.component_builder
            .record_terminal_order_event(event.client_order_id().to_string(), observed_at_ns);
        self.publish_components_if_ready();
        let decision = self
            .submit_admission
            .apply_position_sizing_terminal_order_event(
                event.client_order_id().to_string(),
                observed_at_ns,
                POSITION_SIZER_ORDER_TERMINAL_SOURCE.to_string(),
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
        let decision = self.submit_admission.apply_position_sizing_fill_update(
            BoltV3SubmitPositionSizingFillUpdate {
                client_order_id: fill.client_order_id.to_string(),
                trade_id: fill.trade_id.to_string(),
                instrument_id: instrument_id.clone(),
                side,
                fill_quantity: fill.last_qty.as_decimal(),
                observed_at_ns,
                reconciliation: fill.reconciliation,
                evidence_label: "nt_order_fill".to_string(),
            },
            observed_at_ns,
        );
        if decision.unknown_reservation {
            if !fill.reconciliation {
                self.record_fill_position_delta_once(
                    fill.client_order_id.to_string(),
                    fill.trade_id.to_string(),
                    &instrument_id,
                    side,
                    fill.last_qty.as_decimal(),
                    observed_at_ns,
                );
                self.publish_components_if_ready();
            }
            return None;
        }
        let fill_changes_position = decision.accepted
            && matches!(
                decision.action,
                crate::bolt_v3_position_sizer::PositionSizingLifecycleAction::Revalued
                    | crate::bolt_v3_position_sizer::PositionSizingLifecycleAction::Released
            );
        if fill_changes_position {
            self.record_fill_position_delta_once(
                fill.client_order_id.to_string(),
                fill.trade_id.to_string(),
                &instrument_id,
                side,
                fill.last_qty.as_decimal(),
                observed_at_ns,
            );
        }
        if decision.action == crate::bolt_v3_position_sizer::PositionSizingLifecycleAction::Released
        {
            self.component_builder
                .record_terminal_order_event(fill.client_order_id.to_string(), observed_at_ns);
            self.latest_terminal_observed_at_ns = Some(observed_at_ns);
        }
        if fill_changes_position {
            self.publish_components_if_ready();
        }
        Some(decision)
    }

    fn record_fill_position_delta_once(
        &mut self,
        client_order_id: String,
        trade_id: String,
        instrument_id: &str,
        side: BoltV3CompiledOrderSide,
        fill_quantity: Decimal,
        observed_at_ns: u64,
    ) -> bool {
        if trade_id.trim().is_empty() || fill_quantity <= Decimal::ZERO {
            return false;
        }
        self.prune_fill_position_updates_seen(observed_at_ns);
        let key = (client_order_id, trade_id, instrument_id.to_string());
        if self.fill_position_updates_seen.contains_key(&key) {
            return false;
        }
        self.fill_position_updates_seen.insert(key, observed_at_ns);
        self.component_builder.record_fill_position_delta(
            instrument_id,
            side,
            fill_quantity,
            observed_at_ns,
        );
        true
    }

    fn prune_fill_position_updates_seen(&mut self, observed_at_ns: u64) {
        let max_snapshot_age_ns = self.config.max_snapshot_age_ns;
        self.fill_position_updates_seen
            .retain(|_, fill_observed_at_ns| {
                observed_at_ns.saturating_sub(*fill_observed_at_ns) <= max_snapshot_age_ns
            });
    }

    #[must_use]
    pub const fn latest_terminal_observed_at_ns(&self) -> Option<u64> {
        self.latest_terminal_observed_at_ns
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

impl PositionSizerRuntimeComponentBuilder {
    fn new(config: &PositionSizerRuntimeFeedConfig) -> Self {
        Self {
            latest_account_free_collateral: None,
            latest_portfolio: None,
            live_order_attribution: BTreeMap::new(),
            terminal_order_ids_seen: BTreeMap::new(),
            order_lifecycle: OrderLifecycleSizingSnapshot {
                source: "nt_order_lifecycle_seed".to_string(),
                observed_at_ns: config.startup_observed_at_ns,
                open_order_count: 0,
                all_open_orders_attributed: false,
            },
            product_state: config.product_state.clone(),
            max_snapshot_age_ns: config.max_snapshot_age_ns,
        }
    }

    fn seed_cache_snapshot<I>(
        &mut self,
        client_order_ids: I,
        yes_position: Decimal,
        no_position: Decimal,
        observed_at_ns: u64,
    ) where
        I: IntoIterator<Item = String>,
    {
        self.prune_terminal_order_ids(observed_at_ns);
        let cache_open_ids = client_order_ids.into_iter().collect::<BTreeSet<_>>();
        let existing_live_order_ids = self
            .live_order_attribution
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let merged_live_order_ids = cache_open_ids
            .union(&existing_live_order_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        self.live_order_attribution = merged_live_order_ids
            .iter()
            .filter(|client_order_id| !self.terminal_order_ids_seen.contains_key(*client_order_id))
            .map(|client_order_id| {
                let attributed = self
                    .live_order_attribution
                    .get(client_order_id)
                    .copied()
                    .unwrap_or(false);
                (client_order_id.clone(), attributed)
            })
            .collect();
        self.order_lifecycle = OrderLifecycleSizingSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns,
            open_order_count: self.live_order_attribution.len(),
            all_open_orders_attributed: self.all_live_orders_attributed(),
        };
        match &mut self.product_state {
            ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
                snapshot.source = "nt_position_cache".to_string();
                snapshot.observed_at_ns = observed_at_ns;
                snapshot.yes_position = yes_position;
                snapshot.no_position = no_position;
                snapshot.conditional_token_allowance = yes_position + no_position;
            }
        }
    }

    fn record_live_order_event(
        &mut self,
        client_order_id: String,
        attributed: bool,
        observed_at_ns: u64,
    ) {
        self.prune_terminal_order_ids(observed_at_ns);
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

    fn record_terminal_order_event(&mut self, client_order_id: String, observed_at_ns: u64) {
        self.terminal_order_ids_seen
            .insert(client_order_id.clone(), observed_at_ns);
        self.prune_terminal_order_ids(observed_at_ns);
        self.live_order_attribution.remove(&client_order_id);
        self.refresh_order_lifecycle_from_event(observed_at_ns);
    }

    fn prune_terminal_order_ids(&mut self, observed_at_ns: u64) {
        let max_snapshot_age_ns = self.max_snapshot_age_ns;
        self.terminal_order_ids_seen
            .retain(|_, terminal_observed_at_ns| {
                observed_at_ns.saturating_sub(*terminal_observed_at_ns) <= max_snapshot_age_ns
            });
    }

    fn refresh_order_lifecycle_from_event(&mut self, observed_at_ns: u64) {
        self.order_lifecycle = OrderLifecycleSizingSnapshot {
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
        let ProductSizingSnapshot::PredictionMarketBinary(snapshot) = &mut self.product_state;
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

    fn record_position_event(
        &mut self,
        instrument_id: &str,
        position_quantity: Decimal,
        observed_at_ns: u64,
    ) -> bool {
        let ProductSizingSnapshot::PredictionMarketBinary(snapshot) = &mut self.product_state;
        let outcome_position = if instrument_id == snapshot.yes_instrument_id {
            &mut snapshot.yes_position
        } else if instrument_id == snapshot.no_instrument_id {
            &mut snapshot.no_position
        } else {
            return false;
        };
        *outcome_position = position_quantity;
        snapshot.conditional_token_allowance = snapshot.yes_position + snapshot.no_position;
        snapshot.source = POSITION_SIZER_POSITION_EVENT_SOURCE.to_string();
        snapshot.observed_at_ns = observed_at_ns;
        true
    }

    fn components(
        &self,
        _config: &PositionSizerRuntimeFeedConfig,
    ) -> Option<BoltV3SubmitPositionSizingNtComponents> {
        let (free_collateral, account_observed_at_ns) = self.latest_account_free_collateral?;
        let mut portfolio = self.latest_portfolio.clone()?;
        portfolio.free_collateral = free_collateral;
        let mut product_state = self.product_state.clone();
        let product_observed_at_ns = match &mut product_state {
            ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
                snapshot.collateral_allowance = free_collateral;
                snapshot.observed_at_ns
            }
        };
        let observed_at_ns = account_observed_at_ns
            .max(portfolio.observed_at_ns)
            .max(self.order_lifecycle.observed_at_ns)
            .max(product_observed_at_ns);
        Some(BoltV3SubmitPositionSizingNtComponents {
            source: "nt_position_sizer_runtime_components".to_string(),
            observed_at_ns,
            portfolio,
            order_lifecycle: self.order_lifecycle.clone(),
            product_state,
            loss_snapshot: None,
        })
    }
}

fn position_event_inventory(event: &PositionEvent) -> Option<(Decimal, u64)> {
    match event {
        PositionEvent::PositionOpened(position) => Some((
            long_inventory_quantity(position.side, position.quantity.as_decimal()),
            position.ts_event.as_u64(),
        )),
        PositionEvent::PositionChanged(position) => Some((
            long_inventory_quantity(position.side, position.quantity.as_decimal()),
            position.ts_event.as_u64(),
        )),
        PositionEvent::PositionClosed(position) => {
            Some((Decimal::ZERO, position.ts_event.as_u64()))
        }
        PositionEvent::PositionAdjusted(_) => None,
    }
}

fn long_inventory_quantity(side: PositionSide, quantity: Decimal) -> Decimal {
    match side {
        PositionSide::Long => quantity,
        PositionSide::Flat | PositionSide::Short | PositionSide::NoPositionSide => Decimal::ZERO,
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
    use super::*;
    use crate::bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3PositionSizerRebuildAuditEvidence, BoltV3StrategyInputEvidenceSnapshot,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
    };
    use crate::bolt_v3_position_sizer::PredictionMarketSizingSnapshot;

    #[derive(Debug)]
    struct NoopDecisionEvidenceWriter;

    impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> anyhow::Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &BoltV3AdmissionDecisionEvidence,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn record_position_sizer_rebuild_audit(
            &self,
            _audit: &BoltV3PositionSizerRebuildAuditEvidence,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &BoltV3SubmitReservationMetadataEvidence,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &BoltV3SubmitReservationFillEvidence,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn test_runtime_config(max_snapshot_age_ns: u64) -> PositionSizerRuntimeFeedConfig {
        PositionSizerRuntimeFeedConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: AccountId::from("ACCOUNT-001"),
            collateral_currency: "USD".to_string(),
            product_state: ProductSizingSnapshot::PredictionMarketBinary(
                PredictionMarketSizingSnapshot {
                    source: "test_product_state".to_string(),
                    observed_at_ns: 0,
                    yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                    no_instrument_id: "instrument-no.VENUE-A".to_string(),
                    yes_position: Decimal::ZERO,
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::ZERO,
                    conditional_token_allowance: Decimal::ZERO,
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            startup_observed_at_ns: 0,
            max_snapshot_age_ns,
        }
    }

    #[test]
    fn fill_position_dedup_keys_prune_by_snapshot_age() {
        let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
            NoopDecisionEvidenceWriter,
        )));
        let mut feed = PositionSizerRuntimeFeed::new(test_runtime_config(100), admission);

        assert!(feed.record_fill_position_delta_once(
            "client-order-1".to_string(),
            "trade-1".to_string(),
            "instrument-yes.VENUE-A",
            BoltV3CompiledOrderSide::Buy,
            Decimal::ONE,
            100,
        ));
        assert_eq!(feed.fill_position_updates_seen.len(), 1);
        assert!(!feed.record_fill_position_delta_once(
            "client-order-1".to_string(),
            "trade-1".to_string(),
            "instrument-yes.VENUE-A",
            BoltV3CompiledOrderSide::Buy,
            Decimal::ONE,
            120,
        ));
        assert_eq!(feed.fill_position_updates_seen.len(), 1);

        assert!(feed.record_fill_position_delta_once(
            "client-order-2".to_string(),
            "trade-2".to_string(),
            "instrument-yes.VENUE-A",
            BoltV3CompiledOrderSide::Buy,
            Decimal::ONE,
            250,
        ));
        assert_eq!(feed.fill_position_updates_seen.len(), 1);
    }
}
