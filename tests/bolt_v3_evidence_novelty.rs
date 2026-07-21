use std::cell::Cell;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_decision_evidence::{
        BoltV3EntryBlockReason, BoltV3EntryPricingBlockReason, BoltV3EntrySkipReasonCategory,
        BoltV3OutcomeSide, BoltV3RvGateResult,
    },
    bolt_v3_evidence_novelty::{
        BLOCKED_STRATEGY_INPUT_SNAPSHOT_PER_REGISTERED_SOURCE_STATE_UPPER_BOUND,
        BLOCKED_STRATEGY_INPUT_SNAPSHOT_STATIC_STATE_UPPER_BOUND, CanonicalSet,
        CanonicalSourceStates, ENTRY_SKIP_PER_EPISODE_STATE_UPPER_BOUND, EntrySkipProducer,
        EntrySkipSemanticKey, EvidenceAttemptOutcome, EvidenceAvailability, EvidenceCoherence,
        EvidenceConditionIdentity, EvidenceEpisodeId, EvidenceEpisodeRejection,
        EvidenceMarketIdentity, EvidenceNoveltyGuard, EvidenceOutcomeIdentity,
        EvidenceQuestionIdentity, EvidenceStrategyIdentity, EvidenceTargetIdentity,
        EvidenceWatermark, RvSourceSemanticStateInput,
        blocked_strategy_input_snapshot_per_episode_state_upper_bound, generator::parse_registry,
        generator::render_registry, registered_evidence_dimension_by_id,
    },
    bolt_v3_realized_volatility::{
        RealizedVolBlockReason, RealizedVolSourceRejectReason, RealizedVolSourceStatus,
    },
};
use nautilus_model::identifiers::{InstrumentId, Venue};

const REGISTRY: &str = include_str!("../config/evidence-novelty.toml");
const GENERATED: &str = include_str!("../src/bolt_v3_evidence_novelty/generated.rs");

fn episode(market_id: &str) -> EvidenceEpisodeId {
    EvidenceEpisodeId::try_binary_market(
        EvidenceStrategyIdentity::try_new("binary-oracle-btc").unwrap(),
        EvidenceTargetIdentity::try_new("btc-updown-5m").unwrap(),
        Venue::from("POLYMARKET"),
        EvidenceMarketIdentity::try_new(market_id).unwrap(),
        EvidenceConditionIdentity::try_new(format!("condition-{market_id}")).unwrap(),
        EvidenceQuestionIdentity::try_new(format!("question-{market_id}")).unwrap(),
        [
            EvidenceOutcomeIdentity::new(
                BoltV3OutcomeSide::Up,
                InstrumentId::from(format!("up-{market_id}.POLYMARKET").as_str()),
            ),
            EvidenceOutcomeIdentity::new(
                BoltV3OutcomeSide::Down,
                InstrumentId::from(format!("down-{market_id}.POLYMARKET").as_str()),
            ),
        ],
    )
    .unwrap()
}

fn entry_key(
    reason: BoltV3EntrySkipReasonCategory,
    gate_blockers: Vec<BoltV3EntryBlockReason>,
    pricing_blockers: Vec<BoltV3EntryPricingBlockReason>,
    fast_available: bool,
    reference_available: bool,
    incoherent: bool,
    rv_gate: BoltV3RvGateResult,
    watermark_present: bool,
) -> Result<EntrySkipSemanticKey> {
    EntrySkipSemanticKey::try_new(
        reason,
        CanonicalSet::try_from_iter(gate_blockers)?,
        CanonicalSet::try_from_iter(pricing_blockers)?,
        EvidenceAvailability::from(fast_available),
        EvidenceAvailability::from(reference_available),
        EvidenceCoherence::from(incoherent),
        rv_gate,
        EvidenceWatermark::from(watermark_present),
    )
}

#[test]
fn registry_is_closed_and_generated_rust_is_byte_exact() -> Result<()> {
    let registry = parse_registry(REGISTRY)?;
    assert_eq!(render_registry(&registry)?, GENERATED);

    let unknown_root = format!("{REGISTRY}\nunknown = true\n");
    assert!(parse_registry(&unknown_root).is_err());

    let old_schema = REGISTRY.replacen("schema_version = 2", "schema_version = 1", 1);
    assert!(parse_registry(&old_schema).is_err());

    let duplicate_id = REGISTRY.replacen("id = 145", "id = 144", 1);
    assert!(parse_registry(&duplicate_id).is_err());

    let unknown_domain = REGISTRY.replacen(
        "domain = \"entry_skip_reason\"",
        "domain = \"missing_domain\"",
        1,
    );
    assert!(parse_registry(&unknown_domain).is_err());

    let unknown_optional_component = REGISTRY.replacen(
        "optional_component_domains = [\"rv_blocker\", \"rv_source_rejection\"]",
        "optional_component_domains = [\"missing_domain\"]",
        1,
    );
    assert!(parse_registry(&unknown_optional_component).is_err());

    let repeated_component = REGISTRY.replacen(
        "optional_component_domains = [\"rv_blocker\", \"rv_source_rejection\"]",
        "optional_component_domains = [\"enablement\"]",
        1,
    );
    assert!(parse_registry(&repeated_component).is_err());
    Ok(())
}

#[test]
fn generated_cardinality_formulas_are_checked_and_finite() {
    assert!(ENTRY_SKIP_PER_EPISODE_STATE_UPPER_BOUND > 4_096);
    assert_eq!(
        blocked_strategy_input_snapshot_per_episode_state_upper_bound(0),
        Some(BLOCKED_STRATEGY_INPUT_SNAPSHOT_STATIC_STATE_UPPER_BOUND)
    );
    assert_eq!(
        blocked_strategy_input_snapshot_per_episode_state_upper_bound(1),
        BLOCKED_STRATEGY_INPUT_SNAPSHOT_STATIC_STATE_UPPER_BOUND
            .checked_mul(BLOCKED_STRATEGY_INPUT_SNAPSHOT_PER_REGISTERED_SOURCE_STATE_UPPER_BOUND,)
    );
    assert_eq!(
        BLOCKED_STRATEGY_INPUT_SNAPSHOT_PER_REGISTERED_SOURCE_STATE_UPPER_BOUND,
        2 * 2 * 4 * 11 * 10,
        "source state includes enablement, quorum, status, optional blocker, and optional rejection"
    );
}

#[test]
fn registered_dimension_ids_cover_the_complete_key_schema() {
    let first = registered_evidence_dimension_by_id(144).unwrap();
    assert_eq!(first.producer, "entry_skip");
    assert_eq!(first.name, "reason");

    let last = registered_evidence_dimension_by_id(163).unwrap();
    assert_eq!(last.producer, "blocked_strategy_input_snapshot");
    assert_eq!(last.name, "rv_source_states");

    assert!(registered_evidence_dimension_by_id(143).is_err());
    assert!(registered_evidence_dimension_by_id(164).is_err());
}

#[test]
fn episode_identity_rejects_incomplete_ambiguous_or_wrong_venue_inputs() {
    assert_eq!(
        EvidenceMarketIdentity::try_new(""),
        Err(EvidenceEpisodeRejection::MarketIdentityMissing)
    );
    assert_eq!(
        EvidenceConditionIdentity::try_new(" padded"),
        Err(EvidenceEpisodeRejection::ConditionIdentityNonCanonical)
    );

    let strategy = EvidenceStrategyIdentity::try_new("strategy").unwrap();
    let target = EvidenceTargetIdentity::try_new("target").unwrap();
    let market = EvidenceMarketIdentity::try_new("market").unwrap();
    let condition = EvidenceConditionIdentity::try_new("condition").unwrap();
    let question = EvidenceQuestionIdentity::try_new("question").unwrap();
    let up = InstrumentId::from("up.POLYMARKET");
    let down = InstrumentId::from("down.POLYMARKET");

    assert_eq!(
        EvidenceEpisodeId::try_binary_market(
            strategy.clone(),
            target.clone(),
            Venue::from("POLYMARKET"),
            market.clone(),
            condition.clone(),
            question.clone(),
            [
                EvidenceOutcomeIdentity::new(BoltV3OutcomeSide::Down, down),
                EvidenceOutcomeIdentity::new(BoltV3OutcomeSide::Up, up),
            ],
        ),
        Err(EvidenceEpisodeRejection::OutcomeOrderInvalid)
    );
    assert_eq!(
        EvidenceEpisodeId::try_binary_market(
            strategy.clone(),
            target.clone(),
            Venue::from("POLYMARKET"),
            market.clone(),
            condition.clone(),
            question.clone(),
            [
                EvidenceOutcomeIdentity::new(BoltV3OutcomeSide::Up, up),
                EvidenceOutcomeIdentity::new(BoltV3OutcomeSide::Down, up),
            ],
        ),
        Err(EvidenceEpisodeRejection::OutcomeInstrumentDuplicate)
    );
    assert_eq!(
        EvidenceEpisodeId::try_binary_market(
            strategy,
            target,
            Venue::from("POLYMARKET"),
            market,
            condition,
            question,
            [
                EvidenceOutcomeIdentity::new(BoltV3OutcomeSide::Up, up),
                EvidenceOutcomeIdentity::new(
                    BoltV3OutcomeSide::Down,
                    InstrumentId::from("down.OTHER"),
                ),
            ],
        ),
        Err(EvidenceEpisodeRejection::OutcomeVenueMismatch)
    );
}

#[test]
fn canonical_sets_ignore_order_and_reject_duplicates() -> Result<()> {
    let left = CanonicalSet::try_from_iter([
        RealizedVolBlockReason::SourceStale,
        RealizedVolBlockReason::QuorumNotReady,
    ])?;
    let right = CanonicalSet::try_from_iter([
        RealizedVolBlockReason::QuorumNotReady,
        RealizedVolBlockReason::SourceStale,
    ])?;
    assert_eq!(left, right);
    assert!(
        CanonicalSet::try_from_iter([
            RealizedVolBlockReason::SourceStale,
            RealizedVolBlockReason::SourceStale,
        ])
        .is_err()
    );
    Ok(())
}

#[test]
fn structured_pricing_payloads_are_injective() -> Result<()> {
    let left = entry_key(
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
        vec![],
        vec![
            BoltV3EntryPricingBlockReason::FeeUnavailable(BoltV3OutcomeSide::Up),
            BoltV3EntryPricingBlockReason::ExecutableEntryCostUnavailable(BoltV3OutcomeSide::Down),
        ],
        true,
        true,
        false,
        BoltV3RvGateResult::Accepted,
        true,
    )?;
    let swapped = entry_key(
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
        vec![],
        vec![
            BoltV3EntryPricingBlockReason::FeeUnavailable(BoltV3OutcomeSide::Down),
            BoltV3EntryPricingBlockReason::ExecutableEntryCostUnavailable(BoltV3OutcomeSide::Up),
        ],
        true,
        true,
        false,
        BoltV3RvGateResult::Accepted,
        true,
    )?;
    assert_ne!(left, swapped);
    Ok(())
}

#[test]
fn canonical_source_states_bind_status_to_registered_source_identity() -> Result<()> {
    let source_a = RvSourceSemanticStateInput {
        source_id: "source-a".to_string(),
        enabled: true,
        counts_toward_quorum: true,
        status: RealizedVolSourceStatus::Blocked,
        block_reason: Some(RealizedVolBlockReason::SourceStale),
        last_rejected_reason: Some(RealizedVolSourceRejectReason::InvalidPrice),
    };
    let source_b = RvSourceSemanticStateInput {
        source_id: "source-b".to_string(),
        enabled: true,
        counts_toward_quorum: false,
        status: RealizedVolSourceStatus::DiagnosticOnly,
        block_reason: None,
        last_rejected_reason: None,
    };
    let left = CanonicalSourceStates::try_new(
        ["source-a".to_string(), "source-b".to_string()],
        [source_a.clone(), source_b.clone()],
        [],
    )?;
    let reordered = CanonicalSourceStates::try_new(
        ["source-b".to_string(), "source-a".to_string()],
        [source_b, source_a],
        [],
    )?;
    assert_eq!(left, reordered);
    assert_eq!(left.len(), 2);

    assert!(
        CanonicalSourceStates::try_new(
            ["source-a".to_string()],
            [],
            ["unknown-source".to_string()],
        )
        .is_err()
    );
    assert!(
        CanonicalSourceStates::try_new(
            ["source-a".to_string(), "source-b".to_string()],
            [RvSourceSemanticStateInput {
                source_id: "source-a".to_string(),
                enabled: true,
                counts_toward_quorum: true,
                status: RealizedVolSourceStatus::Ready,
                block_reason: None,
                last_rejected_reason: None,
            }],
            [],
        )
        .is_err(),
        "a registered source without one semantic row must fail closed"
    );
    assert!(
        CanonicalSourceStates::try_new(["source-a".to_string(), "source-a".to_string()], [], [],)
            .is_err(),
        "a duplicate registered source identity must fail closed"
    );
    Ok(())
}

#[test]
fn semantic_a_b_a_oscillation_reaches_a_fixed_ceiling() -> Result<()> {
    let episode = episode("market-a");
    let state_a = entry_key(
        BoltV3EntrySkipReasonCategory::EntryGateBlocked,
        vec![BoltV3EntryBlockReason::BookCrossed],
        vec![],
        true,
        true,
        false,
        BoltV3RvGateResult::Accepted,
        true,
    )?;
    let state_b = entry_key(
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
        vec![],
        vec![BoltV3EntryPricingBlockReason::SpotPriceMissing],
        false,
        true,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    let mut guard = EvidenceNoveltyGuard::<EntrySkipProducer>::new();
    let appends = Cell::new(0_u64);

    for _ in 0..100_000 {
        for key in [state_a.clone(), state_b.clone(), state_a.clone()] {
            let outcome = guard.attempt_once(Ok(episode.clone()), Ok(key), || {
                appends.set(appends.get() + 1);
                Ok(())
            });
            assert!(matches!(
                outcome,
                EvidenceAttemptOutcome::Appended | EvidenceAttemptOutcome::PreviouslyAttempted
            ));
        }
    }

    assert_eq!(appends.get(), 2);
    assert_eq!(guard.seen_episode_count(), 1);
    assert_eq!(guard.seen_state_count(&episode), 2);
    Ok(())
}

#[test]
fn volatile_observations_do_not_enter_the_complete_key() -> Result<()> {
    let first_tick = (101_000.0, 1_u64, 2_u64, 3_u64);
    let second_tick = (99_000.0, 999_999_u64, 88_888_u64, 77_777_u64);
    assert_ne!(first_tick, second_tick);

    let first = entry_key(
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
        vec![],
        vec![BoltV3EntryPricingBlockReason::SpotPriceMissing],
        false,
        false,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    let second = entry_key(
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
        vec![],
        vec![BoltV3EntryPricingBlockReason::SpotPriceMissing],
        false,
        false,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    assert_eq!(first, second);
    Ok(())
}

#[test]
fn unknown_runtime_state_fails_closed() -> Result<()> {
    assert!(
        entry_key(
            BoltV3EntrySkipReasonCategory::Unclassified,
            vec![],
            vec![],
            false,
            false,
            false,
            BoltV3RvGateResult::MissingSnapshot,
            false,
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn writer_failure_is_retained_without_claiming_append_success() -> Result<()> {
    let episode = episode("market-a");
    let key = entry_key(
        BoltV3EntrySkipReasonCategory::EntryGateBlocked,
        vec![BoltV3EntryBlockReason::PhaseNotActive],
        vec![],
        false,
        false,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    let mut guard = EvidenceNoveltyGuard::<EntrySkipProducer>::new();
    let attempts = Cell::new(0_u64);

    let first = guard.attempt_once(Ok(episode.clone()), Ok(key.clone()), || {
        attempts.set(attempts.get() + 1);
        anyhow::bail!("writer unavailable")
    });
    assert!(matches!(
        first,
        EvidenceAttemptOutcome::AttemptFailedAndRetained(_)
    ));
    let second = guard.attempt_once(Ok(episode), Ok(key), || {
        attempts.set(attempts.get() + 1);
        Ok(())
    });
    assert!(matches!(
        second,
        EvidenceAttemptOutcome::PreviouslyAttempted
    ));
    assert_eq!(attempts.get(), 1);
    Ok(())
}

#[test]
fn identity_rejection_is_fail_closed_and_bounded() -> Result<()> {
    let key = entry_key(
        BoltV3EntrySkipReasonCategory::InstrumentIdMissing,
        vec![],
        vec![],
        false,
        false,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    let mut guard = EvidenceNoveltyGuard::<EntrySkipProducer>::new();
    let writes = Cell::new(0_u64);
    let rejection = EvidenceEpisodeRejection::UpOutcomeInstrumentMissing;

    assert!(matches!(
        guard.attempt_once(Err(rejection), Ok(key.clone()), || {
            writes.set(writes.get() + 1);
            Ok(())
        }),
        EvidenceAttemptOutcome::IdentityRejectedFirst(
            EvidenceEpisodeRejection::UpOutcomeInstrumentMissing
        )
    ));
    assert!(matches!(
        guard.attempt_once(Err(rejection), Ok(key), || {
            writes.set(writes.get() + 1);
            Ok(())
        }),
        EvidenceAttemptOutcome::IdentityRejectedPreviously(
            EvidenceEpisodeRejection::UpOutcomeInstrumentMissing
        )
    ));
    assert_eq!(writes.get(), 0);
    assert_eq!(guard.rejected_identity_count(), 1);
    Ok(())
}

#[test]
fn semantic_key_rejection_is_fail_closed_and_bounded_per_episode() -> Result<()> {
    let episode = episode("market-invalid-key");
    let mut guard = EvidenceNoveltyGuard::<EntrySkipProducer>::new();
    let writes = Cell::new(0_u64);

    assert!(matches!(
        guard.attempt_once(
            Ok(episode.clone()),
            Err(anyhow::anyhow!("unregistered semantic component")),
            || {
                writes.set(writes.get() + 1);
                Ok(())
            },
        ),
        EvidenceAttemptOutcome::SemanticKeyRejectedFirst(_)
    ));
    assert!(matches!(
        guard.attempt_once(
            Ok(episode.clone()),
            Err(anyhow::anyhow!("different invalid payload")),
            || {
                writes.set(writes.get() + 1);
                Ok(())
            },
        ),
        EvidenceAttemptOutcome::SemanticKeyRejectedPreviously
    ));
    assert_eq!(writes.get(), 0);
    assert_eq!(guard.rejected_semantic_key_episode_count(), 1);

    let valid_key = entry_key(
        BoltV3EntrySkipReasonCategory::EntryGateBlocked,
        vec![BoltV3EntryBlockReason::PhaseNotActive],
        vec![],
        false,
        false,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    assert!(matches!(
        guard.attempt_once(Ok(episode), Ok(valid_key), || {
            writes.set(writes.get() + 1);
            Ok(())
        }),
        EvidenceAttemptOutcome::Appended
    ));
    assert_eq!(writes.get(), 1);
    Ok(())
}

#[test]
fn market_churn_never_evicts_seen_novelty() -> Result<()> {
    let first = episode("market-0");
    let key = entry_key(
        BoltV3EntrySkipReasonCategory::EntryGateBlocked,
        vec![BoltV3EntryBlockReason::PhaseNotActive],
        vec![],
        false,
        false,
        false,
        BoltV3RvGateResult::MissingSnapshot,
        false,
    )?;
    let mut guard = EvidenceNoveltyGuard::<EntrySkipProducer>::new();
    let appends = Cell::new(0_u64);

    for index in 0..=4_097 {
        let outcome = guard.attempt_once(
            Ok(episode(&format!("market-{index}"))),
            Ok(key.clone()),
            || {
                appends.set(appends.get() + 1);
                Ok(())
            },
        );
        assert!(matches!(outcome, EvidenceAttemptOutcome::Appended));
    }
    assert!(matches!(
        guard.attempt_once(Ok(first), Ok(key), || {
            appends.set(appends.get() + 1);
            Ok(())
        }),
        EvidenceAttemptOutcome::PreviouslyAttempted
    ));

    assert_eq!(appends.get(), 4_098);
    assert_eq!(guard.seen_episode_count(), 4_098);
    Ok(())
}
