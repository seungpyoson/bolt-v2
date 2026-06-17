use std::collections::BTreeSet;

use anyhow::{Result, bail, ensure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardFieldGroup {
    OrdersFillsPositions,
    TradeExplanation,
    AccountStateAndPortfolioEquity,
    Exposure,
    HistoricalPnl,
    RedemptionRealizedPnl,
    StrategyStateOutlook,
    DataHealthFreshness,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceRole {
    Authoritative,
    Derived,
    Exploratory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataStatus {
    Current,
    Stale,
    Partial,
    Unavailable,
    Excluded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GapReason {
    MissingSource,
    UpstreamBlocked,
    ScopeExcluded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunPurpose {
    Normal,
    Reproduction,
    Audit,
    Regression,
    Migration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FidelityClass {
    L2Replay,
    TradeBarReplay,
    SignalOnly,
    ForwardCapturePending,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DashboardSourceRef {
    NtReport { report_uri: String },
    NtEvent { event_uri: String },
    NtSnapshot { snapshot_uri: String },
    PortfolioSnapshot { snapshot_id: String },
    CatalogArrow { artifact_uri: String },
    DurableTradeHistoryPnl { artifact_uri: String },
    AcceptedAnalytics { artifact_uri: String },
    GapLabel { gap_key: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardFreshnessEvidence {
    pub source_timestamp_epoch_seconds: u64,
    pub observed_at_epoch_seconds: u64,
    pub stale_after_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardFieldSource {
    pub field_key: String,
    pub group: DashboardFieldGroup,
    pub source_ref: DashboardSourceRef,
    pub source_binding_key: Option<String>,
    pub source_role: SourceRole,
    pub data_status: DataStatus,
    pub gap_reason: Option<GapReason>,
    pub source_proof_id: Option<String>,
    pub run_purpose: RunPurpose,
    pub fidelity_class: Option<FidelityClass>,
    pub claim_limits: Vec<String>,
    pub warning_fields: Vec<String>,
    pub freshness: Option<DashboardFreshnessEvidence>,
    pub accepted_source_contract_ref: Option<String>,
    pub upgrades_proof_strength: bool,
    pub weakens_forbidden_claims: bool,
    pub relabels_historical_result_after_supersession: bool,
    pub calculates_strategy_truth: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardSourceBinding {
    pub source_binding_key: String,
    pub venue_key: String,
    pub provider_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardArtifactLinkScope {
    ArtifactRootUri,
    DirectUpstreamHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardArtifactLink {
    pub link_key: String,
    pub uri: String,
    pub scope: DashboardArtifactLinkScope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactIndexBulkListSource {
    CommittedSnapshot {
        snapshot_uri: String,
        manifest_lineage_ids: Vec<String>,
    },
    IndependentLatestPointers {
        pointer_uris: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardMutationKind {
    SubmitOrder,
    CancelOrder,
    TransferFunds,
    MutateCredential,
    MutateRuntimeConfig,
    DeleteCanonicalArtifact,
    ExpireCanonicalArtifact,
    PublishArtifactIndexRecord,
    RepairArtifactIndexRecord,
    MutateAcceptedSourceProof,
    MutateRaFindingVerdict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DashboardAction {
    ReadOnlyView {
        action_key: String,
    },
    Mutation {
        action_key: String,
        mutation: DashboardMutationKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardProductGateOutcome {
    SelectedExistingProduct,
    CustomUiRequiresException,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardProductGate {
    pub outcome: DashboardProductGateOutcome,
    pub selected_product_ref: String,
    pub rejected_product_refs: Vec<String>,
    pub no_mutation_controls_ref: String,
    pub annotation_writes_enabled: bool,
    pub annotation_owner_schema_audit_ref: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RaVerdictSource {
    DisplayOnly { verdict_artifact_ref: String },
    NotDisplayed,
    DerivedFromBteMetrics,
    MutatesFindingReviewArtifact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardReadModelSpec {
    pub artifact_root: String,
    pub source_bindings: Vec<DashboardSourceBinding>,
    pub required_field_keys: Vec<String>,
    pub fields: Vec<DashboardFieldSource>,
    pub artifact_links: Vec<DashboardArtifactLink>,
    pub artifact_index_bulk_list_source: ArtifactIndexBulkListSource,
    pub actions: Vec<DashboardAction>,
    pub product_gate: DashboardProductGate,
    pub ra_verdict_source: RaVerdictSource,
    pub redemption_realized_pnl_included: bool,
}

pub fn validate_dashboard_read_model(spec: &DashboardReadModelSpec) -> Result<()> {
    let artifact_root = normalize_artifact_root(&spec.artifact_root)?;
    let binding_sets = validate_source_bindings(&spec.source_bindings)?;
    validate_required_fields(&spec.required_field_keys, &spec.fields)?;

    for field in &spec.fields {
        validate_field(&artifact_root, &binding_sets, spec, field)?;
    }
    for link in &spec.artifact_links {
        validate_artifact_link(&artifact_root, link)?;
    }
    validate_artifact_index_bulk_list_source(
        &artifact_root,
        &spec.artifact_index_bulk_list_source,
    )?;
    validate_actions(&spec.actions)?;
    validate_product_gate(&spec.product_gate)?;
    validate_ra_verdict_source(&spec.ra_verdict_source)?;

    Ok(())
}

#[derive(Debug)]
struct BindingSets {
    source_binding_keys: BTreeSet<String>,
    venue_keys: BTreeSet<String>,
    provider_keys: BTreeSet<String>,
}

fn validate_source_bindings(bindings: &[DashboardSourceBinding]) -> Result<BindingSets> {
    ensure!(!bindings.is_empty(), "source_bindings must not be empty");

    let mut source_binding_keys = BTreeSet::new();
    let mut venue_keys = BTreeSet::new();
    let mut provider_keys = BTreeSet::new();
    for binding in bindings {
        ensure_non_empty("source_binding_key", &binding.source_binding_key)?;
        ensure_non_empty("venue_key", &binding.venue_key)?;
        ensure_non_empty("provider_key", &binding.provider_key)?;
        ensure!(
            source_binding_keys.insert(binding.source_binding_key.clone()),
            "duplicate source_binding_key {:?}",
            binding.source_binding_key
        );
        venue_keys.insert(binding.venue_key.clone());
        provider_keys.insert(binding.provider_key.clone());
    }

    Ok(BindingSets {
        source_binding_keys,
        venue_keys,
        provider_keys,
    })
}

fn validate_required_fields(
    required_field_keys: &[String],
    fields: &[DashboardFieldSource],
) -> Result<()> {
    ensure!(
        !required_field_keys.is_empty(),
        "required_field_keys must not be empty"
    );
    ensure!(!fields.is_empty(), "fields must not be empty");

    let mut required = BTreeSet::new();
    for field_key in required_field_keys {
        ensure_non_empty("required_field_keys", field_key)?;
        ensure!(
            required.insert(field_key.clone()),
            "duplicate required field {field_key:?}"
        );
    }

    let mut mapped = BTreeSet::new();
    for field in fields {
        ensure_non_empty("field_key", &field.field_key)?;
        ensure!(
            mapped.insert(field.field_key.clone()),
            "duplicate dashboard field {:?}",
            field.field_key
        );
    }

    let missing = required
        .difference(&mapped)
        .cloned()
        .collect::<Vec<String>>();
    ensure!(missing.is_empty(), "unmapped dashboard fields: {missing:?}");
    Ok(())
}

fn validate_field(
    artifact_root: &str,
    binding_sets: &BindingSets,
    spec: &DashboardReadModelSpec,
    field: &DashboardFieldSource,
) -> Result<()> {
    ensure_non_empty("field_key", &field.field_key)?;
    validate_source_binding_key(binding_sets, field.source_binding_key.as_deref())?;
    validate_source_ref(artifact_root, &field.source_ref)?;
    validate_status_and_gap(field)?;
    validate_freshness(field)?;
    validate_dashboard_does_not_reclassify(field)?;
    validate_group_source_rules(spec, field)?;
    Ok(())
}

fn validate_source_binding_key(
    binding_sets: &BindingSets,
    source_binding_key: Option<&str>,
) -> Result<()> {
    let Some(value) = source_binding_key else {
        return Ok(());
    };
    ensure_non_empty("source_binding_key", value)?;
    if binding_sets.source_binding_keys.contains(value) {
        return Ok(());
    }
    if binding_sets.venue_keys.contains(value) || binding_sets.provider_keys.contains(value) {
        bail!("dashboard field source_binding_key must not use venue/provider identity");
    }
    bail!("dashboard field source_binding_key {value:?} is not configured")
}

fn validate_source_ref(artifact_root: &str, source_ref: &DashboardSourceRef) -> Result<()> {
    match source_ref {
        DashboardSourceRef::NtReport { report_uri } => {
            ensure_uri_under_artifact_root("report_uri", artifact_root, report_uri)
        }
        DashboardSourceRef::NtEvent { event_uri } => {
            ensure_uri_under_artifact_root("event_uri", artifact_root, event_uri)
        }
        DashboardSourceRef::NtSnapshot { snapshot_uri } => {
            ensure_uri_under_artifact_root("snapshot_uri", artifact_root, snapshot_uri)
        }
        DashboardSourceRef::PortfolioSnapshot { snapshot_id } => {
            ensure_non_empty("portfolio snapshot id", snapshot_id)
        }
        DashboardSourceRef::CatalogArrow { artifact_uri }
        | DashboardSourceRef::DurableTradeHistoryPnl { artifact_uri }
        | DashboardSourceRef::AcceptedAnalytics { artifact_uri } => {
            ensure_uri_under_artifact_root("artifact_uri", artifact_root, artifact_uri)
        }
        DashboardSourceRef::GapLabel { gap_key } => ensure_non_empty("gap_key", gap_key),
    }
}

fn validate_status_and_gap(field: &DashboardFieldSource) -> Result<()> {
    match field.data_status {
        DataStatus::Current | DataStatus::Stale => {
            ensure!(
                field.gap_reason.is_none(),
                "current/stale fields must not carry a gap_reason"
            );
            ensure!(
                !matches!(field.source_ref, DashboardSourceRef::GapLabel { .. }),
                "current/stale fields must not use a gap label source"
            );
        }
        DataStatus::Partial | DataStatus::Unavailable | DataStatus::Excluded => {
            ensure!(
                field.gap_reason.is_some(),
                "partial/unavailable/excluded fields require a gap_reason"
            );
        }
    }
    Ok(())
}

fn validate_freshness(field: &DashboardFieldSource) -> Result<()> {
    let Some(freshness) = &field.freshness else {
        return Ok(());
    };
    ensure!(
        freshness.observed_at_epoch_seconds >= freshness.source_timestamp_epoch_seconds,
        "freshness observed timestamp must not precede source timestamp"
    );
    let age = freshness.observed_at_epoch_seconds - freshness.source_timestamp_epoch_seconds;
    if age > freshness.stale_after_seconds && field.data_status == DataStatus::Current {
        bail!(
            "stale dashboard field {:?} cannot render as current",
            field.field_key
        );
    }
    Ok(())
}

fn validate_dashboard_does_not_reclassify(field: &DashboardFieldSource) -> Result<()> {
    ensure!(
        !field.upgrades_proof_strength,
        "dashboard must not perform proof-strength reclassification"
    );
    ensure!(
        !field.weakens_forbidden_claims,
        "dashboard must not weaken forbidden claims"
    );
    ensure!(
        !field.relabels_historical_result_after_supersession,
        "dashboard must not relabel historical results after proof supersession"
    );
    Ok(())
}

fn validate_group_source_rules(
    spec: &DashboardReadModelSpec,
    field: &DashboardFieldSource,
) -> Result<()> {
    match field.group {
        DashboardFieldGroup::AccountStateAndPortfolioEquity => {
            validate_portfolio_snapshot_source(field)
        }
        DashboardFieldGroup::Exposure => validate_exposure_source(field),
        DashboardFieldGroup::HistoricalPnl => validate_historical_pnl_source(field),
        DashboardFieldGroup::RedemptionRealizedPnl => validate_redemption_pnl_source(spec, field),
        DashboardFieldGroup::StrategyStateOutlook => validate_strategy_outlook_source(field),
        DashboardFieldGroup::OrdersFillsPositions
        | DashboardFieldGroup::TradeExplanation
        | DashboardFieldGroup::DataHealthFreshness => Ok(()),
    }
}

fn validate_portfolio_snapshot_source(field: &DashboardFieldSource) -> Result<()> {
    if field.data_status == DataStatus::Current {
        ensure!(
            matches!(
                field.source_ref,
                DashboardSourceRef::PortfolioSnapshot { .. }
            ),
            "current account/PnL fields require PortfolioSnapshot source"
        );
    }
    Ok(())
}

fn validate_exposure_source(field: &DashboardFieldSource) -> Result<()> {
    if field.data_status == DataStatus::Current {
        ensure!(
            matches!(
                field.source_ref,
                DashboardSourceRef::NtReport { .. }
                    | DashboardSourceRef::NtEvent { .. }
                    | DashboardSourceRef::NtSnapshot { .. }
                    | DashboardSourceRef::AcceptedAnalytics { .. }
            ),
            "current exposure fields require NT source or accepted analytics source"
        );
    }
    Ok(())
}

fn validate_historical_pnl_source(field: &DashboardFieldSource) -> Result<()> {
    if field.data_status == DataStatus::Current {
        ensure!(
            matches!(
                field.source_ref,
                DashboardSourceRef::DurableTradeHistoryPnl { .. }
            ),
            "current historical PnL fields require durable trade-history/PnL source"
        );
    }
    Ok(())
}

fn validate_redemption_pnl_source(
    spec: &DashboardReadModelSpec,
    field: &DashboardFieldSource,
) -> Result<()> {
    if !spec.redemption_realized_pnl_included {
        ensure!(
            field.data_status == DataStatus::Excluded
                && field.gap_reason == Some(GapReason::ScopeExcluded),
            "redemption-realized PnL must be excluded until scope is accepted"
        );
    } else {
        ensure!(
            field.accepted_source_contract_ref.is_some(),
            "included redemption-realized PnL requires accepted source contract"
        );
    }
    Ok(())
}

fn validate_strategy_outlook_source(field: &DashboardFieldSource) -> Result<()> {
    ensure!(
        !field.calculates_strategy_truth,
        "dashboard must not calculate strategy state/outlook as trading truth"
    );
    if field.source_role != SourceRole::Exploratory {
        ensure!(
            field.accepted_source_contract_ref.is_some(),
            "non-exploratory strategy outlook requires accepted source contract"
        );
    }
    Ok(())
}

fn validate_artifact_link(artifact_root: &str, link: &DashboardArtifactLink) -> Result<()> {
    ensure_non_empty("artifact link key", &link.link_key)?;
    ensure_non_empty("artifact link uri", &link.uri)?;
    match link.scope {
        DashboardArtifactLinkScope::ArtifactRootUri => {
            ensure_uri_under_artifact_root("artifact link uri", artifact_root, &link.uri)
        }
        DashboardArtifactLinkScope::DirectUpstreamHandle => {
            ensure!(
                link.uri.starts_with("artifact://"),
                "direct upstream handles must use artifact:// URI"
            );
            Ok(())
        }
    }
}

fn validate_artifact_index_bulk_list_source(
    artifact_root: &str,
    source: &ArtifactIndexBulkListSource,
) -> Result<()> {
    match source {
        ArtifactIndexBulkListSource::CommittedSnapshot {
            snapshot_uri,
            manifest_lineage_ids,
        } => {
            ensure_uri_under_artifact_root("snapshot_uri", artifact_root, snapshot_uri)?;
            ensure!(
                !manifest_lineage_ids.is_empty(),
                "committed snapshot bulk lists require manifest lineage ids"
            );
            for lineage_id in manifest_lineage_ids {
                ensure_non_empty("manifest_lineage_ids", lineage_id)?;
            }
            Ok(())
        }
        ArtifactIndexBulkListSource::IndependentLatestPointers { .. } => {
            bail!(
                "dashboard bulk lists must use a committed snapshot, not independent latest pointers"
            )
        }
    }
}

fn validate_actions(actions: &[DashboardAction]) -> Result<()> {
    ensure!(!actions.is_empty(), "dashboard actions must not be empty");
    for action in actions {
        match action {
            DashboardAction::ReadOnlyView { action_key } => {
                ensure_non_empty("read-only action key", action_key)?;
            }
            DashboardAction::Mutation {
                action_key,
                mutation,
            } => {
                ensure_non_empty("mutation action key", action_key)?;
                bail!("dashboard is read-only; mutation action {mutation:?} is rejected");
            }
        }
    }
    Ok(())
}

fn validate_product_gate(product_gate: &DashboardProductGate) -> Result<()> {
    ensure_non_empty("selected_product_ref", &product_gate.selected_product_ref)?;
    ensure_non_empty(
        "no_mutation_controls_ref",
        &product_gate.no_mutation_controls_ref,
    )?;
    match product_gate.outcome {
        DashboardProductGateOutcome::SelectedExistingProduct => {}
        DashboardProductGateOutcome::CustomUiRequiresException => {
            ensure!(
                !product_gate.rejected_product_refs.is_empty(),
                "custom UI requires rejected product refs"
            );
        }
    }
    for product_ref in &product_gate.rejected_product_refs {
        ensure_non_empty("rejected_product_refs", product_ref)?;
    }
    if product_gate.annotation_writes_enabled {
        let annotation_ref = product_gate
            .annotation_owner_schema_audit_ref
            .as_deref()
            .unwrap_or_default();
        ensure_non_empty("annotation_owner_schema_audit_ref", annotation_ref)?;
    }
    Ok(())
}

fn validate_ra_verdict_source(source: &RaVerdictSource) -> Result<()> {
    match source {
        RaVerdictSource::DisplayOnly {
            verdict_artifact_ref,
        } => ensure_non_empty("verdict_artifact_ref", verdict_artifact_ref),
        RaVerdictSource::NotDisplayed => Ok(()),
        RaVerdictSource::DerivedFromBteMetrics => {
            bail!("dashboard must not derive RA verdict from BTE metrics")
        }
        RaVerdictSource::MutatesFindingReviewArtifact => {
            bail!("dashboard must not mutate RA finding/verdict review artifact")
        }
    }
}

fn normalize_artifact_root(artifact_root: &str) -> Result<String> {
    let normalized = artifact_root.trim_end_matches('/').to_string();
    ensure_non_empty("artifact_root", &normalized)?;
    ensure!(
        normalized.starts_with("s3://"),
        "artifact_root must be an s3:// URI"
    );
    Ok(normalized)
}

fn ensure_uri_under_artifact_root(
    field: &'static str,
    artifact_root: &str,
    uri: &str,
) -> Result<()> {
    ensure_non_empty(field, uri)?;
    ensure!(
        uri.starts_with(&format!("{artifact_root}/")),
        "{field} must live under artifact_root {artifact_root:?}"
    );
    Ok(())
}

fn ensure_non_empty(field: &'static str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    Ok(())
}
