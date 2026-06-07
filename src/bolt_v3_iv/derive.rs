use serde::{Deserialize, Serialize};

use super::{
    bounds::IvNumericBounds,
    error::IvRejectReason,
    health::IvSourceHealthState,
    ingest::IvGreekValues,
    provenance::{IvHelperIdentity, IvPolicyDecision, IvProvenance},
    store::IvPoint,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvNtHelperSymbol {
    ImplyVolAndGreeks,
}

impl IvNtHelperSymbol {
    pub fn nt_symbol(self) -> &'static str {
        match self {
            Self::ImplyVolAndGreeks => "nautilus_model::data::imply_vol_and_greeks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvHelperPolicy {
    pub helper_policy_id: String,
    pub nt_helper_symbol: IvNtHelperSymbol,
    pub parameter_signature: String,
    pub output_bounds: IvNumericBounds,
    pub max_input_timestamp_skew_ns: u64,
    pub max_operator_input_age_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvDerivedInputField {
    OptionPrice,
    UnderlyingPrice,
    Strike,
    OptionSide,
    TimeToExpiryYears,
    Rate,
    Carry,
}

impl IvDerivedInputField {
    pub fn required_fields() -> [Self; 7] {
        [
            Self::OptionPrice,
            Self::UnderlyingPrice,
            Self::Strike,
            Self::OptionSide,
            Self::TimeToExpiryYears,
            Self::Rate,
            Self::Carry,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OptionPrice => "option_price",
            Self::UnderlyingPrice => "underlying_price",
            Self::Strike => "strike",
            Self::OptionSide => "option_side",
            Self::TimeToExpiryYears => "time_to_expiry_years",
            Self::Rate => "rate",
            Self::Carry => "carry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvDerivedInputSourceKind {
    QuerySupplied,
    ProfileSourceRef,
    OperatorConfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvOptionSide {
    Call,
    Put,
}

impl IvOptionSide {
    pub fn is_call(self) -> bool {
        self == Self::Call
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IvTimedInput<T> {
    pub value: T,
    pub ts_ns: UnixNanos,
    pub source_kind: IvDerivedInputSourceKind,
    pub expires_at_ns: Option<UnixNanos>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvDerivedInputSet {
    pub profile_id: String,
    pub source_id: String,
    pub source_kind: IvSourceKind,
    pub selector_fingerprint: String,
    pub instrument_id: String,
    pub basis: IvBasis,
    pub convention: IvConvention,
    pub as_of_ns: UnixNanos,
    pub received_ts_ns: UnixNanos,
    pub subscription_generation: u64,
    pub source_health_state: IvSourceHealthState,
    pub nt_revision: String,
    pub nt_evidence_path: String,
    pub input_event_ids: Vec<String>,
    pub option_price: Option<IvTimedInput<f64>>,
    pub underlying_price: Option<IvTimedInput<f64>>,
    pub strike: Option<IvTimedInput<f64>>,
    pub option_side: Option<IvTimedInput<IvOptionSide>>,
    pub time_to_expiry_years: Option<IvTimedInput<f64>>,
    pub rate: Option<IvTimedInput<f64>>,
    pub carry: Option<IvTimedInput<f64>>,
}

impl IvDerivedInputSet {
    pub fn clear_field(&mut self, field: IvDerivedInputField) {
        match field {
            IvDerivedInputField::OptionPrice => self.option_price = None,
            IvDerivedInputField::UnderlyingPrice => self.underlying_price = None,
            IvDerivedInputField::Strike => self.strike = None,
            IvDerivedInputField::OptionSide => self.option_side = None,
            IvDerivedInputField::TimeToExpiryYears => self.time_to_expiry_years = None,
            IvDerivedInputField::Rate => self.rate = None,
            IvDerivedInputField::Carry => self.carry = None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvDerivedOutput {
    pub point: IvPoint,
    pub greeks: IvGreekValues,
    pub helper_identity: IvHelperIdentity,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvDeriveError {
    HelperPolicyNotFound {
        helper_policy_id: String,
    },
    MissingInput {
        field: IvDerivedInputField,
    },
    Rejected {
        reason: IvRejectReason,
        field: String,
    },
}

pub fn select_helper_policy<'a>(
    policies: &'a [IvHelperPolicy],
    helper_policy_id: &str,
) -> Result<&'a IvHelperPolicy, IvDeriveError> {
    policies
        .iter()
        .find(|policy| policy.helper_policy_id == helper_policy_id)
        .ok_or_else(|| IvDeriveError::HelperPolicyNotFound {
            helper_policy_id: helper_policy_id.to_string(),
        })
}

pub fn derive_iv(
    policy: &IvHelperPolicy,
    inputs: IvDerivedInputSet,
) -> Result<IvDerivedOutput, IvDeriveError> {
    let resolved = ResolvedDerivedInputs::resolve(policy, &inputs)?;
    let helper_result = nautilus_model::data::imply_vol_and_greeks(
        resolved.underlying_price,
        resolved.rate,
        resolved.carry,
        resolved.option_side.is_call(),
        resolved.strike,
        resolved.time_to_expiry_years,
        resolved.option_price,
    );

    if !policy
        .output_bounds
        .accepts(helper_result.vol, &inputs.convention)
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidIvValue,
            field: "iv".to_string(),
        });
    }

    let helper_identity = IvHelperIdentity {
        nt_symbol: policy.nt_helper_symbol.nt_symbol().to_string(),
        nt_revision: inputs.nt_revision.clone(),
        parameter_signature: policy.parameter_signature.clone(),
        helper_policy_id: policy.helper_policy_id.clone(),
        engine_mapping: "derived_iv".to_string(),
    };
    let provenance = IvProvenance {
        profile_id: inputs.profile_id.clone(),
        source_id: inputs.source_id.clone(),
        source_kind: inputs.source_kind,
        selector_fingerprint: inputs.selector_fingerprint.clone(),
        nt_revision: inputs.nt_revision.clone(),
        nt_evidence_path: inputs.nt_evidence_path.clone(),
        nt_symbol: policy.nt_helper_symbol.nt_symbol().to_string(),
        raw_event_id: None,
        payload_kind: None,
        input_event_ids: inputs.input_event_ids.clone(),
        helper_identity: Some(helper_identity.clone()),
        policy_decisions: vec![IvPolicyDecision::Helper],
        transformation_steps: Vec::new(),
        ts_event_ns: inputs.as_of_ns,
        ts_init_ns: None,
        received_ts_ns: inputs.received_ts_ns,
        ingest_sequence: 0,
        subscription_generation: inputs.subscription_generation,
        source_health_state: inputs.source_health_state,
        reject_reason: None,
    };
    let point = IvPoint {
        profile_id: inputs.profile_id,
        source_id: inputs.source_id,
        instrument_id: inputs.instrument_id,
        basis: inputs.basis,
        iv: helper_result.vol,
        convention: inputs.convention,
        ts_event_ns: inputs.as_of_ns,
        ts_init_ns: None,
        provenance: provenance.clone(),
    };

    Ok(IvDerivedOutput {
        point,
        greeks: IvGreekValues {
            delta: Some(helper_result.delta),
            gamma: Some(helper_result.gamma),
            vega: Some(helper_result.vega),
            theta: Some(helper_result.theta),
            rho: None,
        },
        helper_identity,
        provenance,
    })
}

struct ResolvedDerivedInputs {
    option_price: f64,
    underlying_price: f64,
    strike: f64,
    option_side: IvOptionSide,
    time_to_expiry_years: f64,
    rate: f64,
    carry: f64,
}

impl ResolvedDerivedInputs {
    fn resolve(policy: &IvHelperPolicy, inputs: &IvDerivedInputSet) -> Result<Self, IvDeriveError> {
        let option_price = required(inputs.option_price, IvDerivedInputField::OptionPrice)?;
        let underlying_price = required(
            inputs.underlying_price,
            IvDerivedInputField::UnderlyingPrice,
        )?;
        let strike = required(inputs.strike, IvDerivedInputField::Strike)?;
        let option_side = required(inputs.option_side, IvDerivedInputField::OptionSide)?;
        let time_to_expiry_years = required(
            inputs.time_to_expiry_years,
            IvDerivedInputField::TimeToExpiryYears,
        )?;
        let rate = required(inputs.rate, IvDerivedInputField::Rate)?;
        let carry = required(inputs.carry, IvDerivedInputField::Carry)?;
        let timed_values = [
            (IvDerivedInputField::OptionPrice, option_price.ts_ns),
            (IvDerivedInputField::UnderlyingPrice, underlying_price.ts_ns),
            (IvDerivedInputField::Strike, strike.ts_ns),
            (IvDerivedInputField::OptionSide, option_side.ts_ns),
            (
                IvDerivedInputField::TimeToExpiryYears,
                time_to_expiry_years.ts_ns,
            ),
            (IvDerivedInputField::Rate, rate.ts_ns),
            (IvDerivedInputField::Carry, carry.ts_ns),
        ];

        validate_numeric(option_price.value, IvDerivedInputField::OptionPrice, true)?;
        validate_numeric(
            underlying_price.value,
            IvDerivedInputField::UnderlyingPrice,
            true,
        )?;
        validate_numeric(strike.value, IvDerivedInputField::Strike, true)?;
        validate_numeric(
            time_to_expiry_years.value,
            IvDerivedInputField::TimeToExpiryYears,
            true,
        )?;
        validate_numeric(rate.value, IvDerivedInputField::Rate, false)?;
        validate_numeric(carry.value, IvDerivedInputField::Carry, false)?;
        validate_timestamp_skew(policy, inputs.as_of_ns, &timed_values)?;
        validate_operator_input(policy, inputs.as_of_ns, IvDerivedInputField::Rate, rate)?;
        validate_operator_input(policy, inputs.as_of_ns, IvDerivedInputField::Carry, carry)?;

        Ok(Self {
            option_price: option_price.value,
            underlying_price: underlying_price.value,
            strike: strike.value,
            option_side: option_side.value,
            time_to_expiry_years: time_to_expiry_years.value,
            rate: rate.value,
            carry: carry.value,
        })
    }
}

fn required<T>(
    input: Option<IvTimedInput<T>>,
    field: IvDerivedInputField,
) -> Result<IvTimedInput<T>, IvDeriveError> {
    input.ok_or(IvDeriveError::MissingInput { field })
}

fn validate_numeric(
    value: f64,
    field: IvDerivedInputField,
    positive_required: bool,
) -> Result<(), IvDeriveError> {
    if !value.is_finite() || (positive_required && value <= 0.0) {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: field.as_str().to_string(),
        });
    }

    Ok(())
}

fn validate_timestamp_skew(
    policy: &IvHelperPolicy,
    as_of_ns: UnixNanos,
    timed_values: &[(IvDerivedInputField, UnixNanos)],
) -> Result<(), IvDeriveError> {
    let min_ts = timed_values
        .iter()
        .map(|(_, ts)| ts.get())
        .min()
        .unwrap_or(as_of_ns.get());
    let max_ts = timed_values
        .iter()
        .map(|(_, ts)| ts.get())
        .max()
        .unwrap_or(as_of_ns.get());

    if max_ts > as_of_ns.get() || max_ts.saturating_sub(min_ts) > policy.max_input_timestamp_skew_ns
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ClockSkew,
            field: "input_timestamp_skew".to_string(),
        });
    }

    Ok(())
}

fn validate_operator_input(
    policy: &IvHelperPolicy,
    as_of_ns: UnixNanos,
    field: IvDerivedInputField,
    input: IvTimedInput<f64>,
) -> Result<(), IvDeriveError> {
    if input.source_kind != IvDerivedInputSourceKind::OperatorConfigured {
        return Ok(());
    }

    if input
        .expires_at_ns
        .is_none_or(|expires_at_ns| expires_at_ns.get() < as_of_ns.get())
        || as_of_ns.get().saturating_sub(input.ts_ns.get()) > policy.max_operator_input_age_ns
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::OperatorInputExpired,
            field: field.as_str().to_string(),
        });
    }

    Ok(())
}
