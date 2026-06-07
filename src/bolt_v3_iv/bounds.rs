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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvConventionBounds {
    pub allowed_conventions: BTreeSet<IvConvention>,
}
