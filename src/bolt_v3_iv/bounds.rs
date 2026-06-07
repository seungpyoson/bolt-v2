use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::types::IvConvention;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvBoundUnit {
    Unitless,
    Price,
    Rate,
    Carry,
    TimeToExpiry,
    Strike,
    Skew,
    AgreementBand,
    Nanoseconds,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvNumericBounds {
    pub finite_required: bool,
    pub positive_required: bool,
    pub inclusive_min: Option<f64>,
    pub inclusive_max: Option<f64>,
    pub exclusive_min: Option<f64>,
    pub exclusive_max: Option<f64>,
    pub unit: IvBoundUnit,
    pub allowed_conventions: IvConventionBounds,
}

impl IvNumericBounds {
    pub fn accepts(&self, value: f64, convention: &IvConvention) -> bool {
        if self.finite_required && !value.is_finite() {
            return false;
        }
        if self.positive_required && value <= 0.0 {
            return false;
        }
        if self
            .inclusive_min
            .is_some_and(|inclusive_min| value < inclusive_min)
        {
            return false;
        }
        if self
            .inclusive_max
            .is_some_and(|inclusive_max| value > inclusive_max)
        {
            return false;
        }
        if self
            .exclusive_min
            .is_some_and(|exclusive_min| value <= exclusive_min)
        {
            return false;
        }
        if self
            .exclusive_max
            .is_some_and(|exclusive_max| value >= exclusive_max)
        {
            return false;
        }
        if !self.allowed_conventions.allowed_conventions.is_empty()
            && !self
                .allowed_conventions
                .allowed_conventions
                .contains(convention)
        {
            return false;
        }

        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvConventionBounds {
    pub allowed_conventions: BTreeSet<IvConvention>,
}
