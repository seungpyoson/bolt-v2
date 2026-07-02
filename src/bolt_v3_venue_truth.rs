use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
};

use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::OrderSide,
    events::OrderEventAny,
    identifiers::{AccountId, VenueOrderId},
    types::Money,
};
use rust_decimal::Decimal;

pub type VenueTruthSnapshotFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<VenueTruthSnapshot>> + Send + 'a>>;

pub trait VenueTruthSnapshotSource: std::fmt::Debug + Send + Sync {
    fn snapshot(&self, captured_at: UnixNanos) -> VenueTruthSnapshotFuture<'_>;
}

pub trait VenueTruthOrderEventMapper: std::fmt::Debug + Send + Sync {
    fn map_order_event(&self, event: &OrderEventAny) -> Option<VenueTruthOrderEvent>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthSnapshot {
    pub captured_at: UnixNanos,
    pub account_id: AccountId,
    pub collateral_balance: Money,
    pub collateral_allowance: Money,
    pub open_orders: BTreeMap<VenueOrderId, VenueTruthOpenOrder>,
    pub positions_by_product_id: BTreeMap<String, Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthOpenOrder {
    pub venue_order_id: VenueOrderId,
    pub market_id: String,
    pub product_id: String,
    pub side: OrderSide,
    pub original_size: Decimal,
    pub size_matched: Decimal,
    pub open_size: Decimal,
    pub price: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueTruthOrderEvent {
    Accepted {
        client_order_id: String,
        venue_order_id: VenueOrderId,
    },
    Filled {
        venue_order_id: VenueOrderId,
        product_id: String,
        side: OrderSide,
        quantity: Decimal,
    },
    Terminal {
        client_order_id: String,
        venue_order_id: Option<VenueOrderId>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthReconciliation {
    BaselineAccepted,
    DeltaExplained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthDivergenceKind {
    AccountChanged,
    UnexplainedOpenOrderDelta,
    UnexplainedPositionDelta,
    UnexplainedCollateralDelta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthDivergence {
    pub kind: VenueTruthDivergenceKind,
    pub previous_captured_at: Option<UnixNanos>,
    pub current_captured_at: UnixNanos,
}

#[derive(Debug, Clone)]
pub struct VenueTruthReconciler {
    previous_snapshot: Option<VenueTruthSnapshot>,
    event_projection: VenueTruthEventProjection,
}

#[derive(Debug, Clone)]
struct VenueTruthEventProjection {
    accepted_venue_order_ids: BTreeSet<VenueOrderId>,
    client_to_venue_order_id: BTreeMap<String, VenueOrderId>,
    terminal_venue_order_ids: BTreeSet<VenueOrderId>,
    fill_quantity_by_venue_order_id: BTreeMap<VenueOrderId, Decimal>,
    buy_fill_quantity_by_product_id: BTreeMap<String, Decimal>,
    sell_fill_quantity_by_product_id: BTreeMap<String, Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VenueTruthOpenOrderDelta {
    collateral_balance_delta: Decimal,
}

// The bolt-v3 legacy-default fence forbids a `Default` impl on the production
// surface, so the no-argument `new` is sanctioned with an explicit allow rather
// than satisfying `clippy::new_without_default` by adding a forbidden `Default`.
#[allow(clippy::new_without_default)]
impl VenueTruthReconciler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            previous_snapshot: None,
            event_projection: VenueTruthEventProjection::empty(),
        }
    }

    pub fn record_order_event(&mut self, event: VenueTruthOrderEvent) {
        self.event_projection.record_order_event(event);
    }

    pub fn reconcile_snapshot(
        &mut self,
        snapshot: VenueTruthSnapshot,
    ) -> Result<VenueTruthReconciliation, VenueTruthDivergence> {
        let Some(previous) = &self.previous_snapshot else {
            self.previous_snapshot = Some(snapshot);
            return Ok(VenueTruthReconciliation::BaselineAccepted);
        };
        if previous.account_id != snapshot.account_id {
            return Err(divergence(
                VenueTruthDivergenceKind::AccountChanged,
                Some(previous.captured_at),
                snapshot.captured_at,
            ));
        }

        let mut projection = self.event_projection.clone();
        let open_order_delta = match explain_open_order_delta(previous, &snapshot, &mut projection)
        {
            Ok(delta) => delta,
            Err(kind) => {
                return Err(divergence(
                    kind,
                    Some(previous.captured_at),
                    snapshot.captured_at,
                ));
            }
        };
        if let Err(kind) = explain_position_delta(previous, &snapshot, &mut projection) {
            return Err(divergence(
                kind,
                Some(previous.captured_at),
                snapshot.captured_at,
            ));
        }
        if let Err(kind) = explain_collateral_delta(
            previous,
            &snapshot,
            open_order_delta.collateral_balance_delta,
        ) {
            return Err(divergence(
                kind,
                Some(previous.captured_at),
                snapshot.captured_at,
            ));
        }

        self.event_projection = projection;
        self.previous_snapshot = Some(snapshot);
        Ok(VenueTruthReconciliation::DeltaExplained)
    }
}

impl VenueTruthEventProjection {
    fn empty() -> Self {
        Self {
            accepted_venue_order_ids: BTreeSet::new(),
            client_to_venue_order_id: BTreeMap::new(),
            terminal_venue_order_ids: BTreeSet::new(),
            fill_quantity_by_venue_order_id: BTreeMap::new(),
            buy_fill_quantity_by_product_id: BTreeMap::new(),
            sell_fill_quantity_by_product_id: BTreeMap::new(),
        }
    }

    fn record_order_event(&mut self, event: VenueTruthOrderEvent) {
        match event {
            VenueTruthOrderEvent::Accepted {
                client_order_id,
                venue_order_id,
            } => {
                self.accepted_venue_order_ids.insert(venue_order_id);
                self.client_to_venue_order_id
                    .insert(client_order_id, venue_order_id);
            }
            VenueTruthOrderEvent::Filled {
                venue_order_id,
                product_id,
                side,
                quantity,
            } => {
                add_decimal(
                    &mut self.fill_quantity_by_venue_order_id,
                    venue_order_id,
                    quantity,
                );
                match side {
                    OrderSide::Buy => add_decimal(
                        &mut self.buy_fill_quantity_by_product_id,
                        product_id,
                        quantity,
                    ),
                    OrderSide::Sell => add_decimal(
                        &mut self.sell_fill_quantity_by_product_id,
                        product_id,
                        quantity,
                    ),
                    _ => {}
                }
            }
            VenueTruthOrderEvent::Terminal {
                client_order_id,
                venue_order_id,
            } => {
                if let Some(venue_order_id) = venue_order_id
                    .or_else(|| self.client_to_venue_order_id.get(&client_order_id).copied())
                {
                    self.terminal_venue_order_ids.insert(venue_order_id);
                }
            }
        }
    }
}

fn explain_open_order_delta(
    previous: &VenueTruthSnapshot,
    current: &VenueTruthSnapshot,
    projection: &mut VenueTruthEventProjection,
) -> Result<VenueTruthOpenOrderDelta, VenueTruthDivergenceKind> {
    let mut collateral_balance_delta = Decimal::ZERO;
    for (venue_order_id, current_order) in &current.open_orders {
        match previous.open_orders.get(venue_order_id) {
            None => {
                if !projection.accepted_venue_order_ids.remove(venue_order_id) {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
                if current_order.size_matched > Decimal::ZERO
                    && !consume_decimal(
                        &mut projection.fill_quantity_by_venue_order_id,
                        venue_order_id,
                        current_order.size_matched,
                    )
                {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
                if current_order.size_matched > Decimal::ZERO {
                    collateral_balance_delta += collateral_balance_delta_for_fill(
                        current_order,
                        current_order.size_matched,
                    )?;
                }
            }
            Some(previous_order) => {
                if previous_order.market_id != current_order.market_id
                    || previous_order.product_id != current_order.product_id
                    || previous_order.side != current_order.side
                    || previous_order.original_size != current_order.original_size
                    || previous_order.price != current_order.price
                    || current_order.size_matched < previous_order.size_matched
                    || current_order.open_size > previous_order.open_size
                {
                    return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                }
                let matched_delta = current_order.size_matched - previous_order.size_matched;
                if matched_delta > Decimal::ZERO {
                    if !consume_decimal(
                        &mut projection.fill_quantity_by_venue_order_id,
                        venue_order_id,
                        matched_delta,
                    ) {
                        return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
                    }
                    collateral_balance_delta +=
                        collateral_balance_delta_for_fill(current_order, matched_delta)?;
                }
            }
        }
    }
    for (venue_order_id, previous_order) in &previous.open_orders {
        if current.open_orders.contains_key(venue_order_id) {
            continue;
        }
        if projection.terminal_venue_order_ids.remove(venue_order_id) {
            continue;
        }
        if consume_decimal(
            &mut projection.fill_quantity_by_venue_order_id,
            venue_order_id,
            previous_order.open_size,
        ) {
            collateral_balance_delta +=
                collateral_balance_delta_for_fill(previous_order, previous_order.open_size)?;
            continue;
        }
        return Err(VenueTruthDivergenceKind::UnexplainedOpenOrderDelta);
    }
    Ok(VenueTruthOpenOrderDelta {
        collateral_balance_delta,
    })
}

fn explain_position_delta(
    previous: &VenueTruthSnapshot,
    current: &VenueTruthSnapshot,
    projection: &mut VenueTruthEventProjection,
) -> Result<bool, VenueTruthDivergenceKind> {
    let mut product_ids: BTreeSet<&str> = BTreeSet::new();
    product_ids.extend(previous.positions_by_product_id.keys().map(String::as_str));
    product_ids.extend(current.positions_by_product_id.keys().map(String::as_str));

    let mut explained = false;
    for product_id in product_ids {
        let previous_size = previous
            .positions_by_product_id
            .get(product_id)
            .copied()
            .unwrap_or(Decimal::ZERO);
        let current_size = current
            .positions_by_product_id
            .get(product_id)
            .copied()
            .unwrap_or(Decimal::ZERO);
        let delta = current_size - previous_size;
        if delta > Decimal::ZERO {
            if !consume_decimal(
                &mut projection.buy_fill_quantity_by_product_id,
                product_id,
                delta,
            ) {
                return Err(VenueTruthDivergenceKind::UnexplainedPositionDelta);
            }
            explained = true;
        } else if delta < Decimal::ZERO {
            if !consume_decimal(
                &mut projection.sell_fill_quantity_by_product_id,
                product_id,
                -delta,
            ) {
                return Err(VenueTruthDivergenceKind::UnexplainedPositionDelta);
            }
            explained = true;
        }
    }
    Ok(explained)
}

fn explain_collateral_delta(
    previous: &VenueTruthSnapshot,
    current: &VenueTruthSnapshot,
    expected_collateral_balance_delta: Decimal,
) -> Result<(), VenueTruthDivergenceKind> {
    if previous.collateral_allowance != current.collateral_allowance {
        return Err(VenueTruthDivergenceKind::UnexplainedCollateralDelta);
    }
    let collateral_balance_delta =
        current.collateral_balance.as_decimal() - previous.collateral_balance.as_decimal();
    if collateral_balance_delta != expected_collateral_balance_delta {
        return Err(VenueTruthDivergenceKind::UnexplainedCollateralDelta);
    }
    Ok(())
}

fn collateral_balance_delta_for_fill(
    order: &VenueTruthOpenOrder,
    quantity: Decimal,
) -> Result<Decimal, VenueTruthDivergenceKind> {
    match order.side {
        OrderSide::Buy => Ok(-(quantity * order.price)),
        OrderSide::Sell => Ok(quantity * order.price),
        _ => Err(VenueTruthDivergenceKind::UnexplainedCollateralDelta),
    }
}

fn divergence(
    kind: VenueTruthDivergenceKind,
    previous_captured_at: Option<UnixNanos>,
    current_captured_at: UnixNanos,
) -> VenueTruthDivergence {
    VenueTruthDivergence {
        kind,
        previous_captured_at,
        current_captured_at,
    }
}

fn add_decimal<K>(map: &mut BTreeMap<K, Decimal>, key: K, amount: Decimal)
where
    K: Ord,
{
    map.entry(key)
        .and_modify(|current| *current += amount)
        .or_insert(amount);
}

fn consume_decimal<K, Q>(map: &mut BTreeMap<K, Decimal>, key: &Q, amount: Decimal) -> bool
where
    K: Ord + std::borrow::Borrow<Q>,
    Q: Ord + ?Sized,
{
    if amount <= Decimal::ZERO {
        return true;
    }
    let mut remove = false;
    if let Some(current) = map.get_mut(key) {
        if *current < amount {
            return false;
        }
        *current -= amount;
        if *current == Decimal::ZERO {
            remove = true;
        }
    } else {
        return false;
    }
    if remove {
        map.remove(key);
    }
    true
}
