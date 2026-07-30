use std::cell::Cell;

use anyhow::Result;
use bolt_v2::bolt_v3_evidence_novelty::{
    EvidenceCanonicalState, EvidenceEpisodeId, EvidenceEpisodeParts, EvidenceNoveltyGuard,
    EvidenceOutcomeIdentity, EvidenceStateOwner, registered_evidence_state_by_id,
};

fn outcome(index: u8, label: &str, token_id: &str) -> EvidenceOutcomeIdentity {
    EvidenceOutcomeIdentity {
        index,
        normalized_outcome: label.to_string(),
        clob_token_id: token_id.to_string(),
    }
}

fn episode_parts(market_id: &str) -> EvidenceEpisodeParts {
    EvidenceEpisodeParts {
        strategy_id: "binary-oracle-btc".to_string(),
        target_id: "btc-updown-5m".to_string(),
        venue_id: "POLYMARKET".to_string(),
        gamma_market_id: market_id.to_string(),
        condition_id: format!("condition-{market_id}"),
        question_id: format!("question-{market_id}"),
        negative_risk: false,
        outcomes: [
            outcome(0, "up", &format!("up-{market_id}")),
            outcome(1, "down", &format!("down-{market_id}")),
        ],
    }
}

fn episode(market_id: &str) -> EvidenceEpisodeId {
    EvidenceEpisodeId::try_from(episode_parts(market_id))
        .expect("complete stable market identity should construct an episode")
}

#[test]
fn canonical_state_ids_match_the_frozen_registry() {
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
        EvidenceCanonicalState::EntrySkipEntryUnfillableRejectedUnchangedBook as usize,
        175
    );
    assert!(registered_evidence_state_by_id(143).is_err());
    assert!(registered_evidence_state_by_id(176).is_err());
}

#[test]
fn stable_episode_identity_has_no_volatile_constructor_inputs() {
    let first = episode("market-a");
    let first_volatile_sample = (101_000.0, 1_u64, 2_u64, 3_u64, false, "missing_snapshot");
    let second_volatile_sample = (
        99_000.0,
        999_999_u64,
        88_888_u64,
        77_777_u64,
        true,
        "rejected_stale",
    );
    assert_eq!(first, episode("market-a"));
    assert_ne!(first_volatile_sample, second_volatile_sample);
}

#[test]
fn every_stable_market_component_changes_episode_identity() {
    let baseline = episode("market-a");
    let mut mutations = Vec::new();

    let mut parts = episode_parts("market-a");
    parts.strategy_id.push_str("-changed");
    mutations.push(("strategy", parts));
    let mut parts = episode_parts("market-a");
    parts.target_id.push_str("-changed");
    mutations.push(("target", parts));
    let mut parts = episode_parts("market-a");
    parts.venue_id.push_str("-changed");
    mutations.push(("venue", parts));
    let mut parts = episode_parts("market-a");
    parts.gamma_market_id.push_str("-changed");
    mutations.push(("Gamma market", parts));
    let mut parts = episode_parts("market-a");
    parts.condition_id.push_str("-changed");
    mutations.push(("condition", parts));
    let mut parts = episode_parts("market-a");
    parts.question_id.push_str("-changed");
    mutations.push(("question", parts));
    let mut parts = episode_parts("market-a");
    parts.negative_risk = true;
    mutations.push(("negative-risk mode", parts));
    let mut parts = episode_parts("market-a");
    parts.outcomes[0].normalized_outcome.push_str("-changed");
    mutations.push(("normalized outcome", parts));
    let mut parts = episode_parts("market-a");
    parts.outcomes[0].clob_token_id.push_str("-changed");
    mutations.push(("CLOB token", parts));

    for (field, parts) in mutations {
        let changed = EvidenceEpisodeId::try_from(parts).expect("changed identity remains valid");
        assert_ne!(baseline, changed, "changing {field} must change identity");
    }
}

#[test]
fn invalid_ordered_outcome_identity_is_rejected() {
    let mut out_of_range_index = episode_parts("market-a");
    out_of_range_index.outcomes[0].index = 2;
    assert!(EvidenceEpisodeId::try_from(out_of_range_index).is_err());

    let mut reversed_indices = episode_parts("market-a");
    reversed_indices.outcomes[0].index = 1;
    reversed_indices.outcomes[1].index = 0;
    assert!(EvidenceEpisodeId::try_from(reversed_indices).is_err());

    let mut duplicate_index = episode_parts("market-a");
    duplicate_index.outcomes[1].index = duplicate_index.outcomes[0].index;
    assert!(EvidenceEpisodeId::try_from(duplicate_index).is_err());

    let mut duplicate_label = episode_parts("market-a");
    duplicate_label.outcomes[1].normalized_outcome =
        duplicate_label.outcomes[0].normalized_outcome.clone();
    assert!(EvidenceEpisodeId::try_from(duplicate_label).is_err());

    let mut duplicate_token = episode_parts("market-a");
    duplicate_token.outcomes[1].clob_token_id = duplicate_token.outcomes[0].clob_token_id.clone();
    assert!(EvidenceEpisodeId::try_from(duplicate_token).is_err());
}

#[test]
fn blank_or_whitespace_padded_stable_identity_fields_are_rejected() {
    let mut blank_market = episode_parts("market-a");
    blank_market.gamma_market_id = "   ".to_string();
    assert!(EvidenceEpisodeId::try_from(blank_market).is_err());

    let mut padded_condition = episode_parts("market-a");
    padded_condition.condition_id = " condition-market-a".to_string();
    assert!(EvidenceEpisodeId::try_from(padded_condition).is_err());

    let mut padded_token = episode_parts("market-a");
    padded_token.outcomes[0].clob_token_id = "up-market-a ".to_string();
    assert!(EvidenceEpisodeId::try_from(padded_token).is_err());
}

#[test]
fn one_hundred_thousand_semantic_a_b_a_oscillations_reach_a_fixed_ceiling() -> Result<()> {
    let episode = episode("market-a");
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip)?;
    let payloads = Cell::new(0_u64);
    let appends = Cell::new(0_u64);
    let state_a = EvidenceCanonicalState::EntrySkipEntryGateBlocked;
    let state_b = EvidenceCanonicalState::EntrySkipEntryPricingBlocked;

    for _ in 0..100_000 {
        for state in [state_a, state_b, state_a] {
            guard.emit_once(&episode, state, || {
                payloads.set(payloads.get() + 1);
                appends.set(appends.get() + 1);
                Ok(())
            })?;
        }
    }

    assert_eq!(payloads.get(), 2);
    assert_eq!(appends.get(), 2);
    assert_eq!(guard.seen_episode_count(), 1);
    assert_eq!(guard.seen_state_count(&episode), 2);
    Ok(())
}

#[test]
fn episode_a_b_a_preserves_each_episode_novelty_domain() -> Result<()> {
    let episode_a = episode("market-a");
    let episode_b = episode("market-b");
    let state = EvidenceCanonicalState::EntrySkipEntryGateBlocked;
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip)?;
    let appends = Cell::new(0_u64);

    for episode in [&episode_a, &episode_b, &episode_a] {
        guard.emit_once(episode, state, || {
            appends.set(appends.get() + 1);
            Ok(())
        })?;
    }

    assert_eq!(appends.get(), 2);
    assert_eq!(guard.seen_episode_count(), 2);
    assert_eq!(guard.seen_state_count(&episode_a), 1);
    assert_eq!(guard.seen_state_count(&episode_b), 1);
    Ok(())
}

#[test]
fn more_than_retired_episode_ceiling_does_not_evict_seen_novelty() -> Result<()> {
    let first = episode("market-0");
    let state = EvidenceCanonicalState::EntrySkipEntryGateBlocked;
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip)?;
    let appends = Cell::new(0_u64);

    for index in 0..=4_097 {
        let current = episode(&format!("market-{index}"));
        guard.emit_once(&current, state, || {
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
    assert_eq!(guard.seen_state_count(&first), 1);
    Ok(())
}

#[test]
fn owner_mismatch_fails_before_payload_construction() -> Result<()> {
    let episode = episode("market-a");
    let mut guard = EvidenceNoveltyGuard::for_owner(EvidenceStateOwner::EntrySkip)?;
    let payloads = Cell::new(0_u64);
    let error = guard
        .emit_once(
            &episode,
            EvidenceCanonicalState::BlockedStrategyInputRejectedNotReadyWatermarkAbsent,
            || {
                payloads.set(payloads.get() + 1);
                Ok(())
            },
        )
        .expect_err("a canonical state owned by another producer must fail closed");
    assert!(error.to_string().contains("owner mismatch"));
    assert_eq!(payloads.get(), 0);
    Ok(())
}
