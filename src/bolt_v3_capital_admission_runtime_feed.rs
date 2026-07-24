use std::{
    collections::BTreeSet,
    rc::Rc,
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
        ProviderCollateralAllowanceSnapshot,
    },
    bolt_v3_current_evidence::SubmitReservationFillSource,
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderSide, BoltV3SubmitAdmissionState,
        BoltV3SubmitCapitalAdmissionFillUpdate, BoltV3SubmitCapitalAdmissionNtComponents,
        BoltV3SubmitReservationFillEvidenceDecision,
    },
    nt_runtime_capture::{
        account_states_pattern, order_events_pattern, portfolio_snapshots_pattern,
        position_events_pattern,
    },
};

const NT_ACCOUNT_CACHE_PORTFOLIO_SOURCE: &str = "nt_account_cache";
pub use crate::bolt_v3_capital_admission_state::POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionRuntimeFeedConfig {
    pub venue_id: String,
    pub account_id: AccountId,
    pub collateral_currency: String,
    pub product_state: ProductAdmissionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionNtCacheProjection {
    pub accepted_allowance_observed_at_ns: Option<u64>,
    pub account_balances: Option<(Decimal, Decimal)>,
    pub open_client_order_ids: Vec<String>,
    pub yes_position: Decimal,
    pub no_position: Decimal,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapitalAdmissionProjectionError {
    AllowanceGenerationMismatch {
        accepted: Option<u64>,
        projected: Option<u64>,
    },
    MissingNtAccountBalances,
    MissingProviderCollateralAllowance,
    DuplicateNtClientOrderId,
}

#[derive(Debug)]
pub struct CapitalAdmissionRuntimeFeed {
    config: CapitalAdmissionRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    latest_provider_collateral_allowance: Option<ProviderCollateralAllowanceSnapshot>,
    accepted_allowance_observed_at_ns: Option<u64>,
}

pub struct SubmitAdmissionNtProjectionSubscription {
    order_events: Option<TypedHandler<OrderEventAny>>,
    position_events: Option<TypedHandler<PositionEvent>>,
    account_states: Option<TypedHandler<AccountState>>,
    portfolio_snapshots: Option<TypedHandler<PortfolioSnapshot>>,
}

pub type CapitalAdmissionNtProjectionTrigger = Rc<dyn Fn()>;

#[must_use]
pub fn subscribe_submit_admission_nt_projection(
    feed: Option<Arc<Mutex<CapitalAdmissionRuntimeFeed>>>,
    nt_projection_trigger: CapitalAdmissionNtProjectionTrigger,
) -> SubmitAdmissionNtProjectionSubscription {
    let order_feed = feed.clone();
    let order_projection_trigger = Rc::clone(&nt_projection_trigger);
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        if let Some(order_feed) = order_feed.as_ref() {
            order_feed
                .lock()
                .expect("capital admission runtime order-event feed lock poisoned")
                .on_order_event(event);
        }
        order_projection_trigger();
    });
    subscribe_order_events(order_events_pattern(), order_events.clone(), None);
    let position_events = feed.as_ref().map(|_| {
        let projection_trigger = Rc::clone(&nt_projection_trigger);
        let handler = TypedHandler::from(move |_event: &PositionEvent| {
            projection_trigger();
        });
        subscribe_position_events(position_events_pattern(), handler.clone(), None);
        handler
    });
    let account_states = feed.as_ref().map(|_| {
        let projection_trigger = Rc::clone(&nt_projection_trigger);
        let handler = TypedHandler::from(move |_event: &AccountState| {
            projection_trigger();
        });
        subscribe_account_state(account_states_pattern(), handler.clone(), None);
        handler
    });
    let portfolio_snapshots = feed.as_ref().map(|_| {
        let handler = TypedHandler::from(move |_event: &PortfolioSnapshot| {
            nt_projection_trigger();
        });
        subscribe_portfolio_snapshot(portfolio_snapshots_pattern(), handler.clone(), None);
        handler
    });

    SubmitAdmissionNtProjectionSubscription {
        order_events: Some(order_events),
        position_events,
        account_states,
        portfolio_snapshots,
    }
}

impl SubmitAdmissionNtProjectionSubscription {
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

impl Drop for SubmitAdmissionNtProjectionSubscription {
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
        Self {
            config,
            submit_admission,
            latest_provider_collateral_allowance: None,
            accepted_allowance_observed_at_ns: None,
        }
    }

    pub fn on_provider_collateral_allowance_snapshot(
        &mut self,
        snapshot: ProviderCollateralAllowanceSnapshot,
    ) {
        let observed_at_ns = snapshot.observed_at_ns;
        if self.accept_provider_collateral_allowance(snapshot) {
            self.submit_admission
                .invalidate_capital_admission_for_new_allowance_snapshot(observed_at_ns);
        }
    }

    #[must_use]
    pub const fn configured_account_id(&self) -> AccountId {
        self.config.account_id
    }

    pub fn configured_collateral_currency(&self) -> String {
        self.config.collateral_currency.clone()
    }

    #[must_use]
    pub const fn accepted_allowance_observed_at_ns(&self) -> Option<u64> {
        self.accepted_allowance_observed_at_ns
    }

    pub fn canonical_nt_components(
        &self,
        projection: CapitalAdmissionNtCacheProjection,
    ) -> Result<BoltV3SubmitCapitalAdmissionNtComponents, CapitalAdmissionProjectionError> {
        if self.accepted_allowance_observed_at_ns != projection.accepted_allowance_observed_at_ns {
            return Err(
                CapitalAdmissionProjectionError::AllowanceGenerationMismatch {
                    accepted: self.accepted_allowance_observed_at_ns,
                    projected: projection.accepted_allowance_observed_at_ns,
                },
            );
        }
        let (free_collateral, total_equity) = projection
            .account_balances
            .ok_or(CapitalAdmissionProjectionError::MissingNtAccountBalances)?;
        let provider_collateral_allowance = self
            .latest_provider_collateral_allowance
            .clone()
            .ok_or(CapitalAdmissionProjectionError::MissingProviderCollateralAllowance)?;
        let open_client_order_ids = projection
            .open_client_order_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if open_client_order_ids.len() != projection.open_client_order_ids.len() {
            return Err(CapitalAdmissionProjectionError::DuplicateNtClientOrderId);
        }

        let portfolio = PortfolioCapitalAdmissionSnapshot {
            source: NT_ACCOUNT_CACHE_PORTFOLIO_SOURCE.to_string(),
            observed_at_ns: projection.observed_at_ns,
            venue_id: self.config.venue_id.clone(),
            account_id: self.config.account_id.to_string(),
            collateral_currency: self.config.collateral_currency.clone(),
            free_collateral,
            total_equity,
        };
        let order_lifecycle = OrderLifecycleCapitalAdmissionSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns: projection.observed_at_ns,
            open_order_count: open_client_order_ids.len(),
            all_open_orders_attributed: open_client_order_ids.is_empty(),
        };
        let mut product_state = self.config.product_state.clone();
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &mut product_state;
        product.source = "nt_position_cache".to_string();
        product.observed_at_ns = projection.observed_at_ns;
        product.yes_position = projection.yes_position;
        product.no_position = projection.no_position;
        product.collateral_allowance = provider_collateral_allowance.collateral_allowance;
        let observed_at_ns = projection
            .observed_at_ns
            .max(provider_collateral_allowance.observed_at_ns);

        Ok(BoltV3SubmitCapitalAdmissionNtComponents {
            source: "nt_capital_admission_runtime_components".to_string(),
            observed_at_ns,
            portfolio,
            provider_collateral_allowance,
            order_lifecycle,
            product_state,
            loss_snapshot: None,
        })
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
    ) -> Option<BoltV3SubmitReservationFillEvidenceDecision> {
        if let OrderEventAny::Filled(fill) = event {
            return self.on_fill_event(fill);
        }
        None
    }

    fn on_fill_event(
        &mut self,
        fill: &OrderFilled,
    ) -> Option<BoltV3SubmitReservationFillEvidenceDecision> {
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
            _ => {
                self.submit_admission
                    .reject_capital_admission_fill_evidence(fill.ts_event.as_u64());
                return Some(BoltV3SubmitReservationFillEvidenceDecision {
                    accepted: false,
                    unknown_reservation: true,
                });
            }
        };
        let observed_at_ns = fill.ts_event.as_u64();
        let client_order_id = fill.client_order_id.to_string();
        let trade_id = fill.trade_id.to_string();
        let fill_quantity = fill.last_qty.as_decimal();
        let decision = self
            .submit_admission
            .record_capital_admission_fill_evidence(BoltV3SubmitCapitalAdmissionFillUpdate {
                client_order_id: client_order_id.clone(),
                trade_id: trade_id.clone(),
                instrument_id: instrument_id.clone(),
                side,
                fill_quantity,
                observed_at_ns,
                reconciliation: fill.reconciliation,
                evidence_source: SubmitReservationFillSource::NtOrderFill,
            });
        Some(decision)
    }

    fn accept_provider_collateral_allowance(
        &mut self,
        snapshot: ProviderCollateralAllowanceSnapshot,
    ) -> bool {
        let matches_config = snapshot.venue_id == self.config.venue_id
            && snapshot.account_id == self.config.account_id.to_string()
            && snapshot.collateral_currency == self.config.collateral_currency;
        if !matches_config {
            return false;
        }
        if self
            .latest_provider_collateral_allowance
            .as_ref()
            .is_some_and(|current| current.observed_at_ns >= snapshot.observed_at_ns)
        {
            return false;
        }
        self.accepted_allowance_observed_at_ns = Some(snapshot.observed_at_ns);
        self.latest_provider_collateral_allowance = Some(snapshot);
        true
    }
}
