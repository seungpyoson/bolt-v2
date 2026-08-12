use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicsError {
    EmptyIdentifier {
        field: &'static str,
    },
    NonCanonicalIdentifier {
        field: &'static str,
    },
    NonPositiveValue {
        field: &'static str,
    },
    ZeroNativeEffect,
    InvalidPlannedFill,
    MissingHoldingHorizon,
    MissingDebitRiskBound {
        component_id: EconomicComponentId,
    },
    InvalidDebitRiskBound {
        component_id: EconomicComponentId,
    },
    DuplicateComponent {
        component_id: EconomicComponentId,
    },
    DuplicateCalculationFactor {
        factor_id: FormulaId,
    },
    EconomicClassSignMismatch {
        component_id: EconomicComponentId,
    },
    EffectScopeMismatch {
        component_id: EconomicComponentId,
    },
    InvalidSourceTimeline {
        source_id: SourceIdentity,
    },
    StaleSource {
        source_id: SourceIdentity,
    },
    MissingValuation {
        from: NativeUnitId,
        to: CurrencyId,
    },
    ContradictoryValuation {
        from: NativeUnitId,
        to: CurrencyId,
    },
    InvalidValuationRoute {
        route_id: ValuationRouteId,
    },
    StaleValuation {
        route_id: ValuationRouteId,
    },
    EdgeBasisPolicyMismatch,
    EdgeBasisScopeMismatch,
    StaleEdgeBasis,
    GrossCurrencyMismatch {
        gross_currency: CurrencyId,
        reporting_currency: CurrencyId,
    },
    MissingGuaranteedPointValuation {
        component_id: EconomicComponentId,
    },
    RequiredCapabilityStale {
        valid_until_ns: u64,
    },
    ArithmeticOverflow,
}

impl std::fmt::Display for EconomicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyIdentifier { field } => write!(f, "economics {field} is empty"),
            Self::NonCanonicalIdentifier { field } => {
                write!(f, "economics {field} is not canonical")
            }
            Self::NonPositiveValue { field } => {
                write!(f, "economics {field} must be positive")
            }
            Self::ZeroNativeEffect => f.write_str("economics native effect must be non-zero"),
            Self::InvalidPlannedFill => f.write_str("economics planned fill is invalid"),
            Self::MissingHoldingHorizon => {
                f.write_str("economics carry-bearing request has no holding horizon")
            }
            Self::MissingDebitRiskBound { component_id } => {
                write!(
                    f,
                    "economics component {component_id} has no debit risk bound"
                )
            }
            Self::InvalidDebitRiskBound { component_id } => {
                write!(
                    f,
                    "economics component {component_id} has an invalid debit risk bound"
                )
            }
            Self::DuplicateComponent { component_id } => {
                write!(f, "economics component {component_id} is duplicated")
            }
            Self::DuplicateCalculationFactor { factor_id } => {
                write!(f, "economics calculation factor {factor_id} is duplicated")
            }
            Self::EconomicClassSignMismatch { component_id } => write!(
                f,
                "economics component {component_id} class contradicts its signed effect"
            ),
            Self::EffectScopeMismatch { component_id } => write!(
                f,
                "economics component {component_id} scope does not match the quote request"
            ),
            Self::InvalidSourceTimeline { source_id } => {
                write!(f, "economics source {source_id} has an invalid timeline")
            }
            Self::StaleSource { source_id } => {
                write!(f, "economics source {source_id} is stale")
            }
            Self::MissingValuation { from, to } => {
                write!(f, "economics valuation is missing for {from} -> {to}")
            }
            Self::ContradictoryValuation { from, to } => {
                write!(f, "economics valuation is contradictory for {from} -> {to}")
            }
            Self::InvalidValuationRoute { route_id } => {
                write!(f, "economics valuation route {route_id} is invalid")
            }
            Self::StaleValuation { route_id } => {
                write!(f, "economics valuation route {route_id} is stale")
            }
            Self::EdgeBasisPolicyMismatch => {
                f.write_str("economics edge-basis policy does not match the quote")
            }
            Self::EdgeBasisScopeMismatch => {
                f.write_str("economics edge-basis scope does not match the quote")
            }
            Self::StaleEdgeBasis => f.write_str("economics edge basis is stale"),
            Self::GrossCurrencyMismatch {
                gross_currency,
                reporting_currency,
            } => write!(
                f,
                "economics gross currency {gross_currency} does not match reporting currency {reporting_currency}"
            ),
            Self::MissingGuaranteedPointValuation { component_id } => write!(
                f,
                "economics guaranteed component {component_id} has no point valuation"
            ),
            Self::RequiredCapabilityStale { valid_until_ns } => write!(
                f,
                "economics required capability expired at {valid_until_ns}"
            ),
            Self::ArithmeticOverflow => f.write_str("economics arithmetic overflowed"),
        }
    }
}

impl std::error::Error for EconomicsError {}

fn validate_identifier(
    field: &'static str,
    value: impl Into<String>,
) -> Result<String, EconomicsError> {
    let value = value.into();
    if value.is_empty() {
        return Err(EconomicsError::EmptyIdentifier { field });
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(EconomicsError::NonCanonicalIdentifier { field });
    }
    Ok(value)
}

macro_rules! validated_identifier {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, EconomicsError> {
                validate_identifier($field, value).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

validated_identifier!(CurrencyId, "currency_id");
validated_identifier!(AssetId, "asset_id");
validated_identifier!(EconomicsInstrumentId, "instrument_id");
validated_identifier!(ExecutionClientId, "execution_client_id");
validated_identifier!(AccountId, "account_id");
validated_identifier!(ProductSurfaceId, "product_surface_id");
validated_identifier!(DecisionCorrelationId, "decision_correlation_id");
validated_identifier!(ReportingPolicyId, "reporting_policy_id");
validated_identifier!(EdgeBasisPolicyId, "edge_basis_policy_id");
validated_identifier!(RoutingAttachmentId, "routing_attachment_id");
validated_identifier!(ActionId, "action_id");
validated_identifier!(PositionId, "position_id");
validated_identifier!(EconomicComponentId, "economic_component_id");
validated_identifier!(FormulaId, "formula_id");
validated_identifier!(SourceIdentity, "source_identity");
validated_identifier!(SnapshotId, "snapshot_id");
validated_identifier!(ValuationRouteId, "valuation_route_id");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NativeUnitId {
    Currency(CurrencyId),
    Asset(AssetId),
}

impl std::fmt::Display for NativeUnitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Currency(currency) => currency.fmt(f),
            Self::Asset(asset) => asset.fmt(f),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryApplication {
    AlreadyAppliedToGrossFill,
    ApplyOnceToNetPortfolio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedNativeEffect {
    CurrencyAmount {
        amount: Decimal,
        currency_id: CurrencyId,
    },
    AssetQuantity {
        quantity: Decimal,
        asset_id: AssetId,
        inventory_application: InventoryApplication,
    },
}

impl SignedNativeEffect {
    pub fn currency(amount: Decimal, currency_id: CurrencyId) -> Result<Self, EconomicsError> {
        if amount.is_zero() {
            return Err(EconomicsError::ZeroNativeEffect);
        }
        Ok(Self::CurrencyAmount {
            amount,
            currency_id,
        })
    }

    pub fn asset(
        quantity: Decimal,
        asset_id: AssetId,
        inventory_application: InventoryApplication,
    ) -> Result<Self, EconomicsError> {
        if quantity.is_zero() {
            return Err(EconomicsError::ZeroNativeEffect);
        }
        Ok(Self::AssetQuantity {
            quantity,
            asset_id,
            inventory_application,
        })
    }

    pub fn amount(&self) -> Decimal {
        match self {
            Self::CurrencyAmount { amount, .. } => *amount,
            Self::AssetQuantity { quantity, .. } => *quantity,
        }
    }

    pub fn unit(&self) -> NativeUnitId {
        match self {
            Self::CurrencyAmount { currency_id, .. } => NativeUnitId::Currency(currency_id.clone()),
            Self::AssetQuantity { asset_id, .. } => NativeUnitId::Asset(asset_id.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidityRole {
    GuaranteedMaker,
    Taker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Long,
    Short,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFillLeg {
    pub price: Decimal,
    pub quantity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingContext {
    pub attached_charge: Option<RoutingAttachmentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionContext {
    pub position_id: PositionId,
    pub side: PositionSide,
    pub quantity: Decimal,
    pub holding_horizon_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecyclePath {
    PlannedExit,
    HoldToSettlement,
    HoldToRedemption,
    Transfer { action_id: ActionId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskBoundAuthority {
    VenueMaximum,
    VenueRateCapWithPriceStress,
    OperatorRiskLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionTreatment {
    GuaranteedConditionalOnAction,
    RiskBound { authority: RiskBoundAuthority },
    ForecastOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EconomicClass {
    Charge,
    Credit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionKind {
    ProtocolTrading,
    AttachedRoutingCharge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryKind {
    Funding,
    BorrowInterest,
    SuppliedBalanceInterest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncentiveKind {
    MakerRebate,
    LiquidityReward,
    HoldingReward,
    ReferralReward,
    FeeCredit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EconomicKind {
    Execution(ExecutionKind),
    Carry(CarryKind),
    Incentive(IncentiveKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicScope {
    Decision {
        decision_correlation_id: DecisionCorrelationId,
    },
    PositionInterval {
        position_id: PositionId,
        starts_at_ns: u64,
        ends_at_ns: u64,
    },
    Action {
        action_id: ActionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceValidity {
    pub source: SourceIdentity,
    pub snapshot_id: SnapshotId,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalculationFactor {
    pub factor_id: FormulaId,
    pub value: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointEstimate {
    NonZero(SignedNativeEffect),
    ProvenZero { factor_id: FormulaId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstimatedEffect {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_reject_empty_padded_and_control_values() {
        assert!(matches!(
            CurrencyId::try_new(""),
            Err(EconomicsError::EmptyIdentifier { .. })
        ));
        assert!(matches!(
            EconomicsInstrumentId::try_new(" instrument "),
            Err(EconomicsError::NonCanonicalIdentifier { .. })
        ));
        assert!(matches!(
            SourceIdentity::try_new("source\n"),
            Err(EconomicsError::NonCanonicalIdentifier { .. })
        ));
    }

    #[test]
    fn signed_native_effect_preserves_unit_and_rejects_zero() {
        let currency = CurrencyId::try_new("USDC").expect("currency should be canonical");
        assert_eq!(
            SignedNativeEffect::currency(Decimal::ZERO, currency.clone()),
            Err(EconomicsError::ZeroNativeEffect)
        );
        let debit = SignedNativeEffect::currency(Decimal::new(-2, 0), currency.clone())
            .expect("non-zero debit should construct");
        assert_eq!(debit.amount(), Decimal::new(-2, 0));
        assert_eq!(debit.unit(), NativeUnitId::Currency(currency));
    }
}
