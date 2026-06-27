use crate::bolt_v3_iv_support;

use std::collections::BTreeSet;

use bolt_v2::bolt_v3_iv::{
    audit::{IvAuditHandleId, IvAuditPolicy, IvAuditRetention, IvRawProductKind},
    authz::{IvAuthorizationMode, IvSelectorAuthorization},
    bounds::{IvBoundUnit, IvConventionBounds, IvNumericBounds},
    error::IvRejectReason,
    health::{IvSourceHealth, IvSourceHealthState},
    provenance::{IvPolicyDecision, IvProvenance},
    selector::IvSelector,
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvProductKind, IvSourceKind},
};

#[test]
fn foundational_enums_cover_required_iv_surfaces() {
    assert_eq!(
        bolt_v3_iv_support::all_source_kinds(),
        vec![
            IvSourceKind::OptionGreeks,
            IvSourceKind::OptionChain,
            IvSourceKind::AggregateGreeks,
            IvSourceKind::CustomImpliedVolatility,
        ]
    );

    assert_eq!(
        bolt_v3_iv_support::all_product_kinds(),
        vec![
            IvProductKind::IvPoint,
            IvProductKind::IvGreeksPoint,
            IvProductKind::Smile,
            IvProductKind::Surface,
            IvProductKind::AggregateGreeks,
            IvProductKind::CustomIvEvidence,
            IvProductKind::ProjectedScalarIv,
            IvProductKind::DerivedIv,
            IvProductKind::DerivedInputDiagnostics,
            IvProductKind::SourceHealth,
        ]
    );

    assert_eq!(
        IvRejectReason::required_reasons().len(),
        bolt_v3_iv_support::required_reject_reason_count()
    );
}

#[test]
fn selector_authorization_models_profile_wide_and_selector_scoped_access() {
    let profile_wide = IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::ProfileWide,
        strategy_id: bolt_v3_iv_support::strategy_id(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::Surface]),
        allowed_selector_fingerprints: BTreeSet::new(),
        allowed_source_ids: BTreeSet::new(),
    };

    let selector_scoped = IvSelectorAuthorization {
        authorization_mode: IvAuthorizationMode::SelectorScoped,
        strategy_id: bolt_v3_iv_support::strategy_id(),
        allowed_product_kinds: BTreeSet::from([IvProductKind::IvPoint]),
        allowed_selector_fingerprints: BTreeSet::from([bolt_v3_iv_support::selector_fingerprint()]),
        allowed_source_ids: BTreeSet::from([bolt_v3_iv_support::source_id()]),
    };

    assert!(profile_wide.is_profile_wide());
    assert!(selector_scoped.is_selector_scoped());
}

#[test]
fn source_health_transitions_follow_the_data_model() {
    assert!(IvSourceHealthState::Configured.can_transition_to(IvSourceHealthState::Subscribing));
    assert!(IvSourceHealthState::Subscribing.can_transition_to(IvSourceHealthState::Active));
    assert!(IvSourceHealthState::Subscribing.can_transition_to(IvSourceHealthState::Unsubscribing));
    assert!(IvSourceHealthState::Subscribing.can_transition_to(IvSourceHealthState::Removed));
    assert!(IvSourceHealthState::Active.can_transition_to(IvSourceHealthState::Stale));
    assert!(IvSourceHealthState::Active.can_transition_to(IvSourceHealthState::Removed));
    assert!(IvSourceHealthState::Stale.can_transition_to(IvSourceHealthState::Active));
    assert!(IvSourceHealthState::Stale.can_transition_to(IvSourceHealthState::Removed));
    assert!(IvSourceHealthState::Unsubscribing.can_transition_to(IvSourceHealthState::Subscribing));
    assert!(IvSourceHealthState::Unsubscribing.can_transition_to(IvSourceHealthState::Removed));
    assert!(
        IvSourceHealthState::SubscriptionFailed.can_transition_to(IvSourceHealthState::Subscribing)
    );
    assert!(
        IvSourceHealthState::SubscriptionFailed
            .can_transition_to(IvSourceHealthState::Unsubscribing)
    );
    assert!(
        IvSourceHealthState::SubscriptionFailed.can_transition_to(IvSourceHealthState::Removed)
    );
    assert!(IvSourceHealthState::Configured.can_transition_to(IvSourceHealthState::Unsubscribing));
    assert!(IvSourceHealthState::Configured.can_transition_to(IvSourceHealthState::Removed));
    assert!(!IvSourceHealthState::Removed.can_transition_to(IvSourceHealthState::Subscribing));
    assert!(!IvSourceHealthState::Rejected.can_transition_to(IvSourceHealthState::Subscribing));
    assert!(!IvSourceHealthState::Rejected.can_transition_to(IvSourceHealthState::Unsubscribing));
    assert!(!IvSourceHealthState::Rejected.can_transition_to(IvSourceHealthState::Removed));
    assert!(IvSourceHealthState::Active.can_transition_to(IvSourceHealthState::Rejected));
    assert!(!IvSourceHealthState::Removed.can_transition_to(IvSourceHealthState::Active));
    assert!(!IvSourceHealthState::Rejected.can_transition_to(IvSourceHealthState::Active));
    assert!(IvSourceHealthState::Removed.can_transition_to(IvSourceHealthState::Removed));
    assert!(IvSourceHealthState::Rejected.can_transition_to(IvSourceHealthState::Rejected));

    let health = IvSourceHealth {
        profile_id: bolt_v3_iv_support::profile_id(),
        source_id: bolt_v3_iv_support::source_id(),
        subscription_state: IvSourceHealthState::Active,
        last_event_ts_ns: None,
        last_reject_reason: None,
        reject_counts: Default::default(),
        stale_state: false,
        retention_state: false,
        subscription_generation: 0,
    };

    assert!(health.can_satisfy_current_query());
}

#[test]
fn foundational_structs_use_typed_time_bounds_audit_and_provenance() {
    let bounds = IvNumericBounds {
        finite_required: true,
        positive_required: true,
        inclusive_min: None,
        inclusive_max: None,
        exclusive_min: Some(0.0),
        exclusive_max: None,
        unit: IvBoundUnit::Unitless,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: BTreeSet::from([IvConvention::Named(
                bolt_v3_iv_support::convention_name(),
            )]),
        },
    };

    let audit_policy = IvAuditPolicy {
        profile_id: bolt_v3_iv_support::profile_id(),
        enabled_raw_products: BTreeSet::from([IvRawProductKind::OptionGreeks]),
        authorized_audit_handles: BTreeSet::from([IvAuditHandleId(
            bolt_v3_iv_support::audit_handle_id(),
        )]),
        access_purposes: BTreeSet::from([bolt_v3_iv_support::access_purpose()]),
        eligible_sources: BTreeSet::from([bolt_v3_iv_support::source_id()]),
        audit_retention: IvAuditRetention::empty(),
    };

    let selector = IvSelector::PointQuery {
        instrument_ids: vec![bolt_v3_iv_support::instrument_id()],
        basis: IvBasis::Mark,
        as_of_ns: UnixNanos::new(1),
        source_filter: Some(bolt_v3_iv_support::source_id()),
    };

    let provenance = IvProvenance {
        profile_id: bolt_v3_iv_support::profile_id(),
        source_id: bolt_v3_iv_support::source_id(),
        source_kind: IvSourceKind::OptionGreeks,
        selector_fingerprint: bolt_v3_iv_support::selector_fingerprint(),
        nt_revision: bolt_v3_iv_support::nt_revision(),
        nt_evidence_path: bolt_v3_iv_support::nt_evidence_path(),
        nt_symbol: bolt_v3_iv_support::nt_symbol(),
        raw_event_id: None,
        payload_kind: None,
        input_event_ids: Vec::new(),
        helper_identity: None,
        policy_decisions: vec![IvPolicyDecision::RejectionDecision {
            reject_reason: IvRejectReason::ProjectionRejected,
            failed_field: Some("configured-projection".to_string()),
            policy_id: Some("configured-policy".to_string()),
            source_health_state: IvSourceHealthState::Active,
            subscription_generation: 0,
        }],
        transformation_steps: Vec::new(),
        ts_event_ns: UnixNanos::new(1),
        ts_init_ns: None,
        received_ts_ns: UnixNanos::new(2),
        ingest_sequence: 1,
        subscription_generation: 0,
        source_health_state: IvSourceHealthState::Active,
        reject_reason: Some(IvRejectReason::ProjectionRejected),
    };

    assert_eq!(bounds.unit, IvBoundUnit::Unitless);
    assert!(audit_policy.raw_product_enabled(IvRawProductKind::OptionGreeks));
    assert_eq!(selector.product_kind(), IvProductKind::IvPoint);
    assert!(provenance.has_typed_policy_decision());
}

#[test]
fn numeric_bounds_reject_nan_even_when_finite_values_are_not_required() {
    let bounds = IvNumericBounds {
        finite_required: false,
        positive_required: true,
        inclusive_min: Some(0.0),
        inclusive_max: Some(5.0),
        exclusive_min: None,
        exclusive_max: None,
        unit: IvBoundUnit::Unitless,
        allowed_conventions: IvConventionBounds {
            allowed_conventions: BTreeSet::new(),
        },
    };

    assert!(!bounds.accepts(
        f64::NAN,
        &IvConvention::Named(bolt_v3_iv_support::convention_name())
    ));
}
