use anyhow::{Context, Result};
use nautilus_model::{
    enums::OrderSide,
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::Quantity,
};
use rust_decimal::Decimal;

use super::{BoltV3PlannedFillLeg, economics_order_binding};
use crate::{
    bolt_v3_economics_runtime::{EconomicsAdmissionPolicy, EconomicsOrderBinding},
    bolt_v3_submit_admission::{BoltV3OrderAdmissionFacts, BoltV3SubmitIntentKind},
    economics::{
        LifecyclePath, PlannedFillLeg, PlannedFillNotional, PositionContext, PositionSide,
    },
    integrations::nautilus::economics::NautilusPlannedFillLeg,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3TerminalValueEntryPolicy {
    Breakeven,
    MinimumCoreEdgeRatio(Decimal),
}

impl BoltV3TerminalValueEntryPolicy {
    fn minimum_core_edge_ratio(&self) -> Result<Decimal> {
        match self {
            Self::Breakeven => Ok(Decimal::ZERO),
            Self::MinimumCoreEdgeRatio(value) => {
                anyhow::ensure!(
                    *value >= Decimal::ZERO,
                    "terminal-value entry requires a non-negative minimum core edge ratio"
                );
                Ok(*value)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3TerminalValueEntry {
    expected_terminal_value_per_unit: Decimal,
    minimum_core_edge_ratio: Decimal,
}

impl BoltV3TerminalValueEntry {
    pub fn try_new(
        expected_terminal_value_per_unit: Decimal,
        policy: BoltV3TerminalValueEntryPolicy,
    ) -> Result<Self> {
        anyhow::ensure!(
            expected_terminal_value_per_unit > Decimal::ZERO,
            "terminal-value entry requires a positive expected terminal value per unit"
        );
        let minimum_core_edge_ratio = policy.minimum_core_edge_ratio()?;
        Ok(Self {
            expected_terminal_value_per_unit,
            minimum_core_edge_ratio,
        })
    }

    pub const fn expected_terminal_value_per_unit(&self) -> Decimal {
        self.expected_terminal_value_per_unit
    }

    pub const fn minimum_core_edge_ratio(&self) -> Decimal {
        self.minimum_core_edge_ratio
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3FinalOrderEconomicsScenario {
    TerminalValueEntry(BoltV3TerminalValueEntry),
    PlannedRiskReducingExit {
        stored_entry_cost_per_unit: Decimal,
        position: PositionContext,
    },
    ForcedReduction {
        position: PositionContext,
    },
}

impl BoltV3FinalOrderEconomicsScenario {
    pub fn planned_risk_reducing_exit(
        stored_entry_cost_per_unit: Decimal,
        position: PositionContext,
    ) -> Result<Self> {
        anyhow::ensure!(
            stored_entry_cost_per_unit > Decimal::ZERO,
            "planned exit requires a positive stored entry cost per unit"
        );
        validate_position(&position)?;
        Ok(Self::PlannedRiskReducingExit {
            stored_entry_cost_per_unit,
            position,
        })
    }

    pub fn forced_reduction(position: PositionContext) -> Result<Self> {
        validate_position(&position)?;
        Ok(Self::ForcedReduction { position })
    }

    pub const fn intent_kind(&self) -> BoltV3SubmitIntentKind {
        match self {
            Self::TerminalValueEntry(_) => BoltV3SubmitIntentKind::Entry,
            Self::PlannedRiskReducingExit { .. } => BoltV3SubmitIntentKind::RiskReducingExit,
            Self::ForcedReduction { .. } => BoltV3SubmitIntentKind::KillSwitchForcedReduction,
        }
    }

    pub const fn lifecycle_path(&self) -> LifecyclePath {
        match self {
            Self::TerminalValueEntry(_) => LifecyclePath::HoldToRedemption,
            Self::PlannedRiskReducingExit { .. } | Self::ForcedReduction { .. } => {
                LifecyclePath::PlannedExit
            }
        }
    }

    pub const fn admission_policy(&self) -> EconomicsAdmissionPolicy {
        match self {
            Self::TerminalValueEntry(entry) => EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: entry.minimum_core_edge_ratio,
            },
            Self::PlannedRiskReducingExit { .. } | Self::ForcedReduction { .. } => {
                EconomicsAdmissionPolicy::RiskReduction
            }
        }
    }

    pub fn position(&self) -> Option<PositionContext> {
        match self {
            Self::TerminalValueEntry(_) => None,
            Self::PlannedRiskReducingExit { position, .. } | Self::ForcedReduction { position } => {
                Some(position.clone())
            }
        }
    }

    pub fn validate_order_shape(&self, order: &OrderAny) -> Result<()> {
        let expected_side = match self {
            Self::TerminalValueEntry(_) => OrderSide::Buy,
            Self::PlannedRiskReducingExit { position, .. } | Self::ForcedReduction { position } => {
                match position.side {
                    PositionSide::Long => OrderSide::Sell,
                    PositionSide::Short => OrderSide::Buy,
                }
            }
        };
        anyhow::ensure!(
            order.order_side() == expected_side,
            "final order side does not match its economics scenario"
        );
        Ok(())
    }

    pub fn gross_expected_value(&self, legs: &[NautilusPlannedFillLeg]) -> Result<Decimal> {
        anyhow::ensure!(
            !legs.is_empty(),
            "final economics requires retained fill levels"
        );
        match self {
            Self::TerminalValueEntry(entry) => checked_leg_value_sum(legs, |leg| {
                entry
                    .expected_terminal_value_per_unit
                    .checked_sub(leg.price)
            }),
            Self::PlannedRiskReducingExit {
                stored_entry_cost_per_unit,
                ..
            } => checked_leg_value_sum(legs, |leg| {
                leg.price.checked_sub(*stored_entry_cost_per_unit)
            }),
            Self::ForcedReduction { .. } => Ok(Decimal::ZERO),
        }
    }
}

fn validate_position(position: &PositionContext) -> Result<()> {
    anyhow::ensure!(
        position.quantity > Decimal::ZERO && position.holding_horizon_ns > 0,
        "order economics position requires positive quantity and holding horizon"
    );
    Ok(())
}

fn checked_leg_value_sum(
    legs: &[NautilusPlannedFillLeg],
    value_per_unit: impl Fn(&NautilusPlannedFillLeg) -> Option<Decimal>,
) -> Result<Decimal> {
    legs.iter()
        .try_fold(Decimal::ZERO, |total, leg| {
            let value =
                value_per_unit(leg).context("final economics value subtraction overflow")?;
            let contribution = value
                .checked_mul(leg.quantity)
                .context("final economics value multiplication overflow")?;
            total
                .checked_add(contribution)
                .context("final economics value sum overflow")
        })
        .map(|value| value.normalize())
}

#[derive(Clone, Debug)]
pub(super) struct NormalizedFinalFillPlan {
    legs: Vec<NautilusPlannedFillLeg>,
    planned_fill_notional: PlannedFillNotional,
    #[cfg(test)]
    final_dust: Decimal,
    order_binding: EconomicsOrderBinding,
}

#[derive(Clone, Debug)]
pub(super) struct FinalOrderEconomicsBasis {
    normalized: NormalizedFinalFillPlan,
    gross_expected_value: Decimal,
    reservation_basis: Decimal,
    lifecycle_path: LifecyclePath,
    policy: EconomicsAdmissionPolicy,
    position: Option<PositionContext>,
}

impl FinalOrderEconomicsBasis {
    pub(super) fn normalized_fill_legs(&self) -> &[NautilusPlannedFillLeg] {
        self.normalized.legs()
    }

    pub(super) const fn planned_fill_notional(&self) -> PlannedFillNotional {
        self.normalized.planned_fill_notional()
    }

    pub(super) const fn gross_expected_value(&self) -> Decimal {
        self.gross_expected_value
    }

    pub(super) const fn reservation_basis(&self) -> Decimal {
        self.reservation_basis
    }

    pub(super) const fn order_binding(&self) -> &EconomicsOrderBinding {
        self.normalized.order_binding()
    }

    pub(super) fn lifecycle_path(&self) -> LifecyclePath {
        self.lifecycle_path.clone()
    }

    pub(super) const fn policy(&self) -> EconomicsAdmissionPolicy {
        self.policy
    }

    pub(super) fn position(&self) -> Option<PositionContext> {
        self.position.clone()
    }
}

pub(super) fn seal_final_order_economics_basis(
    order: &OrderAny,
    instrument: Option<&InstrumentAny>,
    facts: BoltV3OrderAdmissionFacts,
    scenario: &BoltV3FinalOrderEconomicsScenario,
    candidate_fill_levels: Vec<BoltV3PlannedFillLeg>,
) -> Result<FinalOrderEconomicsBasis> {
    scenario.validate_order_shape(order)?;
    let normalized = normalize_final_fill_levels(order, instrument, facts, candidate_fill_levels)?;
    let gross_expected_value = scenario.gross_expected_value(normalized.legs())?;
    Ok(FinalOrderEconomicsBasis {
        normalized,
        gross_expected_value,
        reservation_basis: facts.reservation_basis,
        lifecycle_path: scenario.lifecycle_path(),
        policy: scenario.admission_policy(),
        position: scenario.position(),
    })
}

impl NormalizedFinalFillPlan {
    pub(super) fn legs(&self) -> &[NautilusPlannedFillLeg] {
        &self.legs
    }

    pub(super) const fn planned_fill_notional(&self) -> PlannedFillNotional {
        self.planned_fill_notional
    }

    #[cfg(test)]
    pub(super) fn provider_fee_legs(&self) -> &[NautilusPlannedFillLeg] {
        &self.legs
    }

    #[cfg(test)]
    pub(super) const fn final_dust(&self) -> Decimal {
        self.final_dust
    }

    pub(super) const fn order_binding(&self) -> &EconomicsOrderBinding {
        &self.order_binding
    }
}

pub(super) fn normalize_final_fill_levels(
    order: &OrderAny,
    instrument: Option<&InstrumentAny>,
    facts: BoltV3OrderAdmissionFacts,
    candidates: Vec<BoltV3PlannedFillLeg>,
) -> Result<NormalizedFinalFillPlan> {
    anyhow::ensure!(
        !candidates.is_empty(),
        "economics requires planned fill levels"
    );
    for candidate in &candidates {
        anyhow::ensure!(
            candidate.price > Decimal::ZERO && candidate.quantity > Decimal::ZERO,
            "economics planned fill level must be positive"
        );
        if let Some(instrument) = instrument {
            require_size_grid(instrument, candidate.quantity)?;
        }
        if order.price().is_some() {
            anyhow::ensure!(
                match order.order_side() {
                    OrderSide::Buy => candidate.price <= facts.price,
                    OrderSide::Sell => candidate.price >= facts.price,
                    OrderSide::NoOrderSide => false,
                },
                "economics planned fill level exceeds the final order limit"
            );
        }
    }

    if order.is_quote_quantity() {
        normalize_quote_quantity(order, instrument, facts, candidates)
    } else {
        normalize_base_quantity(order, instrument, facts, candidates)
    }
}

fn normalize_quote_quantity(
    order: &OrderAny,
    instrument: Option<&InstrumentAny>,
    facts: BoltV3OrderAdmissionFacts,
    candidates: Vec<BoltV3PlannedFillLeg>,
) -> Result<NormalizedFinalFillPlan> {
    let instrument = instrument.context("quote-quantity economics requires instrument context")?;
    let aggregate_candidate_notional =
        candidates.iter().try_fold(Decimal::ZERO, |total, level| {
            total
                .checked_add(
                    level
                        .price
                        .checked_mul(level.quantity)
                        .context("candidate quote notional overflow")?,
                )
                .context("aggregate candidate quote notional overflow")
        })?;
    anyhow::ensure!(
        aggregate_candidate_notional >= facts.order_quantity,
        "economics quote candidate levels under-cover the submitted quote quantity"
    );

    let mut remaining_quote = facts.order_quantity;
    let mut retained = Vec::new();
    let mut retained_by_level = Vec::with_capacity(candidates.len());
    for level in &candidates {
        let affordable_base = floor_to_size_increment(
            remaining_quote
                .checked_div(level.price)
                .context("quote allocation division")?,
            instrument,
        )?;
        let retained_base = affordable_base.min(level.quantity);
        retained_by_level.push(retained_base);
        if retained_base.is_zero() {
            continue;
        }
        require_size_grid(instrument, retained_base)?;
        let retained_notional = level
            .price
            .checked_mul(retained_base)
            .context("retained quote notional overflow")?;
        remaining_quote = remaining_quote
            .checked_sub(retained_notional)
            .context("remaining quote subtraction")?;
        retained.push(NautilusPlannedFillLeg {
            price: level.price,
            quantity: retained_base,
        });
    }

    let increment = instrument.size_increment().as_decimal();
    let executable_residual =
        candidates
            .iter()
            .zip(retained_by_level)
            .any(|(level, retained_quantity)| {
                retained_quantity < level.quantity
                    && level
                        .price
                        .checked_mul(increment)
                        .is_some_and(|one_increment_notional| {
                            remaining_quote >= one_increment_notional
                        })
            });
    anyhow::ensure!(
        !executable_residual,
        "economics quote allocation stopped while final residual remained executable"
    );
    finish_normalized_plan(order, retained, remaining_quote)
}

fn normalize_base_quantity(
    order: &OrderAny,
    instrument: Option<&InstrumentAny>,
    facts: BoltV3OrderAdmissionFacts,
    candidates: Vec<BoltV3PlannedFillLeg>,
) -> Result<NormalizedFinalFillPlan> {
    let mut remaining_base = facts.order_quantity;
    let mut retained = Vec::new();
    for level in candidates {
        let retained_base = level.quantity.min(remaining_base);
        if retained_base.is_zero() {
            continue;
        }
        if let Some(instrument) = instrument {
            require_size_grid(instrument, retained_base)?;
        }
        retained.push(NautilusPlannedFillLeg {
            price: level.price,
            quantity: retained_base,
        });
        remaining_base = remaining_base
            .checked_sub(retained_base)
            .context("remaining base subtraction")?;
        if remaining_base.is_zero() {
            break;
        }
    }
    anyhow::ensure!(
        remaining_base.is_zero(),
        "economics planned fill levels do not cover the final order"
    );
    finish_normalized_plan(order, retained, Decimal::ZERO)
}

pub(super) fn floor_to_size_increment(
    value: Decimal,
    instrument: &InstrumentAny,
) -> Result<Decimal> {
    let increment = instrument.size_increment().as_decimal();
    anyhow::ensure!(
        increment > Decimal::ZERO,
        "instrument size increment must be positive"
    );
    let whole_increments = value
        .checked_div(increment)
        .context("size-increment division overflow")?
        .trunc();
    let aligned = whole_increments
        .checked_mul(increment)
        .context("size-increment multiplication overflow")?;
    require_size_grid(instrument, aligned)
}

fn require_size_grid(instrument: &InstrumentAny, quantity: Decimal) -> Result<Decimal> {
    let candidate = Quantity::from_decimal_dp(quantity, instrument.size_precision())
        .map_err(|error| anyhow::anyhow!(error))?;
    anyhow::ensure!(
        candidate.as_decimal() == quantity,
        "quantity requires rounding to instrument size precision"
    );
    let normalized = instrument
        .try_normalize_qty(candidate)
        .map_err(|error| anyhow::anyhow!(error))?;
    anyhow::ensure!(
        normalized.as_decimal() == quantity,
        "quantity normalization changed the economics quantity"
    );
    Ok(normalized.as_decimal())
}

fn finish_normalized_plan(
    order: &OrderAny,
    legs: Vec<NautilusPlannedFillLeg>,
    final_dust: Decimal,
) -> Result<NormalizedFinalFillPlan> {
    anyhow::ensure!(
        !legs.is_empty(),
        "final economics retained no executable fill levels"
    );
    anyhow::ensure!(
        final_dust >= Decimal::ZERO,
        "final quote dust cannot be negative"
    );
    let core_legs = legs
        .iter()
        .map(|leg| PlannedFillLeg {
            price: leg.price,
            quantity: leg.quantity,
        })
        .collect::<Vec<_>>();
    Ok(NormalizedFinalFillPlan {
        legs,
        planned_fill_notional: PlannedFillNotional::from_legs(&core_legs)
            .map_err(|error| anyhow::anyhow!(error))?,
        #[cfg(test)]
        final_dust,
        order_binding: economics_order_binding(order).map_err(|error| anyhow::anyhow!(error))?,
    })
}

#[cfg(test)]
mod tests {
    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{AssetClass, OrderSide, TimeInForce},
        identifiers::{ClientOrderId, InstrumentId, StrategyId, Symbol, TraderId},
        instruments::{BinaryOption, InstrumentAny},
        orders::{LimitOrder, MarketOrder, Order, OrderAny},
        types::{Currency, Price, Quantity},
    };
    use rust_decimal::Decimal;
    use ustr::Ustr;

    use super::{
        BoltV3FinalOrderEconomicsScenario, BoltV3TerminalValueEntry,
        BoltV3TerminalValueEntryPolicy, normalize_final_fill_levels,
    };
    use crate::{
        bolt_v3_order_execution::{BoltV3PlannedFillLeg, economics_order_binding},
        bolt_v3_submit_admission::BoltV3OrderAdmissionFacts,
        economics::{PositionContext, PositionId, PositionSide},
    };

    fn binary_option(size_increment: &str) -> InstrumentAny {
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from("BASIS.POLYMARKET"),
            Symbol::from("BASIS"),
            AssetClass::Alternative,
            Currency::USD(),
            UnixNanos::from(1_u64),
            UnixNanos::from(2_u64),
            2,
            2,
            Price::from("0.01"),
            Quantity::from(size_increment),
            Some(Ustr::from("YES")),
            None,
            None,
            Some(Quantity::from(size_increment)),
            None,
            None,
            Some(Price::from("1.00")),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            UnixNanos::from(1_u64),
            UnixNanos::from(1_u64),
        ))
    }

    fn limit_order(
        client_order_id: &str,
        side: OrderSide,
        quantity: &str,
        price: &str,
        quote_quantity: bool,
    ) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("BASIS.POLYMARKET"),
                ClientOrderId::from(client_order_id),
                side,
                Quantity::from(quantity),
                Price::from(price),
                TimeInForce::Gtc,
                None,
                false,
                false,
                quote_quantity,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                UUID4::new(),
                UnixNanos::from(1_u64),
            )
            .expect("basis limit order should construct"),
        )
    }

    fn quote_market_order(client_order_id: &str, quantity: &str) -> OrderAny {
        OrderAny::Market(
            MarketOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from("BASIS.POLYMARKET"),
                ClientOrderId::from(client_order_id),
                OrderSide::Buy,
                Quantity::from(quantity),
                TimeInForce::Ioc,
                UUID4::new(),
                UnixNanos::from(1_u64),
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("basis quote-quantity market order should construct"),
        )
    }

    fn long_position(quantity: Decimal) -> PositionContext {
        PositionContext {
            position_id: PositionId::try_new("position-a".to_string())
                .expect("position id should be canonical"),
            side: PositionSide::Long,
            quantity,
            holding_horizon_ns: 1,
        }
    }

    #[test]
    fn base_quantity_seal_truncates_over_cover_and_recomputes_every_money_basis() {
        let order = limit_order("basis-base", OrderSide::Buy, "1.00", "0.50", false);
        let facts = BoltV3OrderAdmissionFacts {
            price: Decimal::new(50, 2),
            order_quantity: Decimal::ONE,
            reservation_basis: Decimal::new(50, 2),
        };
        let plan = normalize_final_fill_levels(
            &order,
            Some(&binary_option("0.05")),
            facts,
            vec![BoltV3PlannedFillLeg {
                price: Decimal::new(40, 2),
                quantity: Decimal::from(2),
            }],
        )
        .expect("over-cover should truncate to the final base quantity");
        let scenario = BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
            BoltV3TerminalValueEntry::try_new(
                Decimal::new(70, 2),
                BoltV3TerminalValueEntryPolicy::Breakeven,
            )
            .expect("terminal entry should construct"),
        );

        assert_eq!(plan.legs().len(), 1);
        assert_eq!(plan.legs()[0].quantity, Decimal::ONE);
        assert_eq!(plan.planned_fill_notional().amount(), Decimal::new(40, 2));
        assert_eq!(
            scenario.gross_expected_value(plan.legs()).unwrap(),
            Decimal::new(30, 2)
        );
        assert_eq!(facts.reservation_basis, Decimal::new(50, 2));
        assert_eq!(plan.final_dust(), Decimal::ZERO);
        assert_eq!(
            plan.order_binding(),
            &economics_order_binding(&order).unwrap()
        );
    }

    #[test]
    fn quote_quantity_seal_rolls_residual_into_a_later_cheaper_level() {
        let instrument = binary_option("0.05");
        let order = limit_order("basis-quote", OrderSide::Sell, "1.23", "0.09", true);
        let facts = BoltV3OrderAdmissionFacts {
            price: Decimal::new(9, 2),
            order_quantity: Decimal::new(123, 2),
            reservation_basis: Decimal::new(123, 2),
        };
        let plan = normalize_final_fill_levels(
            &order,
            Some(&instrument),
            facts,
            vec![
                BoltV3PlannedFillLeg {
                    price: Decimal::new(50, 2),
                    quantity: Decimal::from(3),
                },
                BoltV3PlannedFillLeg {
                    price: Decimal::new(9, 2),
                    quantity: Decimal::new(5, 2),
                },
            ],
        )
        .expect("the first-level residual should remain available to the cheaper level");
        let scenario = BoltV3FinalOrderEconomicsScenario::planned_risk_reducing_exit(
            Decimal::new(4, 2),
            long_position(Decimal::from(3)),
        )
        .expect("planned exit should construct");

        assert_eq!(plan.legs().len(), 2);
        assert_eq!(plan.legs()[0].quantity, Decimal::new(245, 2));
        assert_eq!(plan.legs()[1].quantity, Decimal::new(5, 2));
        assert_eq!(
            plan.planned_fill_notional().amount(),
            Decimal::new(12295, 4)
        );
        assert_eq!(
            scenario.gross_expected_value(plan.legs()).unwrap(),
            Decimal::new(11295, 4)
        );
        assert!(std::ptr::eq(plan.provider_fee_legs(), plan.legs()));
        assert_eq!(plan.final_dust(), Decimal::new(5, 4));
        assert_eq!(facts.reservation_basis, Decimal::new(123, 2));
        assert_eq!(
            plan.order_binding(),
            &economics_order_binding(&order).unwrap()
        );
    }

    #[test]
    fn quote_quantity_seal_rejects_candidate_undercoverage_instead_of_calling_it_dust() {
        let order = limit_order("basis-under", OrderSide::Sell, "1.23", "0.09", true);
        let error = normalize_final_fill_levels(
            &order,
            Some(&binary_option("0.05")),
            BoltV3OrderAdmissionFacts {
                price: Decimal::new(9, 2),
                order_quantity: Decimal::new(123, 2),
                reservation_basis: Decimal::new(123, 2),
            },
            vec![BoltV3PlannedFillLeg {
                price: Decimal::new(50, 2),
                quantity: Decimal::from(2),
            }],
        )
        .expect_err("aggregate candidate liquidity below the submitted quote must fail");

        assert!(error.to_string().contains("under-cover"), "{error:#}");
    }

    #[test]
    fn quote_quantity_seal_accepts_a_price_less_market_order_without_inventing_a_limit() {
        let order = quote_market_order("basis-market", "1.23");
        let plan = normalize_final_fill_levels(
            &order,
            Some(&binary_option("0.05")),
            BoltV3OrderAdmissionFacts {
                price: Decimal::new(50, 2),
                order_quantity: Decimal::new(123, 2),
                reservation_basis: Decimal::new(123, 2),
            },
            vec![BoltV3PlannedFillLeg {
                price: Decimal::new(50, 2),
                quantity: Decimal::from(3),
            }],
        )
        .expect("a price-less market order should use fills without inventing a limit");

        assert!(order.price().is_none());
        assert_eq!(plan.legs()[0].quantity, Decimal::new(245, 2));
        assert_eq!(plan.final_dust(), Decimal::new(5, 3));
    }

    #[test]
    fn final_basis_rejects_side_scenario_and_limit_mismatches() {
        let sell = limit_order("basis-side", OrderSide::Sell, "1.00", "0.50", false);
        let terminal = BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
            BoltV3TerminalValueEntry::try_new(
                Decimal::new(70, 2),
                BoltV3TerminalValueEntryPolicy::Breakeven,
            )
            .expect("terminal entry should construct"),
        );
        assert!(terminal.validate_order_shape(&sell).is_err());

        let buy = limit_order("basis-limit", OrderSide::Buy, "1.00", "0.50", false);
        let error = normalize_final_fill_levels(
            &buy,
            Some(&binary_option("0.05")),
            BoltV3OrderAdmissionFacts {
                price: Decimal::new(50, 2),
                order_quantity: Decimal::ONE,
                reservation_basis: Decimal::new(50, 2),
            },
            vec![BoltV3PlannedFillLeg {
                price: Decimal::new(51, 2),
                quantity: Decimal::ONE,
            }],
        )
        .expect_err("a buy fill above the final limit must fail");
        assert!(error.to_string().contains("final order limit"));
    }

    #[test]
    fn planned_exit_gross_uses_the_post_clamp_quantity() {
        let order = limit_order("basis-exit", OrderSide::Sell, "1.00", "0.60", false);
        let plan = normalize_final_fill_levels(
            &order,
            Some(&binary_option("0.05")),
            BoltV3OrderAdmissionFacts {
                price: Decimal::new(60, 2),
                order_quantity: Decimal::ONE,
                reservation_basis: Decimal::new(60, 2),
            },
            vec![BoltV3PlannedFillLeg {
                price: Decimal::new(60, 2),
                quantity: Decimal::from(2),
            }],
        )
        .expect("the candidate leg should truncate to the post-clamp order");
        let scenario = BoltV3FinalOrderEconomicsScenario::planned_risk_reducing_exit(
            Decimal::new(40, 2),
            long_position(Decimal::ONE),
        )
        .expect("planned exit should construct");

        assert_eq!(plan.legs()[0].quantity, Decimal::ONE);
        assert_eq!(
            scenario.gross_expected_value(plan.legs()).unwrap(),
            Decimal::new(20, 2)
        );
    }

    #[test]
    fn edge_exit_seals_after_the_final_quantity_clamp() {
        let order = limit_order("basis-exit-sealed", OrderSide::Sell, "1.00", "0.60", false);
        let facts = BoltV3OrderAdmissionFacts {
            price: Decimal::new(60, 2),
            order_quantity: Decimal::ONE,
            reservation_basis: Decimal::new(60, 2),
        };
        let scenario = BoltV3FinalOrderEconomicsScenario::planned_risk_reducing_exit(
            Decimal::new(40, 2),
            long_position(Decimal::ONE),
        )
        .expect("planned exit should construct");
        let basis = super::seal_final_order_economics_basis(
            &order,
            Some(&binary_option("0.05")),
            facts,
            &scenario,
            vec![BoltV3PlannedFillLeg {
                price: Decimal::new(60, 2),
                quantity: Decimal::from(2),
            }],
        )
        .expect("post-clamp final order should seal");

        assert_eq!(basis.normalized_fill_legs()[0].quantity, Decimal::ONE);
        assert_eq!(basis.gross_expected_value(), Decimal::new(20, 2));
        assert_eq!(
            basis.lifecycle_path(),
            crate::economics::LifecyclePath::PlannedExit
        );
    }
}
