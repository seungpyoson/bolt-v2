use std::cell::Cell;

use anyhow::Result;
use bolt_v2::bolt_v3_evidence_novelty::{
    EvidenceCanonicalState, EvidenceEpisodeId, EvidenceEpisodeParts, EvidenceNoveltyGuard,
    EvidenceOutcomeIdentity, EvidenceStateOwner, registered_evidence_state_by_id,
};

fn outcome(index: u8, label: &str, instrument_id: &str) -> EvidenceOutcomeIdentity {
    EvidenceOutcomeIdentity {
        index,
        normalized_outcome: label.to_string(),
        instrument_id: instrument_id.to_string(),
    }
}

fn episode_parts(market_id: &str) -> EvidenceEpisodeParts {
    EvidenceEpisodeParts {
        strategy_id: "binary-oracle-btc".to_string(),
        target_id: "btc-updown-5m".to_string(),
        venue_id: "POLYMARKET".to_string(),
        market_id: market_id.to_string(),
        condition_id: format!("condition-{market_id}"),
        question_id: format!("question-{market_id}"),
        outcomes: [
            outcome(0, "up", &format!("up-{market_id}.POLYMARKET")),
            outcome(1, "down", &format!("down-{market_id}.POLYMARKET")),
        ],
    }
}

fn episode(market_id: &str) -> EvidenceEpisodeId {
    EvidenceEpisodeId::try_from(episode_parts(market_id))
        .expect("complete stable market identity should construct an episode")
}

#[test]
fn canonical_state_ids_match_the_closed_registry() {
    assert_eq!(
        EvidenceCanonicalState::BlockedStrategyInputAcceptedWatermarkAbsent as usize,
        144
    );
    assert_eq!(
        EvidenceCanonicalState::BlockedStrategyInputRejectedNotReadyWatermarkPresent as usize,
        155
    );
    assert_eq!(
        EvidenceCanonicalState::EntrySkipStrategyCoreNotRegistered as usize,
        156
    );
    assert_eq!(
        EvidenceCanonicalState::EntrySkipOnePositionInvariantViolation as usize,
        172
    );
    assert!(registered_evidence_state_by_id(143).is_err());
    assert!(registered_evidence_state_by_id(166).is_err());
    assert!(registered_evidence_state_by_id(173).is_err());
}

#[test]
fn stable_episode_identity_has_no_volatile_constructor_inputs() {
    let first_volatile_sample = (101_000.0, 1_u64, 2_u64, false, "missing_snapshot");
    let second_volatile_sample = (99_000.0, 999_999_u64, 88_888_u64, true, "rejected_stale");

    assert_ne!(first_volatile_sample, second_volatile_sample);
    assert_eq!(episode("market-a"), episode("market-a"));
}

#[test]
fn every_stable_market_component_changes_episode_identity() {
    let baseline = episode("market-a");
    let mut mutations = Vec::new();

    let mut parts = episode_parts("market-a");
    parts.strategy_id.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.target_id.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.venue_id.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.market_id.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.condition_id.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.question_id.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.outcomes[0].normalized_outcome.push_str("-changed");
    mutations.push(parts);
    let mut parts = episode_parts("market-a");
    parts.outcomes[0].instrument_id.push_str("-changed");
    mutations.push(parts);

    for parts in mutations {
        let changed = EvidenceEpisodeId::try_from(parts).expect("changed identity remains valid");
        assert_ne!(baseline, changed);
    }
}

#[test]
fn incomplete_or_ambiguous_episode_identity_is_rejected() {
    let mut blank_market = episode_parts("market-a");
    blank_market.market_id = "   ".to_string();
    assert!(EvidenceEpisodeId::try_from(blank_market).is_err());

    let mut padded_condition = episode_parts("market-a");
    padded_condition.condition_id = " condition-market-a".to_string();
    assert!(EvidenceEpisodeId::try_from(padded_condition).is_err());

    let mut reversed_indices = episode_parts("market-a");
    reversed_indices.outcomes[0].index = 1;
    reversed_indices.outcomes[1].index = 0;
    assert!(EvidenceEpisodeId::try_from(reversed_indices).is_err());

    let mut duplicate_label = episode_parts("market-a");
    duplicate_label.outcomes[1].normalized_outcome =
        duplicate_label.outcomes[0].normalized_outcome.clone();
    assert!(EvidenceEpisodeId::try_from(duplicate_label).is_err());

    let mut duplicate_instrument = episode_parts("market-a");
    duplicate_instrument.outcomes[1].instrument_id =
        duplicate_instrument.outcomes[0].instrument_id.clone();
    assert!(EvidenceEpisodeId::try_from(duplicate_instrument).is_err());
}

#[test]
fn semantic_a_b_a_oscillation_reaches_a_fixed_ceiling() -> Result<()> {
    let episode = episode("market-a");
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip);
    let appends = Cell::new(0_u64);
    let state_a = EvidenceCanonicalState::EntrySkipEntryGateBlocked;
    let state_b = EvidenceCanonicalState::EntrySkipEntryPricingBlocked;

    for _ in 0..100_000 {
        for state in [state_a, state_b, state_a] {
            guard.emit_once(&episode, state, || {
                appends.set(appends.get() + 1);
                Ok(())
            })?;
        }
    }

    assert_eq!(appends.get(), 2);
    assert_eq!(guard.seen_episode_count(), 1);
    assert_eq!(guard.seen_state_count(&episode), 2);
    Ok(())
}

#[test]
fn market_churn_never_evicts_seen_novelty() -> Result<()> {
    let first = episode("market-0");
    let state = EvidenceCanonicalState::EntrySkipEntryGateBlocked;
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip);
    let appends = Cell::new(0_u64);

    for index in 0..=4_097 {
        guard.emit_once(&episode(&format!("market-{index}")), state, || {
            appends.set(appends.get() + 1);
            Ok(())
        })?;
    }
    guard.emit_once(&first, state, || {
        appends.set(appends.get() + 1);
        Ok(())
    })?;

    assert_eq!(appends.get(), 4_098);
    assert_eq!(guard.seen_episode_count(), 4_098);
    Ok(())
}

#[test]
fn writer_failure_stays_claimed_and_owner_mismatch_never_emits() -> Result<()> {
    let episode = episode("market-a");
    let state = EvidenceCanonicalState::EntrySkipEntryGateBlocked;
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip);
    let attempts = Cell::new(0_u64);

    assert!(
        guard
            .emit_once(&episode, state, || {
                attempts.set(attempts.get() + 1);
                anyhow::bail!("writer unavailable")
            })
            .is_err()
    );
    assert!(!guard.emit_once(&episode, state, || Ok(()))?);
    assert_eq!(attempts.get(), 1);

    let payloads = Cell::new(0_u64);
    assert!(
        guard
            .emit_once(
                &episode,
                EvidenceCanonicalState::BlockedStrategyInputRejectedStaleWatermarkPresent,
                || {
                    payloads.set(payloads.get() + 1);
                    Ok(())
                },
            )
            .is_err()
    );
    assert_eq!(payloads.get(), 0);
    Ok(())
}
