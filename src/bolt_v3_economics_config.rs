use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::bolt_v3_numeric::MILLIS_PER_SECOND_U64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EconomicsRootConfig {
    pub reporting: EconomicsReportingConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EconomicsReportingConfig {
    pub policy_id: String,
    pub pnl_currency: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EconomicsSliceConfig {
    QuoteOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEconomicsConfig {
    pub economics_slice: EconomicsSliceConfig,
    pub reporting_policy: String,
    pub quote_refresh_secs: u64,
    pub quote_max_age_secs: u64,
    pub quote_validity_ms: u64,
    pub resting_order_refresh_margin_ms: u64,
    pub edge_basis: BTreeMap<String, EdgeBasisResolverConfig>,
    pub product_surface_policies: BTreeMap<String, String>,
    pub valuation: ValuationConfig,
    pub carry: Option<CarryQuotePolicyConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EdgeBasisResolverConfig {
    pub resolver_id: String,
    pub product_metadata_source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValuationConfig {
    pub routes: BTreeMap<String, ValuationRouteConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValuationRouteConfig {
    pub from_unit: String,
    pub to_currency: String,
    pub valuation_policy: String,
    pub client_id: String,
    pub instrument_id: String,
    pub orientation: ValuationOrientation,
    pub max_age_ms: u64,
    pub legs: Vec<ValuationLegConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValuationLegConfig {
    pub from_unit: String,
    pub to_unit: String,
    pub client_id: String,
    pub instrument_id: String,
    pub orientation: ValuationOrientation,
    pub max_age_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ValuationOrientation {
    BaseToQuote,
    QuoteToBase,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CarryQuotePolicyConfig {
    pub holding_horizon_secs: u64,
    pub risk_policy_id: String,
    pub stress_fixture_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EconomicsConfigField {
    ReportingPolicyId,
    ReportingCurrency,
    ExecutionReportingPolicy,
    ProductSurface,
    EdgeBasisPolicyId,
    EdgeBasisResolverId,
    EdgeBasisMetadataSource,
    CarryRiskPolicyId,
    CarryStressFixtureId,
    ValuationRouteId,
    ValuationFromUnit,
    ValuationDestination,
    ValuationPolicy,
    ValuationClient,
    ValuationInstrument,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EconomicsConfigError {
    InvalidText { field: EconomicsConfigField },
    ReportingPolicyMismatch,
    InvalidQuoteWindow,
    InvalidRefreshMargin,
    EmptyEdgeBasisMapping,
    MissingEdgeBasisPolicy { surface: String, policy_id: String },
    MissingProductSurface { surface: String },
    UnexpectedProductSurface { surface: String },
    ZeroCarryHorizon,
    WrongTerminalCurrency { route_id: String },
    DuplicateValuationAuthority { from: String, to: String },
    InactiveDataClient { route_id: String, client_id: String },
    ZeroValuationAge { route_id: String },
    DisconnectedValuationRoute { route_id: String },
    CyclicValuationRoute { route_id: String },
    LiveSubmissionDisabled,
}

impl EconomicsRootConfig {
    pub fn validate(&self) -> Vec<EconomicsConfigError> {
        let mut errors = Vec::new();
        require_text(
            &self.reporting.policy_id,
            EconomicsConfigField::ReportingPolicyId,
            &mut errors,
        );
        require_text(
            &self.reporting.pnl_currency,
            EconomicsConfigField::ReportingCurrency,
            &mut errors,
        );
        errors
    }
}

impl ExecutionEconomicsConfig {
    pub fn validate(
        &self,
        reporting: &EconomicsReportingConfig,
        active_data_clients: &BTreeSet<String>,
    ) -> Vec<EconomicsConfigError> {
        let mut errors = Vec::new();
        require_text(
            &self.reporting_policy,
            EconomicsConfigField::ExecutionReportingPolicy,
            &mut errors,
        );
        if self.reporting_policy != reporting.policy_id {
            errors.push(EconomicsConfigError::ReportingPolicyMismatch);
        }
        if is_zero(self.quote_refresh_secs)
            || is_zero(self.quote_max_age_secs)
            || self.quote_max_age_secs < self.quote_refresh_secs
        {
            errors.push(EconomicsConfigError::InvalidQuoteWindow);
        }
        let validity_within_max_age =
            match self.quote_max_age_secs.checked_mul(MILLIS_PER_SECOND_U64) {
                Some(max_age_ms) => self.quote_validity_ms <= max_age_ms,
                None => false,
            };
        if is_zero(self.quote_validity_ms) || !validity_within_max_age {
            errors.push(EconomicsConfigError::InvalidQuoteWindow);
        }
        if is_zero(self.resting_order_refresh_margin_ms)
            || self.resting_order_refresh_margin_ms >= self.quote_validity_ms
        {
            errors.push(EconomicsConfigError::InvalidRefreshMargin);
        }
        if self.edge_basis.is_empty() || self.product_surface_policies.is_empty() {
            errors.push(EconomicsConfigError::EmptyEdgeBasisMapping);
        }
        for (surface, policy_id) in &self.product_surface_policies {
            require_text(surface, EconomicsConfigField::ProductSurface, &mut errors);
            if !self.edge_basis.contains_key(policy_id) {
                errors.push(EconomicsConfigError::MissingEdgeBasisPolicy {
                    surface: surface.clone(),
                    policy_id: policy_id.clone(),
                });
            }
        }
        for (policy_id, resolver) in &self.edge_basis {
            require_text(
                policy_id,
                EconomicsConfigField::EdgeBasisPolicyId,
                &mut errors,
            );
            require_text(
                &resolver.resolver_id,
                EconomicsConfigField::EdgeBasisResolverId,
                &mut errors,
            );
            require_text(
                &resolver.product_metadata_source,
                EconomicsConfigField::EdgeBasisMetadataSource,
                &mut errors,
            );
        }
        errors.extend(
            self.valuation
                .validate(&reporting.pnl_currency, active_data_clients),
        );
        if let Some(carry) = &self.carry {
            if is_zero(carry.holding_horizon_secs) {
                errors.push(EconomicsConfigError::ZeroCarryHorizon);
            }
            require_text(
                &carry.risk_policy_id,
                EconomicsConfigField::CarryRiskPolicyId,
                &mut errors,
            );
            require_text(
                &carry.stress_fixture_id,
                EconomicsConfigField::CarryStressFixtureId,
                &mut errors,
            );
        }
        errors
    }

    pub fn validate_product_surfaces<'a>(
        &self,
        configured_surfaces: impl IntoIterator<Item = &'a str>,
    ) -> Vec<EconomicsConfigError> {
        let configured = configured_surfaces.into_iter().collect::<BTreeSet<_>>();
        let declared = self
            .product_surface_policies
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let mut errors = configured
            .difference(&declared)
            .map(|surface| EconomicsConfigError::MissingProductSurface {
                surface: (*surface).to_string(),
            })
            .collect::<Vec<_>>();
        errors.extend(declared.difference(&configured).map(|surface| {
            EconomicsConfigError::UnexpectedProductSurface {
                surface: (*surface).to_string(),
            }
        }));
        errors
    }
}

impl ValuationConfig {
    fn validate(
        &self,
        reporting_currency: &str,
        active_data_clients: &BTreeSet<String>,
    ) -> Vec<EconomicsConfigError> {
        let mut errors = Vec::new();
        let mut authority_pairs = BTreeSet::new();
        for (route_id, route) in &self.routes {
            require_text(
                route_id,
                EconomicsConfigField::ValuationRouteId,
                &mut errors,
            );
            for (value, field) in [
                (&route.from_unit, EconomicsConfigField::ValuationFromUnit),
                (
                    &route.to_currency,
                    EconomicsConfigField::ValuationDestination,
                ),
                (
                    &route.valuation_policy,
                    EconomicsConfigField::ValuationPolicy,
                ),
                (&route.client_id, EconomicsConfigField::ValuationClient),
                (
                    &route.instrument_id,
                    EconomicsConfigField::ValuationInstrument,
                ),
            ] {
                require_text(value, field, &mut errors);
            }
            if route.to_currency != reporting_currency {
                errors.push(EconomicsConfigError::WrongTerminalCurrency {
                    route_id: route_id.clone(),
                });
            }
            if !authority_pairs.insert((route.from_unit.clone(), route.to_currency.clone())) {
                errors.push(EconomicsConfigError::DuplicateValuationAuthority {
                    from: route.from_unit.clone(),
                    to: route.to_currency.clone(),
                });
            }
            if !active_data_clients.contains(&route.client_id) {
                errors.push(EconomicsConfigError::InactiveDataClient {
                    route_id: route_id.clone(),
                    client_id: route.client_id.clone(),
                });
            }
            if is_zero(route.max_age_ms) {
                errors.push(EconomicsConfigError::ZeroValuationAge {
                    route_id: route_id.clone(),
                });
            }
            errors.extend(validate_valuation_legs(
                route_id,
                route,
                active_data_clients,
            ));
        }
        errors
    }
}

fn validate_valuation_legs(
    route_id: &str,
    route: &ValuationRouteConfig,
    active_data_clients: &BTreeSet<String>,
) -> Vec<EconomicsConfigError> {
    let mut errors = Vec::new();
    if route.legs.is_empty() {
        if route.from_unit != route.to_currency {
            errors.push(EconomicsConfigError::DisconnectedValuationRoute {
                route_id: route_id.to_string(),
            });
        }
        return errors;
    }
    let mut current = route.from_unit.as_str();
    let mut visited = BTreeSet::from([current]);
    for leg in &route.legs {
        for (value, field) in [
            (&leg.client_id, EconomicsConfigField::ValuationClient),
            (
                &leg.instrument_id,
                EconomicsConfigField::ValuationInstrument,
            ),
        ] {
            require_text(value, field, &mut errors);
        }
        if !active_data_clients.contains(&leg.client_id) {
            errors.push(EconomicsConfigError::InactiveDataClient {
                route_id: route_id.to_string(),
                client_id: leg.client_id.clone(),
            });
        }
        if leg.from_unit != current {
            errors.push(EconomicsConfigError::DisconnectedValuationRoute {
                route_id: route_id.to_string(),
            });
            return errors;
        }
        if !visited.insert(leg.to_unit.as_str()) {
            errors.push(EconomicsConfigError::CyclicValuationRoute {
                route_id: route_id.to_string(),
            });
            return errors;
        }
        if is_zero(leg.max_age_ms) {
            errors.push(EconomicsConfigError::ZeroValuationAge {
                route_id: route_id.to_string(),
            });
        }
        current = leg.to_unit.as_str();
    }
    if current != route.to_currency {
        errors.push(EconomicsConfigError::DisconnectedValuationRoute {
            route_id: route_id.to_string(),
        });
    }
    errors
}

fn require_text(value: &str, field: EconomicsConfigField, errors: &mut Vec<EconomicsConfigError>) {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        errors.push(EconomicsConfigError::InvalidText { field });
    }
}

fn is_zero(value: u64) -> bool {
    value == u64::default()
}
