use std::collections::BTreeMap;

use nautilus_model::{
    data::{BookOrder, OrderBookDepth10},
    enums::OrderSide,
    identifiers::InstrumentId,
    orderbook::{BookLevel, OrderBook},
    types::Price,
};
use rust_decimal::{
    Decimal,
    prelude::{FromPrimitive, ToPrimitive},
};

use crate::{
    bolt_v3_executable_cost::{
        ExecutableBookQuote, ExecutableCostBlockReason, executable_cost_breakdown,
        price_exact_size_vwap,
    },
    bolt_v3_numeric::{
        BPS_DENOMINATOR, CENTS_PER_SHARE, NANOS_PER_MILLI_U64, ZERO_F64, is_non_negative_finite,
        is_positive_finite,
    },
    bolt_v3_outcome_group_sources::outcome_group_observation_is_fresh,
    bolt_v3_outcome_groups::{
        GroupingProof, OutcomeGroup, OutcomeGroupValidationError, PayoutMatrix, TerminalStateKind,
        ValidatedOutcomeGroup,
    },
};

const DECIMAL_F64_ROUND_DP: u32 = 12;

#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeGroupDepthSnapshot {
    pub instrument_id: InstrumentId,
    pub observed_unix_ms: Option<u64>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    bid_levels: BTreeMap<Price, f64>,
    ask_levels: BTreeMap<Price, f64>,
}

impl OutcomeGroupDepthSnapshot {
    pub fn from_depth10(depth: &OrderBookDepth10) -> Result<Self, OutcomeGroupDepthAdapterError> {
        let mut bid_levels = BTreeMap::new();
        let mut ask_levels = BTreeMap::new();
        for bid in depth.bids {
            insert_book_order_level(&mut bid_levels, bid);
        }
        for ask in depth.asks {
            insert_book_order_level(&mut ask_levels, ask);
        }
        Self::from_maps(
            depth.instrument_id,
            Some(depth.ts_event.as_u64() / NANOS_PER_MILLI_U64),
            bid_levels,
            ask_levels,
        )
    }

    pub fn from_order_book(book: &OrderBook) -> Result<Self, OutcomeGroupDepthAdapterError> {
        let mut bid_levels = BTreeMap::new();
        let mut ask_levels = BTreeMap::new();
        for level in book.bids(None) {
            insert_book_level(&mut bid_levels, level);
        }
        for level in book.asks(None) {
            insert_book_level(&mut ask_levels, level);
        }
        Self::from_maps(
            book.instrument_id,
            Some(book.ts_last.as_u64() / NANOS_PER_MILLI_U64),
            bid_levels,
            ask_levels,
        )
    }

    pub fn from_book_levels(
        instrument_id: InstrumentId,
        observed_unix_ms: Option<u64>,
        bid_levels: Vec<BookLevel>,
        ask_levels: Vec<BookLevel>,
    ) -> Result<Self, OutcomeGroupDepthAdapterError> {
        let mut bid_map = BTreeMap::new();
        let mut ask_map = BTreeMap::new();
        for level in &bid_levels {
            insert_book_level(&mut bid_map, level);
        }
        for level in &ask_levels {
            insert_book_level(&mut ask_map, level);
        }
        Self::from_maps(instrument_id, observed_unix_ms, bid_map, ask_map)
    }

    fn from_maps(
        instrument_id: InstrumentId,
        observed_unix_ms: Option<u64>,
        bid_levels: BTreeMap<Price, f64>,
        ask_levels: BTreeMap<Price, f64>,
    ) -> Result<Self, OutcomeGroupDepthAdapterError> {
        let best_bid = bid_levels.last_key_value().map(|(price, _)| price.as_f64());
        let best_ask = ask_levels
            .first_key_value()
            .map(|(price, _)| price.as_f64());
        if best_bid.is_none() && best_ask.is_none() {
            return Err(OutcomeGroupDepthAdapterError::EmptyBook);
        }
        Ok(Self {
            instrument_id,
            observed_unix_ms,
            best_bid,
            best_ask,
            bid_levels,
            ask_levels,
        })
    }

    fn executable_quote(&self) -> ExecutableBookQuote<'_> {
        ExecutableBookQuote {
            best_bid: self.best_bid,
            best_ask: self.best_ask,
            bid_levels: &self.bid_levels,
            ask_levels: &self.ask_levels,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeGroupDepthAdapterError {
    EmptyBook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeGroupCandidateLeg {
    pub leg_id: String,
    pub order_side: OrderSide,
    pub target_notional: Decimal,
}

pub struct OutcomeGroupScanInput<'a> {
    pub group: &'a OutcomeGroup,
    pub candidate_legs: Vec<OutcomeGroupCandidateLeg>,
    pub books: BTreeMap<InstrumentId, OutcomeGroupDepthSnapshot>,
    pub now_unix_ms: u64,
    pub max_book_age_ms: u64,
    pub min_edge_bps: Decimal,
    pub vwap_depth_limit_bps: u64,
    pub slippage_buffer_bps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeGroupScanBlockReason {
    MissingBook,
    MissingBookTimestamp,
    StaleBook,
    InsufficientDepth,
    InvalidCost,
    InvalidPriceScale,
    MinQuantity,
    MinNotional,
    QuantityStep,
    NonPositiveEdge,
    EdgeThreshold,
    UnknownLeg,
    IncompleteCandidate,
    UnsupportedOrderSide,
}

impl OutcomeGroupScanBlockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingBook => "missing_book",
            Self::MissingBookTimestamp => "missing_book_timestamp",
            Self::StaleBook => "stale_book",
            Self::InsufficientDepth => "insufficient_depth",
            Self::InvalidCost => "invalid_cost",
            Self::InvalidPriceScale => "invalid_price_scale",
            Self::MinQuantity => "min_quantity",
            Self::MinNotional => "min_notional",
            Self::QuantityStep => "quantity_step",
            Self::NonPositiveEdge => "non_positive_edge",
            Self::EdgeThreshold => "edge_threshold",
            Self::UnknownLeg => "unknown_leg",
            Self::IncompleteCandidate => "incomplete_candidate",
            Self::UnsupportedOrderSide => "unsupported_order_side",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeGroupLegScanEvidence {
    pub leg_id: String,
    pub instrument_id: InstrumentId,
    pub order_side: OrderSide,
    pub target_notional: Decimal,
    pub executable_quantity: Decimal,
    pub gross_cost: Decimal,
    pub slippage_buffer: Decimal,
    pub total_adjusted_cost: Decimal,
    pub vwap_price: Decimal,
    pub limit_price: Decimal,
    pub observed_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeGroupScanEvidence {
    pub group_id: String,
    pub grouping_proof: Option<GroupingProof>,
    pub leg_costs: Vec<OutcomeGroupLegScanEvidence>,
    pub state_payouts: BTreeMap<String, Decimal>,
    pub guaranteed_payout: Decimal,
    pub total_gross_cost: Decimal,
    pub total_slippage_buffer: Decimal,
    pub total_adjusted_cost: Decimal,
    pub absolute_edge: Decimal,
    pub edge_bps: Decimal,
    pub min_depth_quantity: Decimal,
    pub admissible: bool,
    pub block_reason: Option<OutcomeGroupScanBlockReason>,
}

pub fn scan_outcome_group_candidate(input: OutcomeGroupScanInput<'_>) -> OutcomeGroupScanEvidence {
    match scan_outcome_group_candidate_inner(&input) {
        Ok(mut evidence) => {
            evidence.admissible = true;
            evidence
        }
        Err((reason, partial)) => partial.blocked(reason),
    }
}

#[allow(clippy::result_large_err)]
fn scan_outcome_group_candidate_inner(
    input: &OutcomeGroupScanInput<'_>,
) -> Result<OutcomeGroupScanEvidence, (OutcomeGroupScanBlockReason, OutcomeGroupScanEvidence)> {
    let mut evidence = OutcomeGroupScanEvidence::empty(input.group);
    if let Err(error) = ValidatedOutcomeGroup::validate(input.group) {
        let reason = validation_block_reason(error);
        return Err((reason, evidence));
    }
    if input.candidate_legs.is_empty() {
        return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence));
    }

    let mut quantities_by_leg = BTreeMap::<String, Decimal>::new();
    for candidate in &input.candidate_legs {
        if candidate.order_side != OrderSide::Buy {
            return Err((OutcomeGroupScanBlockReason::UnsupportedOrderSide, evidence));
        }
        if candidate.target_notional <= Decimal::ZERO {
            return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence));
        }
        let leg = match input.group.tradable_legs.get(&candidate.leg_id) {
            Some(leg) => leg,
            None => return Err((OutcomeGroupScanBlockReason::UnknownLeg, evidence)),
        };
        let book = match input.books.get(&leg.instrument_id) {
            Some(book) => book,
            None => return Err((OutcomeGroupScanBlockReason::MissingBook, evidence)),
        };
        let observed_unix_ms = match book.observed_unix_ms {
            Some(value) => value,
            None => {
                return Err((OutcomeGroupScanBlockReason::MissingBookTimestamp, evidence));
            }
        };
        if !outcome_group_observation_is_fresh(
            input.now_unix_ms,
            observed_unix_ms,
            input.max_book_age_ms,
            None,
        ) {
            return Err((OutcomeGroupScanBlockReason::StaleBook, evidence));
        }
        let priced_leg = match price_candidate_leg(input, candidate, book) {
            Ok(priced_leg) => priced_leg,
            Err(reason) => return Err((reason, evidence)),
        };
        if let Err(reason) = validate_leg_constraints(
            priced_leg.executable_quantity,
            priced_leg.gross_cost,
            &leg.order_constraints,
        ) {
            return Err((reason, evidence));
        }
        if quantities_by_leg
            .insert(candidate.leg_id.clone(), priced_leg.executable_quantity)
            .is_some()
        {
            return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence));
        }
        evidence.total_gross_cost =
            match evidence.total_gross_cost.checked_add(priced_leg.gross_cost) {
                Some(value) => value,
                None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
            };
        evidence.total_slippage_buffer = match evidence
            .total_slippage_buffer
            .checked_add(priced_leg.slippage_buffer)
        {
            Some(value) => value,
            None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
        };
        evidence.total_adjusted_cost = match evidence
            .total_adjusted_cost
            .checked_add(priced_leg.total_adjusted_cost)
        {
            Some(value) => value,
            None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
        };
        evidence.min_depth_quantity =
            match positive_min(evidence.min_depth_quantity, priced_leg.executable_quantity) {
                Some(value) => value,
                None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
            };
        evidence.leg_costs.push(priced_leg);
    }
    if !candidate_covers_standard_outcomes(input.group, &quantities_by_leg) {
        return Err((OutcomeGroupScanBlockReason::IncompleteCandidate, evidence));
    }

    evidence.state_payouts =
        match evaluate_state_payouts(&input.group.payout_matrix, &quantities_by_leg) {
            Ok(payouts) => payouts,
            Err(reason) => return Err((reason, evidence)),
        };
    evidence.guaranteed_payout = match minimum_payout(&evidence.state_payouts) {
        Some(value) => value,
        None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
    };
    if evidence.total_adjusted_cost <= Decimal::ZERO {
        return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence));
    }
    evidence.absolute_edge = match evidence
        .guaranteed_payout
        .checked_sub(evidence.total_adjusted_cost)
    {
        Some(value) => value,
        None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
    };
    if evidence.absolute_edge <= Decimal::ZERO {
        return Err((OutcomeGroupScanBlockReason::NonPositiveEdge, evidence));
    }
    evidence.edge_bps = match evidence
        .absolute_edge
        .checked_mul(Decimal::from(BPS_DENOMINATOR as u64))
        .and_then(|value| value.checked_div(evidence.total_adjusted_cost))
    {
        Some(value) => value,
        None => return Err((OutcomeGroupScanBlockReason::InvalidCost, evidence)),
    };
    if evidence.edge_bps < input.min_edge_bps {
        return Err((OutcomeGroupScanBlockReason::EdgeThreshold, evidence));
    }

    Ok(evidence)
}

fn price_candidate_leg(
    input: &OutcomeGroupScanInput<'_>,
    candidate: &OutcomeGroupCandidateLeg,
    book: &OutcomeGroupDepthSnapshot,
) -> Result<OutcomeGroupLegScanEvidence, OutcomeGroupScanBlockReason> {
    let target_notional = decimal_to_positive_f64(candidate.target_notional)?;
    let quote = book.executable_quote();
    let vwap = price_exact_size_vwap(
        &quote,
        candidate.order_side,
        target_notional,
        input.vwap_depth_limit_bps,
    )
    .map_err(scan_reason_from_cost)?;
    let leg = input
        .group
        .tradable_legs
        .get(&candidate.leg_id)
        .ok_or(OutcomeGroupScanBlockReason::UnknownLeg)?;
    let cost_breakdown = executable_cost_breakdown(vwap, input.slippage_buffer_bps)
        .map_err(scan_reason_from_cost)?;
    let quantity = decimal_from_f64(vwap.vwap_quantity)?;
    let gross_cost = settlement_total_from_cents(cost_breakdown.gross_cost_cents, quantity)?;
    let slippage_buffer =
        settlement_total_from_cents(cost_breakdown.slippage_buffer_cents, quantity)?;
    let total_adjusted_cost =
        settlement_total_from_cents(cost_breakdown.total_adjusted_cost_cents, quantity)?;
    Ok(OutcomeGroupLegScanEvidence {
        leg_id: candidate.leg_id.clone(),
        instrument_id: leg.instrument_id,
        order_side: candidate.order_side,
        target_notional: candidate.target_notional,
        executable_quantity: quantity,
        gross_cost,
        slippage_buffer,
        total_adjusted_cost,
        vwap_price: decimal_from_f64(required_vwap_field(vwap.vwap_price)?)?,
        limit_price: decimal_from_f64(required_vwap_field(vwap.limit_price)?)?,
        observed_unix_ms: book
            .observed_unix_ms
            .ok_or(OutcomeGroupScanBlockReason::MissingBookTimestamp)?,
    })
}

fn required_vwap_field(value: f64) -> Result<f64, OutcomeGroupScanBlockReason> {
    if is_positive_finite(value) {
        Ok(value)
    } else {
        Err(OutcomeGroupScanBlockReason::InvalidCost)
    }
}

fn settlement_total_from_cents(
    cents_per_unit: f64,
    quantity: Decimal,
) -> Result<Decimal, OutcomeGroupScanBlockReason> {
    let per_unit = decimal_from_non_negative_f64(cents_per_unit / CENTS_PER_SHARE)?;
    per_unit
        .checked_mul(quantity)
        .map(|value| value.round_dp(DECIMAL_F64_ROUND_DP))
        .ok_or(OutcomeGroupScanBlockReason::InvalidCost)
}

fn candidate_covers_standard_outcomes(
    group: &OutcomeGroup,
    quantities_by_leg: &BTreeMap<String, Decimal>,
) -> bool {
    group
        .terminal_states
        .values()
        .filter(|state| state.kind == TerminalStateKind::Standard)
        .all(|state| {
            quantities_by_leg.keys().any(|leg_id| {
                group
                    .tradable_legs
                    .get(leg_id)
                    .is_some_and(|leg| leg.outcome_label == state.label)
            })
        })
}

fn evaluate_state_payouts(
    payout_matrix: &PayoutMatrix,
    quantities_by_leg: &BTreeMap<String, Decimal>,
) -> Result<BTreeMap<String, Decimal>, OutcomeGroupScanBlockReason> {
    let mut payouts = BTreeMap::new();
    for (state_id, row) in &payout_matrix.payout_per_unit_by_state {
        let mut payout = Decimal::ZERO;
        for (index, leg_id) in payout_matrix.cols.iter().enumerate() {
            let quantity = match quantities_by_leg.get(leg_id) {
                Some(quantity) => *quantity,
                None => Decimal::ZERO,
            };
            let row_value = match row.get(index) {
                Some(value) => value,
                None => return Err(OutcomeGroupScanBlockReason::InvalidCost),
            };
            let addend = match row_value.checked_mul(quantity) {
                Some(value) => value,
                None => return Err(OutcomeGroupScanBlockReason::InvalidCost),
            };
            payout = match payout.checked_add(addend) {
                Some(value) => value,
                None => return Err(OutcomeGroupScanBlockReason::InvalidCost),
            };
        }
        payouts.insert(state_id.clone(), payout);
    }
    Ok(payouts)
}

fn minimum_payout(payouts: &BTreeMap<String, Decimal>) -> Option<Decimal> {
    let mut minimum = None;
    for payout in payouts.values() {
        minimum = match minimum {
            Some(current) if current <= *payout => Some(current),
            Some(_) | None => Some(*payout),
        };
    }
    minimum
}

fn validate_leg_constraints(
    quantity: Decimal,
    gross_notional: Decimal,
    constraints: &crate::bolt_v3_outcome_groups::OutcomeLegOrderConstraints,
) -> Result<(), OutcomeGroupScanBlockReason> {
    if quantity < constraints.min_quantity {
        return Err(OutcomeGroupScanBlockReason::MinQuantity);
    }
    if let Some(min_notional) = constraints.min_notional
        && gross_notional < min_notional
    {
        return Err(OutcomeGroupScanBlockReason::MinNotional);
    }
    if !quantity_step_aligned(quantity, constraints.quantity_step) {
        return Err(OutcomeGroupScanBlockReason::QuantityStep);
    }
    Ok(())
}

fn quantity_step_aligned(quantity: Decimal, step: Decimal) -> bool {
    if step <= Decimal::ZERO {
        return false;
    }
    let units = quantity / step;
    units.fract() == Decimal::ZERO
}

fn decimal_to_positive_f64(value: Decimal) -> Result<f64, OutcomeGroupScanBlockReason> {
    value
        .to_f64()
        .filter(|value| is_positive_finite(*value))
        .ok_or(OutcomeGroupScanBlockReason::InvalidCost)
}

fn decimal_from_f64(value: f64) -> Result<Decimal, OutcomeGroupScanBlockReason> {
    Decimal::from_f64(value)
        .map(|value| value.round_dp(DECIMAL_F64_ROUND_DP))
        .filter(|value| *value > Decimal::ZERO)
        .ok_or(OutcomeGroupScanBlockReason::InvalidCost)
}

fn decimal_from_non_negative_f64(value: f64) -> Result<Decimal, OutcomeGroupScanBlockReason> {
    Decimal::from_f64(value)
        .map(|value| value.round_dp(DECIMAL_F64_ROUND_DP))
        .filter(|value| *value >= Decimal::ZERO)
        .ok_or(OutcomeGroupScanBlockReason::InvalidCost)
}

fn positive_min(current: Decimal, candidate: Decimal) -> Option<Decimal> {
    if candidate <= Decimal::ZERO {
        return None;
    }
    if current <= Decimal::ZERO || candidate < current {
        Some(candidate)
    } else {
        Some(current)
    }
}

fn validation_block_reason(error: OutcomeGroupValidationError) -> OutcomeGroupScanBlockReason {
    if error.is_invalid_price_scale() {
        OutcomeGroupScanBlockReason::InvalidPriceScale
    } else if error.is_invalid_order_constraint() {
        OutcomeGroupScanBlockReason::MinQuantity
    } else {
        OutcomeGroupScanBlockReason::InvalidCost
    }
}

fn scan_reason_from_cost(reason: ExecutableCostBlockReason) -> OutcomeGroupScanBlockReason {
    match reason {
        ExecutableCostBlockReason::MissingOrderBook => OutcomeGroupScanBlockReason::MissingBook,
        ExecutableCostBlockReason::InsufficientDepth => {
            OutcomeGroupScanBlockReason::InsufficientDepth
        }
        ExecutableCostBlockReason::InvalidCost => OutcomeGroupScanBlockReason::InvalidCost,
        ExecutableCostBlockReason::UnsupportedOrderShape => {
            OutcomeGroupScanBlockReason::UnsupportedOrderSide
        }
    }
}

impl OutcomeGroupScanEvidence {
    fn empty(group: &OutcomeGroup) -> Self {
        Self {
            group_id: group.group_id.clone(),
            grouping_proof: group.grouping_proof.clone(),
            leg_costs: Vec::new(),
            state_payouts: BTreeMap::new(),
            guaranteed_payout: Decimal::ZERO,
            total_gross_cost: Decimal::ZERO,
            total_slippage_buffer: Decimal::ZERO,
            total_adjusted_cost: Decimal::ZERO,
            absolute_edge: Decimal::ZERO,
            edge_bps: Decimal::ZERO,
            min_depth_quantity: Decimal::ZERO,
            admissible: false,
            block_reason: None,
        }
    }

    fn blocked(mut self, reason: OutcomeGroupScanBlockReason) -> Self {
        self.admissible = false;
        self.block_reason = Some(reason);
        self
    }
}

fn insert_book_order_level(levels: &mut BTreeMap<Price, f64>, order: BookOrder) {
    insert_level(levels, order.price, order.size.as_f64());
}

fn insert_book_level(levels: &mut BTreeMap<Price, f64>, level: &BookLevel) {
    insert_level(levels, level.price.value, level.size());
}

fn insert_level(levels: &mut BTreeMap<Price, f64>, price: Price, size: f64) {
    if !is_non_negative_finite(size) || size <= ZERO_F64 {
        return;
    }
    levels
        .entry(price)
        .and_modify(|current| *current += size)
        .or_insert(size);
}
