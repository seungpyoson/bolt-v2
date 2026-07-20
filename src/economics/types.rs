use std::{error::Error, fmt, str::FromStr};

pub use nautilus_model::types::Currency;
use rust_decimal::Decimal;

const BASIS_POINT_DECIMAL_SCALE: u32 = 4;

pub fn basis_points_to_fraction(basis_points: Decimal) -> Result<Decimal, EconomicsUnavailable> {
    basis_points
        .checked_mul(Decimal::new(1, BASIS_POINT_DECIMAL_SCALE))
        .ok_or(EconomicsUnavailable::InvalidDecimal)
}

macro_rules! domain_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, EconomicsUnavailable> {
                let value = value.into();
                if value.is_empty() || value.trim() != value || value.chars().any(char::is_control)
                {
                    return Err(EconomicsUnavailable::InvalidIdentifier {
                        kind: stringify!($name),
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

domain_id!(EconomicComponentId);
domain_id!(ExecutionClientId);
domain_id!(AccountId);
domain_id!(InstrumentId);
domain_id!(ProductSurfaceId);
domain_id!(OrderId);
domain_id!(FillId);
domain_id!(PositionId);
domain_id!(ActionId);
domain_id!(MarketId);
domain_id!(DecisionCorrelationId);
domain_id!(ReportingPolicyId);
domain_id!(EdgeBasisPolicyId);
domain_id!(FormulaId);
domain_id!(SourceId);
domain_id!(SnapshotId);
domain_id!(ValuationRouteId);
domain_id!(CanonicalEconomicEventId);
domain_id!(ComponentDiscriminator);
domain_id!(AuthorityEventId);
domain_id!(RoutingAttachmentId);

pub fn currency_from_code(value: &str) -> Result<Currency, EconomicsUnavailable> {
    Currency::from_str(value)
        .map_err(|_| EconomicsUnavailable::InvalidIdentifier { kind: "Currency" })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedNativeEffect {
    amount: Decimal,
    currency: Currency,
}

impl SignedNativeEffect {
    pub fn currency(amount: Decimal, currency: Currency) -> Result<Self, EconomicsUnavailable> {
        Self::reject_zero(amount)?;
        Ok(Self { amount, currency })
    }

    pub fn amount(&self) -> Decimal {
        self.amount
    }

    pub fn currency_id(&self) -> Currency {
        self.currency
    }

    fn reject_zero(value: Decimal) -> Result<(), EconomicsUnavailable> {
        if value.is_zero() {
            Err(EconomicsUnavailable::ZeroNativeEffect)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RiskBoundAuthority {
    VenueMaximum,
    VenueRateCapWithPriceStress,
    OperatorRiskLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionTreatment {
    GuaranteedConditionalOnAction,
    RiskBound { authority: RiskBoundAuthority },
    ForecastOnly,
}

impl AdmissionTreatment {
    pub fn authorizes_admission(self) -> bool {
        !matches!(self, Self::ForecastOnly)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EconomicClass {
    Charge,
    Credit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionKind {
    ProtocolTrading,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CarryKind {
    Funding,
    BorrowInterest,
    SuppliedBalanceInterest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleKind {
    SettlementCharge,
    RedemptionCharge,
    ExerciseCharge,
    SplitMergeCharge,
    LiquidationCharge,
    ActivationCharge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferKind {
    DepositCharge,
    WithdrawalCharge,
    BridgeCharge,
    ConversionCharge,
    NetworkGasCharge,
    IntermediaryCharge,
    InterAccountTransferCharge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncentiveKind {
    MakerRebate,
    LiquidityReward,
    HoldingReward,
    ReferralReward,
    Kickback,
    FeeCredit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EconomicKind {
    Execution(ExecutionKind),
    Carry(CarryKind),
    Lifecycle(LifecycleKind),
    Transfer(TransferKind),
    Incentive(IncentiveKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EconomicScope {
    Decision {
        decision_correlation_id: DecisionCorrelationId,
    },
    Fill {
        order_id: OrderId,
        fill_id: FillId,
    },
    Order {
        order_id: OrderId,
    },
    PositionInterval {
        position_id: PositionId,
        starts_at_ns: u64,
        ends_at_ns: u64,
    },
    Action {
        action_id: ActionId,
    },
    MarketPeriod {
        market_id: MarketId,
        starts_at_ns: u64,
        ends_at_ns: u64,
    },
    AccountPeriod {
        account_id: AccountId,
        starts_at_ns: u64,
        ends_at_ns: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceValidity {
    pub source_id: SourceId,
    pub snapshot_id: SnapshotId,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalculationFactor {
    pub factor_id: FormulaId,
    pub value: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValuationEvidence {
    pub native_effect: SignedNativeEffect,
    pub normalized_amount: Decimal,
    pub reporting_unit: Currency,
    pub route_id: Option<ValuationRouteId>,
    pub legs: Vec<ValuationLegEvidence>,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valued_at_ns: u64,
    pub valid_until_ns: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValuationTransform {
    ExactAmount,
    MultiplicativeRate(Decimal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValuationLegEvidence {
    pub from_unit: Currency,
    pub source_currency: Currency,
    pub to_unit: Currency,
    pub transform: ValuationTransform,
    pub source_snapshot_id: SnapshotId,
    pub observed_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValuationRoute {
    pub route_id: ValuationRouteId,
    pub from_unit: Currency,
    pub to_currency: Currency,
    pub legs: Vec<ValuationLegEvidence>,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PointEstimate {
    NonZero(SignedNativeEffect),
    ProvenZero { factor_id: FormulaId },
}

impl PointEstimate {
    pub fn effect(&self) -> Option<&SignedNativeEffect> {
        match self {
            Self::NonZero(effect) => Some(effect),
            Self::ProvenZero { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EstimatedEconomicComponent {
    pub component_id: EconomicComponentId,
    pub class: EconomicClass,
    pub kind: EconomicKind,
    pub scope: EconomicScope,
    pub point_estimate: PointEstimate,
    pub debit_risk_bound: Option<SignedNativeEffect>,
    pub admission_treatment: AdmissionTreatment,
    pub calculation_factors: Vec<CalculationFactor>,
    pub formula_id: FormulaId,
    pub source: SourceValidity,
    pub normalized: Option<ValuationEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueQuoteEstimate {
    pub authority: SourceValidity,
    pub dependency_sources: Vec<SourceValidity>,
    pub components: Vec<EstimatedEconomicComponent>,
}

impl EstimatedEconomicComponent {
    pub fn authorizes_admission(&self) -> bool {
        self.admission_treatment.authorizes_admission()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiquidityRoleAssumption {
    GuaranteedMaker,
    Taker,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedFillLeg {
    pub price: Decimal,
    pub quantity: Decimal,
}

macro_rules! economic_amount {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name(Decimal);

        impl $name {
            pub const fn amount(self) -> Decimal {
                self.0
            }
        }
    };
    ($name:ident, $constructor_visibility:vis, $is_valid:expr) => {
        economic_amount!($name);

        impl $name {
            $constructor_visibility fn new(
                amount: Decimal,
            ) -> Result<Self, EconomicsUnavailable> {
                if !($is_valid)(amount) {
                    return Err(EconomicsUnavailable::InvalidDecimal);
                }
                Ok(Self(amount))
            }

        }
    };
}

economic_amount!(PlannedFillNotional);
economic_amount!(ReservationBasis, pub, |amount: Decimal| amount
    >= Decimal::ZERO);
economic_amount!(GuaranteedDebit, pub(crate), |amount: Decimal| amount >= Decimal::ZERO);
economic_amount!(FullReservationLiability);
economic_amount!(EdgeBasisAmount, pub, |amount: Decimal| amount
    > Decimal::ZERO);

impl PlannedFillNotional {
    pub fn from_legs(legs: &[PlannedFillLeg]) -> Result<Self, EconomicsUnavailable> {
        if legs.is_empty() {
            return Err(EconomicsUnavailable::InvalidPlannedFill);
        }
        let amount = legs.iter().try_fold(Decimal::ZERO, |total, leg| {
            if leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO {
                return None;
            }
            total.checked_add(leg.price.checked_mul(leg.quantity)?)
        });
        let amount = amount.ok_or(EconomicsUnavailable::InvalidPlannedFill)?;
        if amount <= Decimal::ZERO {
            return Err(EconomicsUnavailable::InvalidPlannedFill);
        }
        Ok(Self(amount))
    }
}

impl FullReservationLiability {
    pub fn from_parts(
        basis: ReservationBasis,
        debit: GuaranteedDebit,
    ) -> Result<Self, EconomicsUnavailable> {
        let amount = basis
            .amount()
            .checked_add(debit.amount())
            .ok_or(EconomicsUnavailable::InvalidDecimal)?;
        Ok(Self(amount))
    }
}

#[cfg(test)]
mod economic_amount_tests {
    use super::*;

    #[test]
    fn full_reservation_liability_overflow_fails_closed() {
        let basis = ReservationBasis::new(Decimal::MAX).expect("maximum decimal is non-negative");
        let debit = GuaranteedDebit::new(Decimal::ONE).expect("one is non-negative");

        assert_eq!(
            FullReservationLiability::from_parts(basis, debit),
            Err(EconomicsUnavailable::InvalidDecimal)
        );
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingContext {
    pub attached_charge: Option<RoutingAttachment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingAttachment {
    pub attachment_id: RoutingAttachmentId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionSide {
    Long,
    Short,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionContext {
    pub side: PositionSide,
    pub quantity: Decimal,
    pub holding_horizon_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecyclePath {
    PlannedExit,
    HoldToSettlement,
    HoldToRedemption,
    Transfer { action_id: ActionId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicQuoteRequest {
    pub execution_client_id: ExecutionClientId,
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
    pub product_surface_id: ProductSurfaceId,
    pub order_side: OrderSide,
    pub liquidity_role: LiquidityRoleAssumption,
    pub planned_fill_legs: Vec<PlannedFillLeg>,
    pub routing: RoutingContext,
    pub position: Option<PositionContext>,
    pub lifecycle_path: LifecyclePath,
    pub reporting_policy_id: ReportingPolicyId,
    pub reporting_unit: Currency,
    pub edge_basis_policy_id: EdgeBasisPolicyId,
    pub requested_at_ns: u64,
    pub decision_correlation_id: DecisionCorrelationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EdgeBasisEvidence {
    pub policy_id: EdgeBasisPolicyId,
    pub resolver_id: FormulaId,
    pub product_metadata_source: SourceId,
    pub policy_version: u64,
    pub normalized_amount: EdgeBasisAmount,
    pub scope: EconomicScope,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedEdgeBasis {
    pub normalized_amount: EdgeBasisAmount,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicQuote {
    pub(super) decision_correlation_id: DecisionCorrelationId,
    pub(super) requested_at_ns: u64,
    pub(super) edge_basis_policy_id: EdgeBasisPolicyId,
    pub(super) components: Vec<EstimatedEconomicComponent>,
    pub(super) normalizations: Vec<ValuationEvidence>,
    pub(super) core_total: Decimal,
    pub(super) forecast_total: Decimal,
    pub(super) forecast_complete: bool,
    pub(super) missing_forecast_component_ids: Vec<EconomicComponentId>,
    pub(super) reporting_unit: Currency,
    pub(super) valid_until_ns: u64,
}

impl EconomicQuote {
    pub fn decision_correlation_id(&self) -> &DecisionCorrelationId {
        &self.decision_correlation_id
    }

    pub fn components(&self) -> &[EstimatedEconomicComponent] {
        &self.components
    }

    pub fn normalizations(&self) -> &[ValuationEvidence] {
        &self.normalizations
    }

    pub fn core_total(&self) -> Decimal {
        self.core_total
    }

    pub fn forecast_total(&self) -> Decimal {
        self.forecast_total
    }

    pub fn forecast_complete(&self) -> bool {
        self.forecast_complete
    }

    pub fn missing_forecast_component_ids(&self) -> &[EconomicComponentId] {
        &self.missing_forecast_component_ids
    }

    pub fn reporting_unit(&self) -> &Currency {
        &self.reporting_unit
    }

    pub fn valid_until_ns(&self) -> u64 {
        self.valid_until_ns
    }

    pub(crate) fn cap_valid_until_ns(&mut self, valid_until_ns: u64) {
        self.valid_until_ns = self.valid_until_ns.min(valid_until_ns);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetEdgeQuote {
    pub(super) gross_expected_value: Decimal,
    pub(super) core_net_edge: Decimal,
    pub(super) forecast_net_edge: Decimal,
    pub(super) core_edge_ratio: Decimal,
    pub(super) forecast_edge_ratio: Decimal,
    pub(super) basis: EdgeBasisEvidence,
}

impl NetEdgeQuote {
    pub fn gross_expected_value(&self) -> Decimal {
        self.gross_expected_value
    }

    pub fn core_net_edge(&self) -> Decimal {
        self.core_net_edge
    }

    pub fn forecast_net_edge(&self) -> Decimal {
        self.forecast_net_edge
    }

    pub fn core_edge_ratio(&self) -> Decimal {
        self.core_edge_ratio
    }

    pub fn forecast_edge_ratio(&self) -> Decimal {
        self.forecast_edge_ratio
    }

    pub fn basis(&self) -> &EdgeBasisEvidence {
        &self.basis
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActualEntryKey {
    pub execution_client_id: ExecutionClientId,
    pub account_id: AccountId,
    pub canonical_event_id: CanonicalEconomicEventId,
    pub kind: EconomicKind,
    pub native_unit: Currency,
    pub component_discriminator: ComponentDiscriminator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceOrigin {
    VenueBookedAmount,
    LocalProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActualEconomicEntry {
    pub key: ActualEntryKey,
    pub class: EconomicClass,
    pub scope: EconomicScope,
    pub effect: SignedNativeEffect,
    pub authority_event_id: AuthorityEventId,
    pub venue_event_at_ns: u64,
    pub ingested_at_ns: u64,
    pub authority_source: SourceId,
    pub evidence_origin: EvidenceOrigin,
    pub corrects: Vec<CanonicalEconomicEventId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValuationRequest {
    pub reporting_unit: Currency,
    pub reporting_policy_id: ReportingPolicyId,
    pub requested_at_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EconomicsUnavailable {
    InvalidIdentifier { kind: &'static str },
    ZeroNativeEffect,
    InvalidPlannedFill,
    InvalidQuoteValidityPolicy,
    InvalidDecimal,
    EconomicClassSignMismatch,
    InvalidProvenZeroPoint { component_id: EconomicComponentId },
    MissingQuoteAuthority,
    AmbiguousQuoteAuthority,
    InvalidSourceTimeline { source_id: SourceId },
    StaleSource { source_id: SourceId },
    DuplicateComponent { component_id: EconomicComponentId },
    DuplicateCalculationFactor { factor_id: FormulaId },
    MissingDebitRiskBound { component_id: EconomicComponentId },
    InvalidDebitRiskBound { component_id: EconomicComponentId },
    MissingValuation { unit: Currency },
    AmbiguousValuation { unit: Currency },
    MissingValuationRoute { from: Currency, to: Currency },
    DisconnectedValuationRoute { route_id: ValuationRouteId },
    CyclicValuationRoute { route_id: ValuationRouteId },
    StaleValuation { route_id: ValuationRouteId },
    InvalidValuationRate { route_id: ValuationRouteId },
    ValuationEvidenceMismatch,
    InvalidEdgeBasis,
    NonPositiveNetEdge,
    StaleEdgeBasis { valid_until_ns: u64 },
    EdgeBasisPolicyMismatch,
    RequiredCapabilityStale { valid_until_ns: u64 },
    ProviderQuoteUnavailable { source_id: SourceId },
}

impl fmt::Display for EconomicsUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, formatter)
    }
}

impl Error for EconomicsUnavailable {}
