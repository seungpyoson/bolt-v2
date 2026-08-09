//! Per-provider binding root for bolt-v3 client config block shapes
//! and per-client startup-validation policy.
//!
//! Core config in `crate::bolt_v3_config` owns the root and strategy
//! envelopes plus NT venue identifiers. Concrete NT venue literals and
//! `[clients.<name>.{data,execution,secrets}]` block shapes live in
//! per-provider binding modules under this root.
//!
//! This module also owns the family-agnostic dispatch surface that
//! core startup validation in `crate::bolt_v3_validate` calls into:
//! every `[clients.<id>]` block is routed here, the NT venue is read
//! once, and the matching per-provider
//! validator owns the rest of the structural venue-shape rules.
//! Provider-neutral helpers used by more than one provider validator
//! (today: `crate::bolt_v3_validate::validate_ssm_parameter_path`)
//! stay in core and are called from the per-provider modules.

pub mod binance;
pub mod boundary_registry;
pub mod chainlink;
pub mod chainlink_reference;
pub mod hyperliquid;
pub mod hyperliquid_artifacts;
pub mod market_data;
pub mod polymarket;
pub mod polyresearch;
pub mod reference_boundary_capture;
pub mod reference_live_probe;

// Neutral resolution-oracle seam. Core config resolution
// (`crate::bolt_v3_config`), core validation (`crate::bolt_v3_validate`), the
// binary-oracle archetype, and the binary-oracle strategy reach the live
// Chainlink Data Streams strike provider through these provider-agnostic
// re-exports and delegators, so no core module names the concrete provider
// module path, provider type, or provider-key literal.
pub use chainlink::KEY as RESOLUTION_ORACLE_VENUE_KEY;
pub use chainlink::PROVIDER_KIND as RESOLUTION_ORACLE_PROVIDER_KIND;
pub(crate) use chainlink::{
    SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM, STRIKE_FETCH_INSTRUMENT_ID_PARAM,
    STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM,
    strike_fetch_request_data_type as resolution_strike_fetch_request_data_type,
};
pub use chainlink_reference::KEY as REFERENCE_CATALOG_VENUE_KEY;
pub use hyperliquid::KEY as OUTCOME_GROUP_HIP4_VENUE_KEY;
pub use polymarket::KEY as OUTCOME_GROUP_POLYMARKET_VENUE_KEY;

use std::{any::Any, collections::BTreeMap, fmt, path::Path, sync::Arc};

use nautilus_model::{
    enums::TimeInForce,
    identifiers::{InstrumentId, Venue},
    instruments::{Instrument, InstrumentAny},
};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterConfigs, BoltV3AdapterMappingError, BoltV3ClientAdapterConfig,
        BoltV3MarketClockFn,
    },
    bolt_v3_config::{BoltV3RootConfig, ClientBlock, ExecutionEconomicsConfig, LoadedBoltV3Config},
    bolt_v3_market_families::MarketIdentityPlan,
    bolt_v3_operator_artifacts::{BoltV3OperatorArtifactError, WrittenOperatorArtifact},
    bolt_v3_operator_health::{BoltV3InputHealthTransitionEmitter, BoltV3MissingInputSource},
    bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceSnapshotSource,
    bolt_v3_secrets::{BoltV3SecretError, ResolvedBoltV3Secrets},
    economics::VenueEconomicsAdapter,
};

pub trait ProviderResolvedSecrets: fmt::Debug + Send + Sync {
    fn provider_key(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    /// Required (no default): each provider's resolved-secrets type MUST declare the
    /// secret strings the post-run residue scan redacts. Removing the default makes a
    /// missing override a COMPILE error, so a new provider can't silently contribute
    /// zero redaction values (F10).
    fn redaction_values(&self) -> Vec<&str>;

    fn exclusive_signer_owner(&self) -> Option<ProviderExclusiveSignerOwner> {
        None
    }
}

pub type ResolvedClientSecrets = Arc<dyn ProviderResolvedSecrets>;

pub(crate) fn attach_live_input_health_transition_emitters(
    adapters: &mut BoltV3AdapterConfigs,
    input_health_transition_emitter: BoltV3InputHealthTransitionEmitter,
    input_health_sources_by_client: &BTreeMap<String, Vec<BoltV3MissingInputSource>>,
) {
    chainlink_reference::attach_live_input_health_transition_emitter(
        adapters,
        input_health_transition_emitter,
        input_health_sources_by_client,
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExclusiveSignerOwner {
    pub provider_key: &'static str,
    pub fingerprint: String,
}

pub trait SsmSecretResolver {
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String>;
}

impl<F, E> SsmSecretResolver for F
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: fmt::Display,
{
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String> {
        self(region, ssm_path).map_err(|error| error.to_string())
    }
}

pub struct ProviderSecretResolveContext<'a> {
    pub client_key: &'a str,
    pub region: &'a str,
    pub client: &'a ClientBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSsmPathReference {
    pub field_name: &'static str,
    pub ssm_path: String,
}

pub struct ProviderAdapterMapContext<'a> {
    pub root: &'a BoltV3RootConfig,
    pub client_key: &'a str,
    pub client: &'a ClientBlock,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub plan: &'a MarketIdentityPlan,
    pub clock: BoltV3MarketClockFn,
    pub runtime_approvals: ProviderRuntimeApprovals<'a>,
}

pub struct ProviderCollateralAllowanceSourceContext<'a> {
    pub client_key: &'a str,
    pub client: &'a ClientBlock,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub collateral_currency: &'a str,
}

#[derive(Clone)]
pub struct ProviderCollateralAllowanceRuntimeSource {
    pub source: Arc<dyn ProviderCollateralAllowanceSnapshotSource>,
    pub poll_interval_ms: u64,
}

#[derive(Clone, Copy)]
pub struct ProviderLiveSubmitApprovalContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub client_key: &'a str,
    pub client: &'a ClientBlock,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub product_surface: Option<&'a str>,
    pub now_unix_seconds: u64,
    pub build_head_sha: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderArtifactReference<'a> {
    pub artifact_path: &'a str,
    pub artifact_sha256: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderProductSubmitProofArtifactRequest<'a> {
    pub provider_id: &'a str,
    pub product_surface: &'a str,
    pub toml_checksum: &'a str,
    pub order_proof: ProviderArtifactReference<'a>,
    pub fill_proof: ProviderArtifactReference<'a>,
    pub rounding_proof: ProviderArtifactReference<'a>,
    pub fee_proof: ProviderArtifactReference<'a>,
    pub settlement_proof: Option<ProviderArtifactReference<'a>>,
    pub output_path: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLiveSubmitOrderLimits {
    pub max_order_count: u32,
    pub max_order_notional: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderLiveSubmitArmingPreflight {
    pub provider_key: &'static str,
    pub client_key: String,
    pub product_surface: String,
    pub approval_artifact_path: String,
    pub product_submit_proof_artifact_path: String,
    pub max_order_count: u32,
    pub max_order_notional: String,
}

pub struct ProviderLiveSubmitApproval {
    payload: Box<dyn Any>,
    order_limits: Option<ProviderLiveSubmitOrderLimits>,
}

impl ProviderLiveSubmitApproval {
    pub fn new(payload: Box<dyn Any>) -> Self {
        Self {
            payload,
            order_limits: None,
        }
    }

    pub fn with_order_limits(
        payload: Box<dyn Any>,
        order_limits: ProviderLiveSubmitOrderLimits,
    ) -> Self {
        Self {
            payload,
            order_limits: Some(order_limits),
        }
    }

    pub fn order_limits(&self) -> Option<&ProviderLiveSubmitOrderLimits> {
        self.order_limits.as_ref()
    }

    fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.payload.downcast_ref()
    }
}

pub struct ProviderLiveSubmitApprovals {
    by_client: BTreeMap<String, ProviderLiveSubmitApproval>,
}

impl ProviderLiveSubmitApprovals {
    pub fn empty() -> Self {
        Self {
            by_client: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, client_key: String, approval: ProviderLiveSubmitApproval) {
        self.by_client.insert(client_key, approval);
    }

    pub fn is_empty(&self) -> bool {
        self.by_client.is_empty()
    }

    pub fn get_as<T: 'static>(&self, client_key: &str) -> Option<&T> {
        self.by_client
            .get(client_key)
            .and_then(ProviderLiveSubmitApproval::downcast_ref)
    }

    pub fn order_limits(&self) -> impl Iterator<Item = (&String, &ProviderLiveSubmitOrderLimits)> {
        self.by_client.iter().filter_map(|(client_key, approval)| {
            approval
                .order_limits()
                .map(|order_limits| (client_key, order_limits))
        })
    }
}

#[derive(Clone, Copy)]
pub struct ProviderRuntimeApprovals<'a> {
    pub live_submit: Option<&'a dyn Any>,
}

impl<'a> ProviderRuntimeApprovals<'a> {
    pub const fn none() -> Self {
        Self { live_submit: None }
    }
}

pub trait FeeProvider: Send + Sync {
    fn fee_bps(&self, instrument_id: InstrumentId) -> Option<Decimal>;
    fn entry_fee_bps(&self, instrument: &InstrumentAny, _entry_price: Decimal) -> Option<Decimal> {
        self.fee_bps(instrument.id())
    }
    fn max_entry_fee_bps(
        &self,
        instrument: &InstrumentAny,
        entry_price: Decimal,
    ) -> Option<Decimal> {
        self.entry_fee_bps(instrument, entry_price)
    }
    fn warm(
        &self,
        instrument_id: InstrumentId,
    ) -> futures_util::future::BoxFuture<'_, anyhow::Result<()>>;
}

type FeeProviderBuilder = fn(
    &str,
    &ClientBlock,
    &ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, BoltV3AdapterMappingError>;

type LiveSubmitApprovalLoader =
    for<'a> fn(
        ProviderLiveSubmitApprovalContext<'a>,
    ) -> Result<Option<ProviderLiveSubmitApproval>, anyhow::Error>;

type LiveSubmitArmingPreflight =
    for<'a> fn(
        ProviderLiveSubmitApprovalContext<'a>,
    ) -> Result<Option<ProviderLiveSubmitArmingPreflight>, anyhow::Error>;

type LiveSubmitApprovalArtifactWriter =
    for<'a> fn(
        ProviderLiveSubmitApprovalContext<'a>,
        u64,
    ) -> Result<WrittenOperatorArtifact, anyhow::Error>;

type ProductSubmitProofArtifactWriter =
    for<'a> fn(
        ProviderProductSubmitProofArtifactRequest<'a>,
    ) -> Result<WrittenOperatorArtifact, anyhow::Error>;

type MetadataRefreshIntervalLoader = fn(&ClientBlock) -> Result<Option<u64>, String>;
type ProviderCollateralAllowanceRuntimeSourceBuilder =
    for<'a> fn(
        ProviderCollateralAllowanceSourceContext<'a>,
    ) -> Result<ProviderCollateralAllowanceRuntimeSource, anyhow::Error>;

// PROVIDER-SPECIFIC (Polymarket CLOB v2). Every `ClobV2*` type and
// `*_clob_v2_*` below materializes Polymarket CLOB v2 signing, fee, and collateral
// evidence from NT `nautilus_polymarket` sources. Despite the neutral `ClobV2`
// prefix, these declarations are provider-owned and are not venue-neutral extension points.
#[derive(Clone, Copy)]
pub struct ClobV2AdapterSigningSourceMaterializationRequest<'a> {
    pub schema_version: u32,
    pub domain_requirements_record_kind: &'static str,
    pub signed_order_fixture_record_kind: &'static str,
    pub signature_verification_record_kind: &'static str,
    pub clob_signing_version: &'a str,
    pub clob_signing_source_sha256: &'a str,
    pub clob_signing_source: &'a str,
}

pub struct ClobV2AdapterSigningSourceMaterialization {
    pub domain_requirements_sha256: String,
    pub signed_order_fixture_sha256: String,
    pub signature_verification_sha256: String,
    pub signer_recovered_matches_expected: bool,
}

#[derive(Clone, Copy)]
pub struct ClobV2FeeBehaviorSourceMaterializationRequest<'a> {
    pub schema_version: u32,
    pub nt_execution_parse_source: &'a str,
    pub nt_http_parse_source: &'a str,
}

pub struct ClobV2FeeBehaviorSourceMaterialization {
    pub maker_zero_fee_verified: bool,
    pub taker_fee_schedule_verified: bool,
    pub market_buy_fee_adjustment_verified: bool,
    pub price: String,
    pub fee_rate: String,
    pub fee_behavior_source_sha256: String,
    pub fee_assumptions_sha256: String,
}

pub struct ClobV2BalanceAllowanceCacheSyncRequest<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy_instance_id: &'a str,
    pub resolved: &'a ResolvedBoltV3Secrets,
}

pub struct ClobV2BalanceAllowanceCacheSync {
    pub execution_client_id: String,
    pub request_path: &'static str,
    pub base_url_http_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialedBlock {
    Data,
    Execution,
}

impl ProviderCredentialedBlock {
    fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Execution => "execution",
        }
    }

    fn is_present(self, client: &ClientBlock) -> bool {
        match self {
            Self::Data => client.data.is_some(),
            Self::Execution => client.execution.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSecretRequirement {
    pub block: ProviderCredentialedBlock,
    pub consumer: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMarketExitOrderConstraints {
    pub allowed_market_time_in_forces: Option<&'static [TimeInForce]>,
    pub reduce_only_supported: bool,
}

pub const DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS: ProviderMarketExitOrderConstraints =
    ProviderMarketExitOrderConstraints {
        allowed_market_time_in_forces: None,
        reduce_only_supported: true,
    };

const IMMEDIATE_ONLY_MARKET_EXIT_ORDER_CONSTRAINTS: ProviderMarketExitOrderConstraints =
    ProviderMarketExitOrderConstraints {
        allowed_market_time_in_forces: Some(&[TimeInForce::Ioc, TimeInForce::Fok]),
        reduce_only_supported: false,
    };

pub(crate) type NtReconnectBudgetLoader = fn(&toml::Value) -> Result<u64, toml::de::Error>;

#[derive(Clone, Copy)]
pub(crate) enum NtReconnectBudgetCapability {
    NotApplicable,
    Required(NtReconnectBudgetLoader),
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderExecutionEconomicsBinding {
    pub load_config: fn(&toml::Value) -> Result<Option<ExecutionEconomicsConfig>, String>,
    pub build_adapter: for<'a> fn(
        ProviderEconomicsAdapterBuildContext<'a>,
    ) -> Result<BuiltProviderEconomicsAdapter, String>,
}

pub(crate) struct BuiltProviderEconomicsAdapter {
    pub account_id: String,
    pub adapter: Arc<dyn VenueEconomicsAdapter>,
}

pub(crate) struct ProviderEconomicsAdapterBuildContext<'a> {
    pub execution: &'a toml::Value,
    pub config: &'a ExecutionEconomicsConfig,
    pub product_surface_id: &'a str,
    pub authority: &'a (dyn Any + Send + Sync),
}

pub struct ProviderBinding {
    pub key: &'static str,
    pub(crate) nt_reconnect_budget: NtReconnectBudgetCapability,
    pub validate_client: fn(&str, &ClientBlock) -> Vec<String>,
    pub supported_market_families: &'static [&'static str],
    pub market_exit_order_constraints: ProviderMarketExitOrderConstraints,
    pub metadata_refresh_interval_mins: Option<MetadataRefreshIntervalLoader>,
    pub required_secret_blocks: &'static [ProviderSecretRequirement],
    pub secret_field_names: &'static [&'static str],
    pub credential_log_modules: &'static [&'static str],
    pub forbidden_env_vars: &'static [&'static str],
    pub resolve_secrets: for<'a> fn(
        ProviderSecretResolveContext<'a>,
        &mut dyn SsmSecretResolver,
    ) -> Result<ResolvedClientSecrets, BoltV3SecretError>,
    pub configured_secret_paths:
        for<'a> fn(
            ProviderSecretResolveContext<'a>,
        ) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError>,
    pub map_adapters: for<'a> fn(
        ProviderAdapterMapContext<'a>,
    )
        -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError>,
    pub load_live_submit_approval: Option<LiveSubmitApprovalLoader>,
    pub preflight_live_submit_arming: Option<LiveSubmitArmingPreflight>,
    pub write_live_submit_approval_artifact: Option<LiveSubmitApprovalArtifactWriter>,
    pub write_product_submit_proof_artifact: Option<ProductSubmitProofArtifactWriter>,
    pub(crate) execution_economics: Option<ProviderExecutionEconomicsBinding>,
    pub build_fee_provider: Option<FeeProviderBuilder>,
    pub build_provider_collateral_allowance_runtime_source:
        Option<ProviderCollateralAllowanceRuntimeSourceBuilder>,
    /// Conditions that must be closed before enforced capital admission can
    /// trust this provider's reconciled order state. Empty means every known
    /// condition is closed; a non-empty list blocks
    /// `enforce_submit_admission = true` and each entry is reported verbatim.
    ///
    /// These are properties of the pinned NautilusTrader adapter, not of Bolt
    /// policy, so emptying this list asserts a verdict about one NT revision and
    /// must be re-established when the pin moves.
    pub reconciliation_unmet: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferencePriceIdentifierKind {
    InstrumentId,
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferencePriceProviderMetadata {
    pub provider_key: &'static str,
    pub client_venue_key: &'static str,
    pub identifier_kind: ReferencePriceIdentifierKind,
    pub supported_assets: &'static [&'static str],
    pub emits_live_input_health: bool,
}

pub const REFERENCE_PRICE_PROVIDER_METADATA: &[ReferencePriceProviderMetadata] = &[
    ReferencePriceProviderMetadata {
        provider_key: chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY,
        client_venue_key: chainlink_reference::KEY,
        identifier_kind: ReferencePriceIdentifierKind::InstrumentId,
        supported_assets: &[],
        emits_live_input_health: true,
    },
    ReferencePriceProviderMetadata {
        provider_key: polyresearch::REFERENCE_PRICE_PROVIDER_KEY,
        client_venue_key: polyresearch::KEY,
        identifier_kind: ReferencePriceIdentifierKind::Symbol,
        supported_assets: &[],
        emits_live_input_health: false,
    },
];

pub const fn reference_price_provider_metadata_entries() -> &'static [ReferencePriceProviderMetadata]
{
    REFERENCE_PRICE_PROVIDER_METADATA
}

pub fn reference_price_provider_metadata(
    provider_key: &str,
) -> Option<ReferencePriceProviderMetadata> {
    reference_price_provider_metadata_entries()
        .iter()
        .copied()
        .find(|metadata| metadata.provider_key == provider_key)
}

pub fn reference_price_provider_supports_asset(provider_key: &str, asset: &str) -> bool {
    let Some(metadata) = reference_price_provider_metadata(provider_key) else {
        return false;
    };
    metadata.supported_assets.is_empty() || metadata.supported_assets.contains(&asset)
}

pub fn reference_price_provider_emits_live_input_health(provider_key: &str) -> bool {
    reference_price_provider_metadata(provider_key)
        .is_some_and(|metadata| metadata.emits_live_input_health)
}

pub fn reference_price_provider_identifier_is_configured(
    root: &BoltV3RootConfig,
    provider_key: &str,
    identifier: &str,
) -> Result<bool, String> {
    if provider_key == chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY {
        return chainlink::reference_price_instrument_in_shared_catalog(root, identifier);
    }
    if provider_key == polyresearch::REFERENCE_PRICE_PROVIDER_KEY {
        return Ok(true);
    }
    Err(format!(
        "reference price provider `{provider_key}` is unsupported"
    ))
}

pub(crate) fn validate_reference_live_probe_block(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(probe) = root.reference_live_probe.as_ref() else {
        return errors;
    };
    if probe.duration_secs == 0 {
        errors.push("reference_live_probe.duration_secs must be positive".to_string());
    }
    if probe.min_chainlink_data_frames == 0 {
        errors.push("reference_live_probe.min_chainlink_data_frames must be positive".to_string());
    }
    validate_reference_live_probe_client(
        root,
        "reference_live_probe.chainlink_client_id",
        probe.chainlink_client_id.as_str(),
        chainlink_reference::KEY,
        &mut errors,
    );
    validate_reference_live_probe_client(
        root,
        "reference_live_probe.polyresearch_client_id",
        probe.polyresearch_client_id.as_str(),
        polyresearch::KEY,
        &mut errors,
    );
    errors
}

fn validate_reference_live_probe_client(
    root: &BoltV3RootConfig,
    field: &str,
    client_key: &str,
    expected_venue: &str,
    errors: &mut Vec<String>,
) {
    if client_key.trim().is_empty() || client_key.trim() != client_key {
        errors.push(format!(
            "{field} must be non-empty without surrounding whitespace"
        ));
        return;
    }
    let Some(client) = root.clients.get(client_key) else {
        errors.push(format!(
            "{field} `{client_key}` must reference a configured client"
        ));
        return;
    };
    if client.venue.as_str() != expected_venue {
        errors.push(format!(
            "{field} `{client_key}` must reference provider `{expected_venue}`, got `{}`",
            client.venue.as_str()
        ));
    }
    if client.data.is_none() {
        errors.push(format!(
            "{field} `{client_key}` must reference a client with [data]"
        ));
    }
    if client.secrets.is_none() {
        errors.push(format!(
            "{field} `{client_key}` must reference a client with [secrets]"
        ));
    }
}

const PROVIDER_BINDINGS: &[ProviderBinding] = &[
    ProviderBinding {
        key: polymarket::KEY,
        reconciliation_unmet: &[
            "the pinned Polymarket adapter omits venue orders and positions from the mass status \
it returns -- those it cannot represent, and holdings it filters as dust -- and still reports \
success, with no count of either omission reaching that report, so a mass-status report can be \
silently partial",
            "the pinned Polymarket adapter caps order filled quantity against confirmed \
fills with a zero local-filled floor, so a matched-but-unconfirmed trade understates filled \
quantity and overstates remaining quantity",
            "the pinned Polymarket adapter discards non-confirmed trades with no fill report, no \
counter and no log entry, so a settlement-pending fill's quantity, price and fee never reach the \
projection as a trade. Bolt runs the engine with no cache database, so at startup every venue \
order is unknown to it and its report is projected as an external order rather than reconciled \
against a cached one. That projection recovers nothing today only because the cap above has \
already driven the report's filled quantity to zero, and no fill is inferred from a zero \
quantity, so the quantity stays understated. The zero is what prevents it, not the terminal \
status: the same path given a non-zero filled quantity infers a fill identified by the order's \
own fields rather than by the venue trade id the adapter never supplied, which the venue's later \
confirmation of that same trade does not match and is applied on top of. Closing the cap \
condition on its own therefore converts a permanent understatement into a double count, on the \
route where that confirmation is reconciled by itself: applied on top of the inferred fill, with \
no order status report in the same pass to measure the result against, it leaves twice the \
executed quantity standing. A same-pass batch reconciliation is not that route and does not \
double count by quantity -- it applies the fill to a working copy and then projects the order \
snapshot against the report, which still carries the smaller quantity, so the LIFO void removes \
the venue-confirmed fill and leaves the inferred fill standing. A fix supplying the status report \
and its fill reports together therefore restores quantity but still loses the venue trade id, \
venue price and venue fee",
            "the pinned Polymarket adapter builds no fill report for a confirmed maker trade \
holding none of the account's own maker orders, incrementing no counter and logging nothing, so a \
confirmed fill can be missing from a mass status that reports success; ownership is decided by \
exact string comparison against either the configured account address or the configured API key, \
so a funder configured in the checksummed form block explorers display fails the address test for \
every one of the account's own orders, leaving the API-key comparison as the only thing \
establishing ownership",
            "the pinned NautilusTrader execution manager drops a venue position report whose \
instrument is absent from its cache: the netting path returns `None` and the caller discards it \
with no else branch, no counter and no log, so a mass status the adapter built completely can \
still lose positions inside the engine",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: polymarket::validate_client,
        supported_market_families: polymarket::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: IMMEDIATE_ONLY_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: Some(polymarket::metadata_refresh_interval_mins),
        required_secret_blocks: polymarket::REQUIRED_SECRET_BLOCKS,
        secret_field_names: polymarket::SECRET_FIELD_NAMES,
        credential_log_modules: polymarket::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: polymarket::FORBIDDEN_ENV_VARS,
        resolve_secrets: polymarket::resolve_secrets,
        configured_secret_paths: polymarket::configured_secret_paths,
        map_adapters: polymarket::map_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: Some(ProviderExecutionEconomicsBinding {
            load_config: polymarket::execution_economics_config,
            build_adapter: polymarket::build_execution_economics_adapter,
        }),
        build_fee_provider: Some(polymarket::build_fee_provider),
        build_provider_collateral_allowance_runtime_source: Some(
            polymarket::build_provider_collateral_allowance_runtime_source,
        ),
    },
    ProviderBinding {
        key: binance::KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: binance::validate_client,
        supported_market_families: binance::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: binance::REQUIRED_SECRET_BLOCKS,
        secret_field_names: binance::SECRET_FIELD_NAMES,
        credential_log_modules: binance::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: binance::FORBIDDEN_ENV_VARS,
        resolve_secrets: binance::resolve_secrets,
        configured_secret_paths: binance::configured_secret_paths,
        map_adapters: binance::map_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: hyperliquid::KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: hyperliquid::validate_client,
        supported_market_families: hyperliquid::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: Some(hyperliquid::metadata_refresh_interval_mins),
        required_secret_blocks: hyperliquid::REQUIRED_SECRET_BLOCKS,
        secret_field_names: hyperliquid::SECRET_FIELD_NAMES,
        credential_log_modules: hyperliquid::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: hyperliquid::FORBIDDEN_ENV_VARS,
        resolve_secrets: hyperliquid::resolve_secrets,
        configured_secret_paths: hyperliquid::configured_secret_paths,
        map_adapters: hyperliquid::map_adapters,
        load_live_submit_approval: Some(hyperliquid::load_live_submit_approval),
        preflight_live_submit_arming: Some(hyperliquid::preflight_live_submit_arming),
        write_live_submit_approval_artifact: Some(
            hyperliquid::write_configured_live_submit_approval_artifact,
        ),
        write_product_submit_proof_artifact: Some(hyperliquid::write_product_submit_proof_artifact),
        execution_economics: Some(ProviderExecutionEconomicsBinding {
            load_config: hyperliquid::execution_economics_config,
            build_adapter: hyperliquid::build_execution_economics_adapter,
        }),
        build_fee_provider: Some(hyperliquid::build_fee_provider),
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: market_data::BITMEX_KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: market_data::validate_bitmex_client,
        supported_market_families: market_data::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: market_data::NO_REQUIRED_SECRET_BLOCKS,
        secret_field_names: market_data::NO_SECRET_FIELD_NAMES,
        credential_log_modules: market_data::BITMEX_CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: market_data::BITMEX_FORBIDDEN_ENV_VARS,
        resolve_secrets: market_data::resolve_unsupported_secrets,
        configured_secret_paths: market_data::configured_secret_paths,
        map_adapters: market_data::map_bitmex_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: market_data::BYBIT_KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: market_data::validate_bybit_client,
        supported_market_families: market_data::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: market_data::NO_REQUIRED_SECRET_BLOCKS,
        secret_field_names: market_data::NO_SECRET_FIELD_NAMES,
        credential_log_modules: market_data::BYBIT_CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: market_data::BYBIT_FORBIDDEN_ENV_VARS,
        resolve_secrets: market_data::resolve_unsupported_secrets,
        configured_secret_paths: market_data::configured_secret_paths,
        map_adapters: market_data::map_bybit_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: market_data::COINBASE_KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: market_data::validate_coinbase_client,
        supported_market_families: market_data::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: market_data::NO_REQUIRED_SECRET_BLOCKS,
        secret_field_names: market_data::NO_SECRET_FIELD_NAMES,
        credential_log_modules: market_data::COINBASE_CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: market_data::COINBASE_FORBIDDEN_ENV_VARS,
        resolve_secrets: market_data::resolve_unsupported_secrets,
        configured_secret_paths: market_data::configured_secret_paths,
        map_adapters: market_data::map_coinbase_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: market_data::DERIBIT_KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: market_data::validate_deribit_client,
        supported_market_families: market_data::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: market_data::NO_REQUIRED_SECRET_BLOCKS,
        secret_field_names: market_data::NO_SECRET_FIELD_NAMES,
        credential_log_modules: market_data::DERIBIT_CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: market_data::DERIBIT_FORBIDDEN_ENV_VARS,
        resolve_secrets: market_data::resolve_unsupported_secrets,
        configured_secret_paths: market_data::configured_secret_paths,
        map_adapters: market_data::map_deribit_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: market_data::OKX_KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: market_data::validate_okx_client,
        supported_market_families: market_data::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: market_data::NO_REQUIRED_SECRET_BLOCKS,
        secret_field_names: market_data::NO_SECRET_FIELD_NAMES,
        credential_log_modules: market_data::OKX_CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: market_data::OKX_FORBIDDEN_ENV_VARS,
        resolve_secrets: market_data::resolve_unsupported_secrets,
        configured_secret_paths: market_data::configured_secret_paths,
        map_adapters: market_data::map_okx_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: market_data::KRAKEN_KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: market_data::validate_kraken_client,
        supported_market_families: market_data::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: market_data::NO_REQUIRED_SECRET_BLOCKS,
        secret_field_names: market_data::NO_SECRET_FIELD_NAMES,
        credential_log_modules: market_data::KRAKEN_CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: market_data::KRAKEN_FORBIDDEN_ENV_VARS,
        resolve_secrets: market_data::resolve_unsupported_secrets,
        configured_secret_paths: market_data::configured_secret_paths,
        map_adapters: market_data::map_kraken_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: chainlink::KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: chainlink::validate_client,
        supported_market_families: chainlink::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: chainlink::REQUIRED_SECRET_BLOCKS,
        secret_field_names: chainlink::SECRET_FIELD_NAMES,
        credential_log_modules: chainlink::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: chainlink::FORBIDDEN_ENV_VARS,
        resolve_secrets: chainlink::resolve_secrets,
        configured_secret_paths: chainlink::configured_secret_paths,
        map_adapters: chainlink::map_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: chainlink_reference::KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::Required(
            chainlink_reference::reconnect_timeout_ms_for_nt_connect_budget,
        ),
        validate_client: chainlink_reference::validate_client,
        supported_market_families: chainlink_reference::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: chainlink_reference::REQUIRED_SECRET_BLOCKS,
        secret_field_names: chainlink_reference::SECRET_FIELD_NAMES,
        credential_log_modules: chainlink_reference::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: chainlink_reference::FORBIDDEN_ENV_VARS,
        resolve_secrets: chainlink_reference::resolve_secrets,
        configured_secret_paths: chainlink_reference::configured_secret_paths,
        map_adapters: chainlink_reference::map_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
    ProviderBinding {
        key: polyresearch::KEY,
        reconciliation_unmet: &[
            "no submit-admission reconciliation path is verified for this provider",
        ],
        nt_reconnect_budget: NtReconnectBudgetCapability::Required(
            polyresearch::reconnect_timeout_ms_for_nt_connect_budget,
        ),
        validate_client: polyresearch::validate_client,
        supported_market_families: polyresearch::SUPPORTED_MARKET_FAMILIES,
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: polyresearch::REQUIRED_SECRET_BLOCKS,
        secret_field_names: polyresearch::SECRET_FIELD_NAMES,
        credential_log_modules: polyresearch::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: polyresearch::FORBIDDEN_ENV_VARS,
        resolve_secrets: polyresearch::resolve_secrets,
        configured_secret_paths: polyresearch::configured_secret_paths,
        map_adapters: polyresearch::map_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        execution_economics: None,
        build_fee_provider: None,
        build_provider_collateral_allowance_runtime_source: None,
    },
];

pub fn provider_bindings() -> &'static [ProviderBinding] {
    PROVIDER_BINDINGS
}

pub fn binding_for_provider_key(key: &str) -> Option<&'static ProviderBinding> {
    provider_bindings()
        .iter()
        .find(|binding| binding.key == key)
}

pub fn metadata_refresh_interval_mins(client: &ClientBlock) -> Result<Option<u64>, String> {
    let Some(binding) = binding_for_provider_key(client.venue.as_str()) else {
        return Ok(None);
    };
    let Some(loader) = binding.metadata_refresh_interval_mins else {
        return Ok(None);
    };
    loader(client)
}

/// A configured trading venue's modeled REST-egress capabilities. The per-minute
/// request `cap_per_minute` and the per-order-command request fanout
/// `max_rest_requests_per_order_command` share the venue's lifecycle, so they are
/// looked up together (group-by-change). The fanout derates the command-rate
/// ceiling: an NT order command can issue more than one REST request, so a submit
/// rate at the raw cap would over-drive the venue's request quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueEgressModel {
    pub cap_per_minute: u32,
    pub max_rest_requests_per_order_command: u32,
}

/// REST-egress model for a configured trading venue, looked up by NT venue key
/// from the owning provider module so core validation stays provider-agnostic.
/// Returns `None` for venues whose egress bolt-v3 does not model; the complete
/// multi-venue total-REST-budget contract (retries + cancels + status + probes,
/// across per-client buckets) is tracked in #501.
pub fn venue_egress_model(venue: &str) -> Option<VenueEgressModel> {
    match venue {
        polymarket::KEY => Some(VenueEgressModel {
            cap_per_minute: polymarket::REST_EGRESS_CAP_PER_MINUTE,
            max_rest_requests_per_order_command: polymarket::MAX_REST_REQUESTS_PER_ORDER_COMMAND,
        }),
        hyperliquid::KEY => Some(VenueEgressModel {
            cap_per_minute: hyperliquid::REST_EGRESS_CAP_PER_MINUTE,
            max_rest_requests_per_order_command: hyperliquid::MAX_REST_REQUESTS_PER_ORDER_COMMAND,
        }),
        _ => None,
    }
}

pub fn normalize_base_order_quantity_for_execution_venue(
    execution_venue: Venue,
    quantity: Decimal,
) -> Option<Decimal> {
    if quantity <= Decimal::ZERO {
        return None;
    }
    if execution_venue.as_str() == polymarket::KEY {
        return polymarket::normalize_base_order_quantity(quantity);
    }
    Some(quantity)
}

pub fn market_quote_buy_min_notional_for_execution_venue(
    execution_venue: Venue,
) -> Option<Decimal> {
    (execution_venue.as_str() == polymarket::KEY)
        .then_some(polymarket::MARKET_QUOTE_BUY_MIN_NOTIONAL)
}

pub fn materialize_clob_v2_adapter_signing_source_from_nt_signing_source(
    request: ClobV2AdapterSigningSourceMaterializationRequest<'_>,
) -> Result<ClobV2AdapterSigningSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_clob_v2_adapter_signing_source_from_nt_signing_source(request)
}

pub fn materialize_clob_v2_fee_behavior_source_from_nt_fee_sources(
    request: ClobV2FeeBehaviorSourceMaterializationRequest<'_>,
) -> Result<ClobV2FeeBehaviorSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_clob_v2_fee_behavior_source_from_nt_fee_sources(request)
}

pub async fn sync_clob_v2_balance_allowance_cache_from_configured_account(
    request: ClobV2BalanceAllowanceCacheSyncRequest<'_>,
) -> Result<ClobV2BalanceAllowanceCacheSync, BoltV3OperatorArtifactError> {
    polymarket::sync_clob_v2_balance_allowance_cache_from_configured_account(request).await
}

/// Provider-owned NT adapter modules whose info logs can expose
/// credential metadata. The live-node builder consumes this provider
/// binding surface to install `WARN` module filters without hardcoding
/// concrete provider module paths in the live-node assembly layer.
pub fn credential_log_modules() -> impl Iterator<Item = &'static str> {
    provider_bindings()
        .iter()
        .flat_map(|binding| binding.credential_log_modules.iter().copied())
}

/// Provider-neutral seam read by core startup validation: cross-checks any
/// configured live resolution-oracle strike client against the matching gate
/// provider so the live strike path and the offline-evidence path cannot drift
/// onto different endpoints/credentials. Delegates to the owning provider
/// binding, which deserializes the concrete client config block shape.
pub fn validate_resolution_oracle_client_consistency(root: &BoltV3RootConfig) -> Vec<String> {
    chainlink::validate_client_gate_provider_consistency(root)
}

pub(crate) fn resolution_oracle_client_http_timeout_secs(
    root: &BoltV3RootConfig,
    client_key: &str,
) -> Result<Option<u64>, String> {
    chainlink::resolution_oracle_client_http_timeout_secs(root, client_key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NtReconnectBudget {
    NotApplicable,
    Required {
        provider_key: &'static str,
        reconnect_timeout_ms: u64,
    },
}

#[derive(Debug)]
pub(crate) enum NtReconnectBudgetResolutionError {
    UnsupportedProvider {
        provider_key: String,
    },
    MissingData {
        provider_key: &'static str,
    },
    InvalidData {
        provider_key: &'static str,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for NtReconnectBudgetResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProvider { provider_key } => write!(
                f,
                "error_variant=NtReconnectBudgetUnsupportedProvider provider `{provider_key}` is not registered"
            ),
            Self::MissingData { provider_key } => write!(
                f,
                "error_variant=NtReconnectBudgetMissingData provider `{provider_key}` requires clients data to validate its NT reconnect budget"
            ),
            Self::InvalidData {
                provider_key,
                source,
            } => write!(
                f,
                "error_variant=NtReconnectBudgetInvalidData provider `{provider_key}` has invalid typed config for NT reconnect-budget validation: {source}"
            ),
        }
    }
}

impl std::error::Error for NtReconnectBudgetResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidData { source, .. } => Some(source),
            Self::UnsupportedProvider { .. } | Self::MissingData { .. } => None,
        }
    }
}

pub(crate) fn nt_reconnect_budget(
    provider_key: &str,
    data: Option<&toml::Value>,
) -> Result<NtReconnectBudget, NtReconnectBudgetResolutionError> {
    let binding = binding_for_provider_key(provider_key).ok_or_else(|| {
        NtReconnectBudgetResolutionError::UnsupportedProvider {
            provider_key: provider_key.to_string(),
        }
    })?;
    match binding.nt_reconnect_budget {
        NtReconnectBudgetCapability::NotApplicable => Ok(NtReconnectBudget::NotApplicable),
        NtReconnectBudgetCapability::Required(load_reconnect_timeout_ms) => {
            let data = data.ok_or(NtReconnectBudgetResolutionError::MissingData {
                provider_key: binding.key,
            })?;
            let reconnect_timeout_ms = load_reconnect_timeout_ms(data).map_err(|source| {
                NtReconnectBudgetResolutionError::InvalidData {
                    provider_key: binding.key,
                    source,
                }
            })?;
            Ok(NtReconnectBudget::Required {
                provider_key: binding.key,
                reconnect_timeout_ms,
            })
        }
    }
}

/// Family-agnostic surface read by core startup validation. Routes
/// each client block to its per-provider validator based on provider
/// key. Returns the full error list for the client block.
pub fn validate_client_block(key: &str, client: &ClientBlock) -> Vec<String> {
    match binding_for_provider_key(client.venue.as_str()) {
        Some(binding) => {
            let mut errors = validate_required_secret_blocks(
                key,
                binding.key,
                client,
                binding.required_secret_blocks,
            );
            errors.extend((binding.validate_client)(key, client));
            errors
        }
        None => vec![format!(
            "clients.{key}.venue `{}` is not supported by this build",
            client.venue.as_str()
        )],
    }
}

#[derive(Debug)]
pub enum FeeProviderResolutionError {
    UnsupportedProvider {
        client_key: String,
        provider_key: String,
    },
    ProviderWithoutFeeBinding {
        client_key: String,
        provider_key: String,
    },
    ProviderBuild {
        client_key: String,
        provider_key: String,
        source: BoltV3AdapterMappingError,
    },
}

impl std::fmt::Display for FeeProviderResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProvider {
                client_key,
                provider_key,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` has no registered provider binding",
            ),
            Self::ProviderWithoutFeeBinding {
                client_key,
                provider_key,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` has no registered fee-provider binding",
            ),
            Self::ProviderBuild {
                client_key,
                provider_key,
                source,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` fee-provider construction failed: {source}",
            ),
        }
    }
}

impl std::error::Error for FeeProviderResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProviderBuild { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn resolve_fee_provider(
    execution_client_id: &str,
    client: &ClientBlock,
    execution_venue: Venue,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, FeeProviderResolutionError> {
    let provider_key = execution_venue.as_str();
    let binding = binding_for_provider_key(provider_key).ok_or_else(|| {
        FeeProviderResolutionError::UnsupportedProvider {
            client_key: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let build_fee_provider = binding.build_fee_provider.ok_or_else(|| {
        FeeProviderResolutionError::ProviderWithoutFeeBinding {
            client_key: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    build_fee_provider(execution_client_id, client, resolved).map_err(|source| {
        FeeProviderResolutionError::ProviderBuild {
            client_key: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
            source,
        }
    })
}

fn validate_required_secret_blocks(
    key: &str,
    provider_key: &str,
    client: &ClientBlock,
    requirements: &[ProviderSecretRequirement],
) -> Vec<String> {
    let mut errors = Vec::new();
    if client.secrets.is_some() {
        return errors;
    }
    for requirement in requirements {
        if requirement.block.is_present(client) {
            errors.push(format!(
                "clients.{key} (provider={provider_key}) declares [{}] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every {}",
                requirement.block.as_str(),
                requirement.consumer
            ));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_config::LoadedBoltV3Config,
        bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, resolve_bolt_v3_secrets_with},
    };
    use std::{collections::BTreeMap, path::PathBuf};

    fn client_from_toml(text: &str) -> ClientBlock {
        toml::from_str(text).expect("test client should parse")
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: "test-config-bundle-checksum".to_string(),
            root: toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root should parse"),
            strategies: Vec::new(),
        }
    }

    fn binance_reference_client() -> ClientBlock {
        client_from_toml(include_str!(
            "../../tests/fixtures/bolt_v3/binance_reference_client.toml"
        ))
    }

    fn decimal(value: &str) -> Decimal {
        Decimal::from_str_exact(value).expect("test decimal should parse")
    }

    #[test]
    fn polymarket_base_quantity_normalizer_truncates_to_provider_direct_amount_scale() {
        let normalized = normalize_base_order_quantity_for_execution_venue(
            Venue::from(polymarket::KEY),
            decimal("2.641"),
        )
        .expect("positive Polymarket quantity should normalize");

        assert_eq!(normalized, decimal("2.64"));
    }

    #[test]
    fn polymarket_base_quantity_normalizer_fails_closed_on_zero_underflow() {
        assert_eq!(
            normalize_base_order_quantity_for_execution_venue(
                Venue::from(polymarket::KEY),
                decimal("0.001"),
            ),
            None
        );
    }

    #[test]
    fn non_polymarket_base_quantity_normalizer_preserves_quantity() {
        let quantity = decimal("2.641");

        assert_eq!(
            normalize_base_order_quantity_for_execution_venue(Venue::from("OKX"), quantity),
            Some(quantity)
        );
    }

    #[test]
    fn reference_price_live_input_health_capability_is_metadata_driven() {
        let chainlink_metadata =
            reference_price_provider_metadata(chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY)
                .expect("Chainlink provider metadata should be registered");
        let polyresearch_metadata =
            reference_price_provider_metadata(polyresearch::REFERENCE_PRICE_PROVIDER_KEY)
                .expect("PolyResearch provider metadata should be registered");

        assert!(chainlink_metadata.emits_live_input_health);
        assert!(reference_price_provider_emits_live_input_health(
            chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY
        ));
        assert!(!polyresearch_metadata.emits_live_input_health);
        assert!(!reference_price_provider_emits_live_input_health(
            polyresearch::REFERENCE_PRICE_PROVIDER_KEY
        ));
        assert!(!reference_price_provider_emits_live_input_health(
            "unregistered_reference_provider"
        ));
        let emitting_provider_keys = reference_price_provider_metadata_entries()
            .iter()
            .filter(|metadata| metadata.emits_live_input_health)
            .map(|metadata| metadata.provider_key)
            .collect::<Vec<_>>();
        assert_eq!(
            emitting_provider_keys,
            vec![chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY],
            "attach_live_input_health_transition_emitters currently attaches Chainlink live input-health emitters; add a provider attach path when adding another emitting provider"
        );
    }

    #[test]
    fn provider_bindings_explicitly_classify_nt_reconnect_budget_capability() {
        let nt_backed_provider_keys = provider_bindings()
            .iter()
            .filter_map(|binding| match binding.nt_reconnect_budget {
                NtReconnectBudgetCapability::NotApplicable => None,
                NtReconnectBudgetCapability::Required(_) => Some(binding.key),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            nt_backed_provider_keys,
            vec![chainlink_reference::KEY, polyresearch::KEY]
        );
        assert_eq!(
            nt_reconnect_budget(polymarket::KEY, None)
                .expect("Polymarket should have an explicit reconnect-budget classification"),
            NtReconnectBudget::NotApplicable
        );
        assert!(matches!(
            nt_reconnect_budget("UNREGISTERED_PROVIDER", None),
            Err(NtReconnectBudgetResolutionError::UnsupportedProvider { .. })
        ));
    }

    fn fake_secret_value(path: &str) -> String {
        match path {
            "/bolt/polymarket/private-key" => {
                "0x1111111111111111111111111111111111111111111111111111111111111111".to_string()
            }
            "/bolt/polymarket/api-key" => "poly-api-key".to_string(),
            "/bolt/polymarket/api-secret" => "YWJj".to_string(),
            "/bolt/polymarket/api-passphrase" => "poly-passphrase".to_string(),
            _ => panic!("unexpected test SSM path {path}"),
        }
    }

    fn resolved_polymarket_secrets() -> ResolvedBoltV3Secrets {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .clients
            .retain(|client_key, _| client_key == "polymarket_main");
        resolve_bolt_v3_secrets_with(&loaded, |_region, path| {
            Ok::<_, String>(fake_secret_value(path))
        })
        .expect("fixture polymarket secrets should resolve")
    }

    fn set_polymarket_execution_field(
        loaded: &mut LoadedBoltV3Config,
        field: &'static str,
        value: toml::Value,
    ) {
        let execution = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .and_then(|client| client.execution.as_mut())
            .expect("fixture should include polymarket execution block");
        let table = execution
            .as_table_mut()
            .expect("polymarket execution should be a table");
        table.insert(field.to_string(), value);
    }

    fn expect_resolution_error(
        result: Result<Arc<dyn FeeProvider>, FeeProviderResolutionError>,
        message: &'static str,
    ) -> FeeProviderResolutionError {
        match result {
            Ok(_) => panic!("{message}"),
            Err(error) => error,
        }
    }

    fn resolve_configured_fee_provider(
        loaded: &LoadedBoltV3Config,
        execution_client_id: &str,
        resolved: &ResolvedBoltV3Secrets,
    ) -> Result<Arc<dyn FeeProvider>, FeeProviderResolutionError> {
        let client = loaded
            .root
            .clients
            .get(execution_client_id)
            .expect("test execution client should be configured");
        resolve_fee_provider(execution_client_id, client, client.venue, resolved)
    }

    #[derive(Debug)]
    struct SentinelSecrets {
        value: String,
    }

    impl ProviderResolvedSecrets for SentinelSecrets {
        fn provider_key(&self) -> &'static str {
            "SENTINEL"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        fn redaction_values(&self) -> Vec<&str> {
            vec![self.value.as_str()]
        }
    }

    #[test]
    fn credential_log_modules_are_provider_owned() {
        let polymarket = binding_for_provider_key(polymarket::KEY)
            .expect("Polymarket binding must be registered");
        assert_eq!(
            polymarket.credential_log_modules,
            polymarket::CREDENTIAL_LOG_MODULES
        );

        let binance =
            binding_for_provider_key(binance::KEY).expect("Binance binding must be registered");
        assert_eq!(
            binance.credential_log_modules,
            binance::CREDENTIAL_LOG_MODULES
        );
    }

    #[test]
    fn provider_required_secrets_rejects_credentialed_block_without_secrets() {
        let client = client_from_toml(
            r#"
            venue = "FAKE"

            [data]
            "#,
        );
        let requirement = ProviderSecretRequirement {
            block: ProviderCredentialedBlock::Data,
            consumer: "fake data adapter",
        };

        let errors =
            validate_required_secret_blocks("fake_client", "FAKE", &client, &[requirement]);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("declares [data] but is missing the required [secrets] block"));
        assert!(errors[0].contains("fake data adapter"));
    }

    #[test]
    fn provider_required_secrets_ignores_absent_credentialed_block() {
        let client = client_from_toml(
            r#"
            venue = "FAKE"
            "#,
        );
        let requirement = ProviderSecretRequirement {
            block: ProviderCredentialedBlock::Execution,
            consumer: "fake execution adapter",
        };

        let errors =
            validate_required_secret_blocks("fake_client", "FAKE", &client, &[requirement]);

        assert!(errors.is_empty());
    }

    #[test]
    fn fee_provider_resolution_uses_provider_binding_registry() {
        let loaded = fixture_loaded_config();
        let resolved = resolved_polymarket_secrets();

        let provider = resolve_configured_fee_provider(&loaded, "polymarket_main", &resolved)
            .expect("polymarket fee provider should resolve through provider binding");

        assert!(
            binding_for_provider_key(polymarket::KEY)
                .expect("polymarket binding should exist")
                .build_fee_provider
                .is_some(),
            "polymarket binding should expose a fee-provider capability"
        );
        assert!(
            provider
                .fee_bps("condition-token.POLYMARKET".into())
                .is_none()
        );
    }

    #[test]
    fn fee_provider_resolution_rejects_unsupported_provider_kind() {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.insert(
            "fake_execution".to_string(),
            client_from_toml(
                r#"
                venue = "FAKE"
                "#,
            ),
        );
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "fake_execution", &resolved),
            "unsupported provider key must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::UnsupportedProvider { .. }
        ));
        assert!(error.to_string().contains("FAKE"));
    }

    #[test]
    fn fee_provider_resolution_rejects_provider_without_fee_binding() {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .clients
            .insert("binance_reference".to_string(), binance_reference_client());
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "binance_reference", &resolved),
            "provider without fee binding must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderWithoutFeeBinding { .. }
        ));
        assert!(error.to_string().contains("BINANCE"));
    }

    #[test]
    fn fee_provider_resolution_reports_provider_config_parse_failure() {
        let mut loaded = fixture_loaded_config();
        set_polymarket_execution_field(
            &mut loaded,
            "fee_cache_ttl_secs",
            toml::Value::String("not-an-integer".to_string()),
        );
        let resolved = resolved_polymarket_secrets();

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "polymarket_main", &resolved),
            "provider config parse failure must surface through resolver",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild {
                source: BoltV3AdapterMappingError::SchemaParse { .. },
                ..
            }
        ));
        assert!(error.to_string().contains("failed to deserialize"));
    }

    #[test]
    fn fee_provider_resolution_rejects_invalid_secret_binding() {
        let loaded = fixture_loaded_config();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "polymarket_main", &resolved),
            "missing resolved secret binding must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild {
                source: BoltV3AdapterMappingError::MissingResolvedSecrets { .. },
                ..
            }
        ));
    }

    #[test]
    fn fee_provider_resolution_reports_provider_client_construction_failure() {
        let mut loaded = fixture_loaded_config();
        set_polymarket_execution_field(
            &mut loaded,
            "base_url_http",
            toml::Value::String("not a url".to_string()),
        );
        let resolved = resolved_polymarket_secrets();

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "polymarket_main", &resolved),
            "provider client construction failure must surface through resolver",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild {
                source: BoltV3AdapterMappingError::ValidationInvariant { .. },
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .contains("failed to create Polymarket fee HTTP client")
        );
    }

    #[test]
    fn fee_provider_resolution_error_display_debug_redacts_sentinel_secret() {
        let loaded = fixture_loaded_config();
        let sentinel = "sentinel-secret-value-453";
        let mut clients = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(SentinelSecrets {
                value: sentinel.to_string(),
            }) as ResolvedBoltV3ClientSecrets,
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "polymarket_main", &resolved),
            "sentinel secret provider mismatch must fail without leaking secret value",
        );
        let display = error.to_string();
        let debug = format!("{error:?}");

        assert!(!display.contains(sentinel), "{display}");
        assert!(!debug.contains(sentinel), "{debug}");
    }

    #[test]
    fn fee_provider_resolution_redacts_provider_build_secret_errors() {
        let loaded = fixture_loaded_config();
        let sentinel = "sentinel-secret-value-453";
        let mut clients = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(polymarket::ResolvedBoltV3PolymarketSecrets {
                private_key: zeroize::Zeroizing::new(sentinel.to_string()),
                api_key: zeroize::Zeroizing::new("poly-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("YWJj".to_string()),
                passphrase: zeroize::Zeroizing::new("poly-passphrase".to_string()),
            }) as ResolvedBoltV3ClientSecrets,
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = expect_resolution_error(
            resolve_configured_fee_provider(&loaded, "polymarket_main", &resolved),
            "provider build failure must not leak malformed resolved secret values",
        );
        let display = error.to_string();
        let debug = format!("{error:?}");

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild { .. }
        ));
        assert!(!display.contains(sentinel), "{display}");
        assert!(!debug.contains(sentinel), "{debug}");
    }
}
