use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    bounds::{IvConventionBounds, IvNumericBounds},
    error::IvRejectReason,
    health::IvSourceHealthState,
    ingest::IvGreekValues,
    provenance::{
        IvHelperEngineMapping, IvHelperIdentity, IvPolicyDecision, IvProvenance,
        validate_iv_provenance,
    },
    store::IvPoint,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

const NT_IMPLY_VOL_FAILURE_FLOOR: f64 = 1.0e-8;
const NT_REFINE_VOL_FAILURE_FLOOR: f64 = 1.0e-6;
const NT_REFINE_VOL_FAILURE_CEILING: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvNtHelperSymbol {
    ImplyVolAndGreeks,
    RefineVolAndGreeks,
}

impl IvNtHelperSymbol {
    pub fn nt_symbol(self) -> &'static str {
        match self {
            Self::ImplyVolAndGreeks => "nautilus_model::data::imply_vol_and_greeks",
            Self::RefineVolAndGreeks => "nautilus_model::data::refine_vol_and_greeks",
        }
    }

    pub fn parameter_signature(self) -> &'static str {
        match self {
            Self::ImplyVolAndGreeks => "s,r,b,is_call,k,t,price",
            Self::RefineVolAndGreeks => "s,r,b,is_call,k,t,target_price,initial_vol",
        }
    }

    pub fn required_fields(self) -> &'static [IvDerivedInputField] {
        match self {
            Self::ImplyVolAndGreeks => &IMPLY_VOL_AND_GREEKS_REQUIRED_FIELDS,
            Self::RefineVolAndGreeks => &REFINE_VOL_AND_GREEKS_REQUIRED_FIELDS,
        }
    }

    pub fn minimum_valid_output_floor(self) -> f64 {
        match self {
            Self::ImplyVolAndGreeks => NT_IMPLY_VOL_FAILURE_FLOOR,
            Self::RefineVolAndGreeks => NT_REFINE_VOL_FAILURE_FLOOR,
        }
    }

    fn is_failure_sentinel(self, vol: f64) -> bool {
        match self {
            Self::ImplyVolAndGreeks => vol <= NT_IMPLY_VOL_FAILURE_FLOOR,
            Self::RefineVolAndGreeks => {
                vol <= NT_REFINE_VOL_FAILURE_FLOOR || vol >= NT_REFINE_VOL_FAILURE_CEILING
            }
        }
    }
}

const IMPLY_VOL_AND_GREEKS_REQUIRED_FIELDS: [IvDerivedInputField; 7] = [
    IvDerivedInputField::OptionPrice,
    IvDerivedInputField::UnderlyingPrice,
    IvDerivedInputField::Strike,
    IvDerivedInputField::OptionSide,
    IvDerivedInputField::TimeToExpiryYears,
    IvDerivedInputField::Rate,
    IvDerivedInputField::Carry,
];

const REFINE_VOL_AND_GREEKS_REQUIRED_FIELDS: [IvDerivedInputField; 8] = [
    IvDerivedInputField::OptionPrice,
    IvDerivedInputField::UnderlyingPrice,
    IvDerivedInputField::Strike,
    IvDerivedInputField::OptionSide,
    IvDerivedInputField::TimeToExpiryYears,
    IvDerivedInputField::Rate,
    IvDerivedInputField::Carry,
    IvDerivedInputField::InitialVol,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvHelperPolicy {
    pub helper_policy_id: String,
    pub nt_helper_symbol: IvNtHelperSymbol,
    pub parameter_signature: String,
    pub allowed_outputs: BTreeSet<IvHelperOutput>,
    pub input_policy_ref: String,
    pub output_bounds: IvNumericBounds,
    pub minimum_valid_iv_output: f64,
    pub convention_policy: IvConventionBounds,
    pub failure_policy: IvHelperFailurePolicy,
    pub max_input_timestamp_skew_ns: u64,
    pub max_operator_input_age_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvHelperOutput {
    Iv,
    Greeks,
    IvAndGreeks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvHelperFailurePolicy {
    RejectInvalidHelperOutput,
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
    InitialVol,
}

impl IvDerivedInputField {
    pub fn required_fields() -> [Self; 7] {
        IMPLY_VOL_AND_GREEKS_REQUIRED_FIELDS
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
            Self::InitialVol => "initial_vol",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvDerivedInputSourceKind {
    QuerySupplied,
    ProfileSourceRef,
    InstrumentMetadata,
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
#[serde(deny_unknown_fields)]
pub struct IvTimedInput<T> {
    pub value: T,
    pub ts_ns: UnixNanos,
    pub source_kind: IvDerivedInputSourceKind,
    pub expires_at_ns: Option<UnixNanos>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvDerivedProfileSourceRef {
    pub source_id: String,
    pub selector_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvDerivedInputFieldPolicy {
    pub field: IvDerivedInputField,
    pub allowed_source_kinds: BTreeSet<IvDerivedInputSourceKind>,
    pub profile_source_ref: Option<IvDerivedProfileSourceRef>,
    pub operator_number: Option<IvTimedInput<f64>>,
    pub operator_side: Option<IvTimedInput<IvOptionSide>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvDerivedInputPolicy {
    pub input_policy_id: String,
    pub helper_policy_ref: String,
    pub required_fields: Vec<IvDerivedInputField>,
    pub field_sources: Vec<IvDerivedInputFieldPolicy>,
    pub freshness_ns: u64,
    pub max_input_skew_ns: u64,
    pub bounds: IvDerivedInputBounds,
    pub convention_policy: IvConventionBounds,
    pub operator_value_refresh_policy: IvOperatorValueRefreshPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvDerivedInputBounds {
    pub option_price: Option<IvNumericBounds>,
    pub underlying_price: Option<IvNumericBounds>,
    pub strike: Option<IvNumericBounds>,
    pub time_to_expiry_years: Option<IvNumericBounds>,
    pub rate: Option<IvNumericBounds>,
    pub carry: Option<IvNumericBounds>,
    pub initial_vol: Option<IvNumericBounds>,
}

impl IvDerivedInputBounds {
    pub fn numeric_bound(&self, field: IvDerivedInputField) -> Option<&IvNumericBounds> {
        match field {
            IvDerivedInputField::OptionPrice => self.option_price.as_ref(),
            IvDerivedInputField::UnderlyingPrice => self.underlying_price.as_ref(),
            IvDerivedInputField::Strike => self.strike.as_ref(),
            IvDerivedInputField::TimeToExpiryYears => self.time_to_expiry_years.as_ref(),
            IvDerivedInputField::Rate => self.rate.as_ref(),
            IvDerivedInputField::Carry => self.carry.as_ref(),
            IvDerivedInputField::InitialVol => self.initial_vol.as_ref(),
            IvDerivedInputField::OptionSide => None,
        }
    }

    pub fn numeric_bounds(&self) -> impl Iterator<Item = (IvDerivedInputField, &IvNumericBounds)> {
        [
            (IvDerivedInputField::OptionPrice, self.option_price.as_ref()),
            (
                IvDerivedInputField::UnderlyingPrice,
                self.underlying_price.as_ref(),
            ),
            (IvDerivedInputField::Strike, self.strike.as_ref()),
            (
                IvDerivedInputField::TimeToExpiryYears,
                self.time_to_expiry_years.as_ref(),
            ),
            (IvDerivedInputField::Rate, self.rate.as_ref()),
            (IvDerivedInputField::Carry, self.carry.as_ref()),
            (IvDerivedInputField::InitialVol, self.initial_vol.as_ref()),
        ]
        .into_iter()
        .filter_map(|(field, bounds)| bounds.map(|bounds| (field, bounds)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvOperatorValueRefreshPolicy {
    RejectExpiredOperatorValues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub initial_vol: Option<IvTimedInput<f64>>,
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
            IvDerivedInputField::InitialVol => self.initial_vol = None,
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
    if policy.parameter_signature.trim() != policy.nt_helper_symbol.parameter_signature() {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: "parameter_signature".to_string(),
        });
    }
    if !policy
        .allowed_outputs
        .contains(&IvHelperOutput::IvAndGreeks)
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: "allowed_outputs".to_string(),
        });
    }
    if !convention_policy_accepts(&policy.convention_policy, &inputs.convention) {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: "convention_policy".to_string(),
        });
    }

    let resolved = ResolvedDerivedInputs::resolve(policy, &inputs)?;
    let helper_result = match policy.nt_helper_symbol {
        IvNtHelperSymbol::ImplyVolAndGreeks => nautilus_model::data::imply_vol_and_greeks(
            resolved.underlying_price,
            resolved.rate,
            resolved.carry,
            resolved.option_side.is_call(),
            resolved.strike,
            resolved.time_to_expiry_years,
            resolved.option_price,
        ),
        IvNtHelperSymbol::RefineVolAndGreeks => nautilus_model::data::refine_vol_and_greeks(
            resolved.underlying_price,
            resolved.rate,
            resolved.carry,
            resolved.option_side.is_call(),
            resolved.strike,
            resolved.time_to_expiry_years,
            resolved.option_price,
            resolved.initial_vol.ok_or(IvDeriveError::MissingInput {
                field: IvDerivedInputField::InitialVol,
            })?,
        ),
    };

    if policy
        .nt_helper_symbol
        .is_failure_sentinel(helper_result.vol)
        || helper_result.vol <= policy.minimum_valid_iv_output
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidIvValue,
            field: "iv".to_string(),
        });
    }
    if !policy
        .output_bounds
        .accepts(helper_result.vol, &inputs.convention)
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidIvValue,
            field: "iv".to_string(),
        });
    }
    let greeks = IvGreekValues {
        delta: Some(helper_result.delta),
        gamma: Some(helper_result.gamma),
        vega: Some(helper_result.vega),
        theta: Some(helper_result.theta),
        rho: None,
    };
    if greeks.has_non_finite_value() {
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
        engine_mapping: IvHelperEngineMapping::IvDerivedHelper,
    };
    let input_event_ids = inputs.input_event_ids.clone();
    let input_set_id = format!(
        "{}:{}:{}:{}",
        inputs.profile_id,
        inputs.source_id,
        inputs.instrument_id,
        inputs.as_of_ns.get()
    );
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
        input_event_ids: input_event_ids.clone(),
        helper_identity: Some(helper_identity.clone()),
        policy_decisions: vec![IvPolicyDecision::HelperDecision {
            helper_policy_id: policy.helper_policy_id.clone(),
            helper_identity: helper_identity.clone(),
            helper_symbol: policy.nt_helper_symbol.nt_symbol().to_string(),
            input_set_id,
            input_event_ids,
            output_validated: true,
            rejection_reason: None,
        }],
        transformation_steps: Vec::new(),
        ts_event_ns: inputs.as_of_ns,
        ts_init_ns: None,
        received_ts_ns: inputs.received_ts_ns,
        ingest_sequence: inputs.received_ts_ns.get(),
        subscription_generation: inputs.subscription_generation,
        source_health_state: inputs.source_health_state,
        reject_reason: None,
    };
    validate_iv_provenance(&provenance).map_err(|_| IvDeriveError::Rejected {
        reason: IvRejectReason::ProvenanceIncomplete,
        field: "provenance".to_string(),
    })?;
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
        greeks,
        helper_identity,
        provenance,
    })
}

pub fn resolve_derived_input_policy(
    policy: &IvDerivedInputPolicy,
    mut request: IvDerivedInputSet,
    profile_inputs: &[IvDerivedInputSet],
) -> Result<IvDerivedInputSet, IvDeriveError> {
    for field in &policy.required_fields {
        let Some(field_policy) = policy
            .field_sources
            .iter()
            .find(|field_policy| field_policy.field == *field)
        else {
            return Err(IvDeriveError::MissingInput { field: *field });
        };
        match *field {
            IvDerivedInputField::OptionPrice
            | IvDerivedInputField::UnderlyingPrice
            | IvDerivedInputField::Strike
            | IvDerivedInputField::TimeToExpiryYears
            | IvDerivedInputField::Rate
            | IvDerivedInputField::Carry
            | IvDerivedInputField::InitialVol => {
                resolve_number_field(*field, Some(field_policy), &mut request, profile_inputs)?;
            }
            IvDerivedInputField::OptionSide => {
                resolve_side_field(Some(field_policy), &mut request, profile_inputs)?;
            }
        }
    }

    validate_resolved_input_policy(policy, &request)?;

    Ok(request)
}

fn validate_resolved_input_policy(
    policy: &IvDerivedInputPolicy,
    inputs: &IvDerivedInputSet,
) -> Result<(), IvDeriveError> {
    let timed_inputs = policy
        .required_fields
        .iter()
        .filter_map(|field| timed_input_metadata(inputs, *field).map(|metadata| (*field, metadata)))
        .collect::<Vec<_>>();
    if timed_inputs.len() != policy.required_fields.len() {
        let missing = policy
            .required_fields
            .iter()
            .find(|field| timed_input_ns(inputs, **field).is_none())
            .copied()
            .unwrap_or(IvDerivedInputField::OptionPrice);
        return Err(IvDeriveError::MissingInput { field: missing });
    }

    if timed_inputs
        .iter()
        .any(|(_, (_, ts))| ts.get() > inputs.as_of_ns.get())
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ClockSkew,
            field: "max_input_skew_ns".to_string(),
        });
    }

    let market_timed_inputs = timed_inputs
        .iter()
        .filter(|(_, (source_kind, _))| {
            *source_kind != IvDerivedInputSourceKind::OperatorConfigured
        })
        .collect::<Vec<_>>();
    let min_ts = market_timed_inputs
        .iter()
        .map(|(_, (_, ts))| ts.get())
        .min()
        .unwrap_or(inputs.as_of_ns.get());
    let max_ts = market_timed_inputs
        .iter()
        .map(|(_, (_, ts))| ts.get())
        .max()
        .unwrap_or(inputs.as_of_ns.get());
    if max_ts.saturating_sub(min_ts) > policy.max_input_skew_ns {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ClockSkew,
            field: "max_input_skew_ns".to_string(),
        });
    }
    if market_timed_inputs
        .iter()
        .any(|(_, (_, ts))| inputs.as_of_ns.get().saturating_sub(ts.get()) > policy.freshness_ns)
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::StaleData,
            field: "freshness".to_string(),
        });
    }
    if !convention_policy_accepts(&policy.convention_policy, &inputs.convention) {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: "convention_policy".to_string(),
        });
    }
    for field in policy.required_fields.iter().copied() {
        validate_input_bounds(policy, inputs, field)?;
        validate_operator_refresh_policy(policy, inputs, field)?;
    }

    Ok(())
}

fn convention_policy_accepts(policy: &IvConventionBounds, convention: &IvConvention) -> bool {
    !policy.allowed_conventions.is_empty() && policy.allowed_conventions.contains(convention)
}

fn validate_input_bounds(
    policy: &IvDerivedInputPolicy,
    inputs: &IvDerivedInputSet,
    field: IvDerivedInputField,
) -> Result<(), IvDeriveError> {
    let Some(bound) = policy.bounds.numeric_bound(field) else {
        return Ok(());
    };
    let Some(value) = number_field(inputs, field).map(|input| input.value) else {
        return Ok(());
    };
    if !bound.accepts(value, &inputs.convention) {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: field.as_str().to_string(),
        });
    }

    Ok(())
}

fn validate_operator_refresh_policy(
    policy: &IvDerivedInputPolicy,
    inputs: &IvDerivedInputSet,
    field: IvDerivedInputField,
) -> Result<(), IvDeriveError> {
    let Some((source_kind, expires_at_ns)) = timed_input_refresh_metadata(inputs, field) else {
        return Ok(());
    };
    if source_kind != IvDerivedInputSourceKind::OperatorConfigured {
        return Ok(());
    }
    match policy.operator_value_refresh_policy {
        IvOperatorValueRefreshPolicy::RejectExpiredOperatorValues
            if expires_at_ns
                .is_some_and(|expires_at_ns| expires_at_ns.get() < inputs.as_of_ns.get()) =>
        {
            Err(IvDeriveError::Rejected {
                reason: IvRejectReason::OperatorInputExpired,
                field: field.as_str().to_string(),
            })
        }
        _ => Ok(()),
    }
}

fn timed_input_refresh_metadata(
    inputs: &IvDerivedInputSet,
    field: IvDerivedInputField,
) -> Option<(IvDerivedInputSourceKind, Option<UnixNanos>)> {
    match field {
        IvDerivedInputField::OptionPrice
        | IvDerivedInputField::UnderlyingPrice
        | IvDerivedInputField::Strike
        | IvDerivedInputField::TimeToExpiryYears
        | IvDerivedInputField::Rate
        | IvDerivedInputField::Carry
        | IvDerivedInputField::InitialVol => {
            number_field(inputs, field).map(|input| (input.source_kind, input.expires_at_ns))
        }
        IvDerivedInputField::OptionSide => inputs
            .option_side
            .map(|input| (input.source_kind, input.expires_at_ns)),
    }
}

fn resolve_number_field(
    field: IvDerivedInputField,
    field_policy: Option<&IvDerivedInputFieldPolicy>,
    request: &mut IvDerivedInputSet,
    profile_inputs: &[IvDerivedInputSet],
) -> Result<(), IvDeriveError> {
    if let Some(input) = number_field(request, field) {
        validate_allowed_source_kind(field_policy, field, input.source_kind)?;
        return Ok(());
    }

    let Some(field_policy) = field_policy else {
        return Err(IvDeriveError::MissingInput { field });
    };

    if let Some(input) = field_policy.operator_number {
        validate_allowed_source_kind(Some(field_policy), field, input.source_kind)?;
        set_number_field(request, field, input);
        return Ok(());
    }

    if let Some(source_ref) = &field_policy.profile_source_ref
        && let Some((input, event_ids)) =
            profile_number_field(profile_inputs, request, source_ref, field)
    {
        validate_allowed_source_kind(Some(field_policy), field, input.source_kind)?;
        set_number_field(request, field, input);
        merge_input_event_ids(&mut request.input_event_ids, event_ids);
        return Ok(());
    }

    if field_policy
        .allowed_source_kinds
        .contains(&IvDerivedInputSourceKind::InstrumentMetadata)
        && let Some((input, event_ids)) =
            instrument_metadata_number_field(profile_inputs, request, field)
    {
        validate_allowed_source_kind(Some(field_policy), field, input.source_kind)?;
        set_number_field(request, field, input);
        merge_input_event_ids(&mut request.input_event_ids, event_ids);
        return Ok(());
    }

    Err(IvDeriveError::MissingInput { field })
}

fn resolve_side_field(
    field_policy: Option<&IvDerivedInputFieldPolicy>,
    request: &mut IvDerivedInputSet,
    profile_inputs: &[IvDerivedInputSet],
) -> Result<(), IvDeriveError> {
    let field = IvDerivedInputField::OptionSide;
    if let Some(input) = request.option_side {
        validate_allowed_source_kind(field_policy, field, input.source_kind)?;
        return Ok(());
    }

    let Some(field_policy) = field_policy else {
        return Err(IvDeriveError::MissingInput { field });
    };

    if let Some(input) = field_policy.operator_side {
        validate_allowed_source_kind(Some(field_policy), field, input.source_kind)?;
        request.option_side = Some(input);
        return Ok(());
    }

    if let Some(source_ref) = &field_policy.profile_source_ref
        && let Some((input, event_ids)) = profile_side_field(profile_inputs, request, source_ref)
    {
        validate_allowed_source_kind(Some(field_policy), field, input.source_kind)?;
        request.option_side = Some(input);
        merge_input_event_ids(&mut request.input_event_ids, event_ids);
        return Ok(());
    }

    if field_policy
        .allowed_source_kinds
        .contains(&IvDerivedInputSourceKind::InstrumentMetadata)
        && let Some((input, event_ids)) = instrument_metadata_side_field(profile_inputs, request)
    {
        validate_allowed_source_kind(Some(field_policy), field, input.source_kind)?;
        request.option_side = Some(input);
        merge_input_event_ids(&mut request.input_event_ids, event_ids);
        return Ok(());
    }

    Err(IvDeriveError::MissingInput { field })
}

fn number_field(
    inputs: &IvDerivedInputSet,
    field: IvDerivedInputField,
) -> Option<IvTimedInput<f64>> {
    match field {
        IvDerivedInputField::OptionPrice => inputs.option_price,
        IvDerivedInputField::UnderlyingPrice => inputs.underlying_price,
        IvDerivedInputField::Strike => inputs.strike,
        IvDerivedInputField::TimeToExpiryYears => inputs.time_to_expiry_years,
        IvDerivedInputField::Rate => inputs.rate,
        IvDerivedInputField::Carry => inputs.carry,
        IvDerivedInputField::InitialVol => inputs.initial_vol,
        IvDerivedInputField::OptionSide => None,
    }
}

fn set_number_field(
    inputs: &mut IvDerivedInputSet,
    field: IvDerivedInputField,
    input: IvTimedInput<f64>,
) {
    match field {
        IvDerivedInputField::OptionPrice => inputs.option_price = Some(input),
        IvDerivedInputField::UnderlyingPrice => inputs.underlying_price = Some(input),
        IvDerivedInputField::Strike => inputs.strike = Some(input),
        IvDerivedInputField::TimeToExpiryYears => inputs.time_to_expiry_years = Some(input),
        IvDerivedInputField::Rate => inputs.rate = Some(input),
        IvDerivedInputField::Carry => inputs.carry = Some(input),
        IvDerivedInputField::InitialVol => inputs.initial_vol = Some(input),
        IvDerivedInputField::OptionSide => {}
    }
}

fn timed_input_ns(inputs: &IvDerivedInputSet, field: IvDerivedInputField) -> Option<UnixNanos> {
    timed_input_metadata(inputs, field).map(|(_, ts_ns)| ts_ns)
}

fn timed_input_metadata(
    inputs: &IvDerivedInputSet,
    field: IvDerivedInputField,
) -> Option<(IvDerivedInputSourceKind, UnixNanos)> {
    match field {
        IvDerivedInputField::OptionPrice
        | IvDerivedInputField::UnderlyingPrice
        | IvDerivedInputField::Strike
        | IvDerivedInputField::TimeToExpiryYears
        | IvDerivedInputField::Rate
        | IvDerivedInputField::Carry
        | IvDerivedInputField::InitialVol => {
            number_field(inputs, field).map(|input| (input.source_kind, input.ts_ns))
        }
        IvDerivedInputField::OptionSide => inputs
            .option_side
            .map(|input| (input.source_kind, input.ts_ns)),
    }
}

fn profile_number_field(
    profile_inputs: &[IvDerivedInputSet],
    request: &IvDerivedInputSet,
    source_ref: &IvDerivedProfileSourceRef,
    field: IvDerivedInputField,
) -> Option<(IvTimedInput<f64>, Vec<String>)> {
    profile_inputs
        .iter()
        .filter_map(|candidate| {
            if profile_source_matches(candidate, request, source_ref) {
                number_field(candidate, field).map(|input| (candidate, input))
            } else {
                None
            }
        })
        .max_by_key(|(candidate, _)| candidate.as_of_ns.get())
        .map(|(candidate, input)| (input, candidate.input_event_ids.clone()))
}

fn profile_side_field(
    profile_inputs: &[IvDerivedInputSet],
    request: &IvDerivedInputSet,
    source_ref: &IvDerivedProfileSourceRef,
) -> Option<(IvTimedInput<IvOptionSide>, Vec<String>)> {
    profile_inputs
        .iter()
        .filter_map(|candidate| {
            if profile_source_matches(candidate, request, source_ref) {
                candidate.option_side.map(|input| (candidate, input))
            } else {
                None
            }
        })
        .max_by_key(|(candidate, _)| candidate.as_of_ns.get())
        .map(|(candidate, input)| (input, candidate.input_event_ids.clone()))
}

fn instrument_metadata_number_field(
    profile_inputs: &[IvDerivedInputSet],
    request: &IvDerivedInputSet,
    field: IvDerivedInputField,
) -> Option<(IvTimedInput<f64>, Vec<String>)> {
    profile_inputs
        .iter()
        .filter_map(|candidate| {
            if instrument_metadata_matches(candidate, request) {
                number_field(candidate, field)
                    .filter(|input| {
                        input.source_kind == IvDerivedInputSourceKind::InstrumentMetadata
                    })
                    .map(|input| (candidate, input))
            } else {
                None
            }
        })
        .max_by_key(|(candidate, _)| candidate.as_of_ns.get())
        .map(|(candidate, input)| (input, candidate.input_event_ids.clone()))
}

fn instrument_metadata_side_field(
    profile_inputs: &[IvDerivedInputSet],
    request: &IvDerivedInputSet,
) -> Option<(IvTimedInput<IvOptionSide>, Vec<String>)> {
    profile_inputs
        .iter()
        .filter_map(|candidate| {
            if instrument_metadata_matches(candidate, request) {
                candidate
                    .option_side
                    .filter(|input| {
                        input.source_kind == IvDerivedInputSourceKind::InstrumentMetadata
                    })
                    .map(|input| (candidate, input))
            } else {
                None
            }
        })
        .max_by_key(|(candidate, _)| candidate.as_of_ns.get())
        .map(|(candidate, input)| (input, candidate.input_event_ids.clone()))
}

fn profile_source_matches(
    candidate: &IvDerivedInputSet,
    request: &IvDerivedInputSet,
    source_ref: &IvDerivedProfileSourceRef,
) -> bool {
    candidate.profile_id == request.profile_id
        && candidate.source_id == source_ref.source_id
        && candidate.selector_fingerprint == source_ref.selector_fingerprint
        && candidate.instrument_id == request.instrument_id
        && candidate.as_of_ns.get() <= request.as_of_ns.get()
}

fn instrument_metadata_matches(candidate: &IvDerivedInputSet, request: &IvDerivedInputSet) -> bool {
    candidate.profile_id == request.profile_id
        && candidate.instrument_id == request.instrument_id
        && candidate.as_of_ns.get() <= request.as_of_ns.get()
}

fn validate_allowed_source_kind(
    field_policy: Option<&IvDerivedInputFieldPolicy>,
    field: IvDerivedInputField,
    source_kind: IvDerivedInputSourceKind,
) -> Result<(), IvDeriveError> {
    if field_policy.is_some_and(|field_policy| {
        !field_policy.allowed_source_kinds.is_empty()
            && !field_policy.allowed_source_kinds.contains(&source_kind)
    }) {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::InvalidDerivedInput,
            field: field.as_str().to_string(),
        });
    }

    Ok(())
}

fn merge_input_event_ids(target: &mut Vec<String>, event_ids: Vec<String>) {
    for event_id in event_ids {
        if !target.contains(&event_id) {
            target.push(event_id);
        }
    }
}

struct ResolvedDerivedInputs {
    option_price: f64,
    underlying_price: f64,
    strike: f64,
    option_side: IvOptionSide,
    time_to_expiry_years: f64,
    rate: f64,
    carry: f64,
    initial_vol: Option<f64>,
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
        let initial_vol = match policy.nt_helper_symbol {
            IvNtHelperSymbol::ImplyVolAndGreeks => None,
            IvNtHelperSymbol::RefineVolAndGreeks => Some(required(
                inputs.initial_vol,
                IvDerivedInputField::InitialVol,
            )?),
        };
        let mut timed_values = vec![
            (
                IvDerivedInputField::OptionPrice,
                option_price.source_kind,
                option_price.ts_ns,
            ),
            (
                IvDerivedInputField::UnderlyingPrice,
                underlying_price.source_kind,
                underlying_price.ts_ns,
            ),
            (
                IvDerivedInputField::Strike,
                strike.source_kind,
                strike.ts_ns,
            ),
            (
                IvDerivedInputField::OptionSide,
                option_side.source_kind,
                option_side.ts_ns,
            ),
            (
                IvDerivedInputField::TimeToExpiryYears,
                time_to_expiry_years.source_kind,
                time_to_expiry_years.ts_ns,
            ),
            (IvDerivedInputField::Rate, rate.source_kind, rate.ts_ns),
            (IvDerivedInputField::Carry, carry.source_kind, carry.ts_ns),
        ];
        if let Some(initial_vol) = initial_vol {
            timed_values.push((
                IvDerivedInputField::InitialVol,
                initial_vol.source_kind,
                initial_vol.ts_ns,
            ));
        }

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
        if let Some(initial_vol) = initial_vol {
            validate_numeric(initial_vol.value, IvDerivedInputField::InitialVol, true)?;
        }
        validate_timestamp_skew(policy, inputs.as_of_ns, &timed_values)?;
        validate_operator_input(
            policy,
            inputs.as_of_ns,
            IvDerivedInputField::OptionPrice,
            &option_price,
        )?;
        validate_operator_input(
            policy,
            inputs.as_of_ns,
            IvDerivedInputField::UnderlyingPrice,
            &underlying_price,
        )?;
        validate_operator_input(
            policy,
            inputs.as_of_ns,
            IvDerivedInputField::Strike,
            &strike,
        )?;
        validate_operator_input(
            policy,
            inputs.as_of_ns,
            IvDerivedInputField::OptionSide,
            &option_side,
        )?;
        validate_operator_input(
            policy,
            inputs.as_of_ns,
            IvDerivedInputField::TimeToExpiryYears,
            &time_to_expiry_years,
        )?;
        validate_operator_input(policy, inputs.as_of_ns, IvDerivedInputField::Rate, &rate)?;
        validate_operator_input(policy, inputs.as_of_ns, IvDerivedInputField::Carry, &carry)?;
        if let Some(initial_vol) = initial_vol {
            validate_operator_input(
                policy,
                inputs.as_of_ns,
                IvDerivedInputField::InitialVol,
                &initial_vol,
            )?;
        }

        Ok(Self {
            option_price: option_price.value,
            underlying_price: underlying_price.value,
            strike: strike.value,
            option_side: option_side.value,
            time_to_expiry_years: time_to_expiry_years.value,
            rate: rate.value,
            carry: carry.value,
            initial_vol: initial_vol.map(|initial_vol| initial_vol.value),
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
    timed_values: &[(IvDerivedInputField, IvDerivedInputSourceKind, UnixNanos)],
) -> Result<(), IvDeriveError> {
    if timed_values
        .iter()
        .any(|(_, _, ts)| ts.get() > as_of_ns.get())
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ClockSkew,
            field: "input_timestamp_skew".to_string(),
        });
    }

    let market_timed_values = timed_values
        .iter()
        .filter(|(_, source_kind, _)| *source_kind != IvDerivedInputSourceKind::OperatorConfigured)
        .collect::<Vec<_>>();
    let min_ts = market_timed_values
        .iter()
        .map(|(_, _, ts)| ts.get())
        .min()
        .unwrap_or(as_of_ns.get());
    let max_ts = market_timed_values
        .iter()
        .map(|(_, _, ts)| ts.get())
        .max()
        .unwrap_or(as_of_ns.get());

    if max_ts.saturating_sub(min_ts) > policy.max_input_timestamp_skew_ns {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::ClockSkew,
            field: "input_timestamp_skew".to_string(),
        });
    }

    Ok(())
}

fn validate_operator_input<T>(
    policy: &IvHelperPolicy,
    as_of_ns: UnixNanos,
    field: IvDerivedInputField,
    input: &IvTimedInput<T>,
) -> Result<(), IvDeriveError> {
    if input.source_kind != IvDerivedInputSourceKind::OperatorConfigured {
        return Ok(());
    }

    if input
        .expires_at_ns
        .is_some_and(|expires_at_ns| expires_at_ns.get() < as_of_ns.get())
        || as_of_ns.get().saturating_sub(input.ts_ns.get()) > policy.max_operator_input_age_ns
    {
        return Err(IvDeriveError::Rejected {
            reason: IvRejectReason::OperatorInputExpired,
            field: field.as_str().to_string(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct ExpectedFailureSentinel {
        floor: f64,
        ceiling: Option<f64>,
    }

    fn expected_failure_sentinel(symbol: IvNtHelperSymbol) -> ExpectedFailureSentinel {
        match symbol {
            IvNtHelperSymbol::ImplyVolAndGreeks => ExpectedFailureSentinel {
                floor: 1.0e-8,
                ceiling: None,
            },
            IvNtHelperSymbol::RefineVolAndGreeks => ExpectedFailureSentinel {
                floor: 1.0e-6,
                ceiling: Some(10.0),
            },
        }
    }

    #[test]
    fn nt_helper_failure_sentinels_are_closed_protocol_constants() {
        for symbol in [
            IvNtHelperSymbol::ImplyVolAndGreeks,
            IvNtHelperSymbol::RefineVolAndGreeks,
        ] {
            let expected = expected_failure_sentinel(symbol);

            assert_eq!(symbol.minimum_valid_output_floor(), expected.floor);
            assert!(symbol.is_failure_sentinel(expected.floor));
            assert!(symbol.is_failure_sentinel(expected.floor / 2.0));
            assert!(!symbol.is_failure_sentinel(expected.floor * 2.0));

            match expected.ceiling {
                Some(ceiling) => {
                    assert!(symbol.is_failure_sentinel(ceiling));
                    assert!(symbol.is_failure_sentinel(ceiling * 2.0));
                    assert!(!symbol.is_failure_sentinel(ceiling / 2.0));
                }
                None => assert!(!symbol.is_failure_sentinel(10.0)),
            }
        }
    }
}
