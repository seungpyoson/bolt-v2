use std::cell::Cell;

use anyhow::Result;
use bolt_v2::bolt_v3_evidence_novelty::{
    EvidenceCanonicalState, EvidenceEpisodeId, EvidenceEpisodeParts, EvidenceMarketIdentity,
    EvidenceNoveltyGuard, EvidenceOutcomeIdentity, EvidenceStateOwner,
    registered_evidence_state_by_id,
};
use bolt_v2::bolt_v3_market_families::{
    SelectedMarketEvidenceIdentity, SelectedMarketEvidenceOutcome,
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
        market: market_identity(market_id),
    }
}

fn market_identity(market_id: &str) -> EvidenceMarketIdentity {
    EvidenceMarketIdentity::try_new(
        market_id.to_string(),
        format!("condition-{market_id}"),
        format!("question-{market_id}"),
        [
            outcome(0, "up", &format!("up-{market_id}")),
            outcome(1, "down", &format!("down-{market_id}")),
        ],
    )
    .expect("complete stable market identity should construct")
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
    for (field, market) in [
        (
            "Gamma market",
            EvidenceMarketIdentity::try_new(
                "market-a-changed".to_string(),
                "condition-market-a".to_string(),
                "question-market-a".to_string(),
                [
                    outcome(0, "up", "up-market-a"),
                    outcome(1, "down", "down-market-a"),
                ],
            )
            .expect("changed market id remains valid"),
        ),
        (
            "condition",
            EvidenceMarketIdentity::try_new(
                "market-a".to_string(),
                "condition-market-a-changed".to_string(),
                "question-market-a".to_string(),
                [
                    outcome(0, "up", "up-market-a"),
                    outcome(1, "down", "down-market-a"),
                ],
            )
            .expect("changed condition remains valid"),
        ),
        (
            "question",
            EvidenceMarketIdentity::try_new(
                "market-a".to_string(),
                "condition-market-a".to_string(),
                "question-market-a-changed".to_string(),
                [
                    outcome(0, "up", "up-market-a"),
                    outcome(1, "down", "down-market-a"),
                ],
            )
            .expect("changed question remains valid"),
        ),
        (
            "normalized outcome",
            EvidenceMarketIdentity::try_new(
                "market-a".to_string(),
                "condition-market-a".to_string(),
                "question-market-a".to_string(),
                [
                    outcome(0, "up-changed", "up-market-a"),
                    outcome(1, "down", "down-market-a"),
                ],
            )
            .expect("changed outcome remains valid"),
        ),
        (
            "CLOB token",
            EvidenceMarketIdentity::try_new(
                "market-a".to_string(),
                "condition-market-a".to_string(),
                "question-market-a".to_string(),
                [
                    outcome(0, "up", "up-market-a-changed"),
                    outcome(1, "down", "down-market-a"),
                ],
            )
            .expect("changed token remains valid"),
        ),
    ] {
        let mut parts = episode_parts("market-a");
        parts.market = market;
        mutations.push((field, parts));
    }

    for (field, parts) in mutations {
        let changed = EvidenceEpisodeId::try_from(parts).expect("changed identity remains valid");
        assert_ne!(baseline, changed, "changing {field} must change identity");
    }
}

#[test]
fn negative_risk_selection_metadata_does_not_split_episode_identity() {
    let selected = |negative_risk| {
        SelectedMarketEvidenceIdentity::try_new(
            "market-a".to_string(),
            "condition-market-a".to_string(),
            "question-market-a".to_string(),
            negative_risk,
            [
                SelectedMarketEvidenceOutcome {
                    index: 0,
                    normalized_outcome: "up".to_string(),
                    clob_token_id: "up-market-a".to_string(),
                },
                SelectedMarketEvidenceOutcome {
                    index: 1,
                    normalized_outcome: "down".to_string(),
                    clob_token_id: "down-market-a".to_string(),
                },
            ],
        )
        .expect("selection identity should be valid")
    };
    let episode_for = |identity: &SelectedMarketEvidenceIdentity| {
        EvidenceEpisodeId::try_from(EvidenceEpisodeParts {
            strategy_id: "binary-oracle-btc".to_string(),
            target_id: "btc-updown-5m".to_string(),
            venue_id: "POLYMARKET".to_string(),
            market: identity.market().clone(),
        })
        .expect("selected market should construct an episode")
    };

    assert_eq!(episode_for(&selected(false)), episode_for(&selected(true)));
}

#[test]
fn invalid_ordered_outcome_identity_is_rejected() {
    let candidate = |outcomes| {
        EvidenceMarketIdentity::try_new(
            "market-a".to_string(),
            "condition-market-a".to_string(),
            "question-market-a".to_string(),
            outcomes,
        )
    };
    assert!(candidate([outcome(2, "up", "up"), outcome(1, "down", "down")]).is_err());
    assert!(candidate([outcome(1, "up", "up"), outcome(0, "down", "down")]).is_err());
    assert!(candidate([outcome(0, "up", "up"), outcome(0, "down", "down")]).is_err());
    assert!(candidate([outcome(0, "same", "up"), outcome(1, "same", "down")]).is_err());
    assert!(candidate([outcome(0, "up", "same"), outcome(1, "down", "same")]).is_err());
}

#[test]
fn blank_or_whitespace_padded_stable_identity_fields_are_rejected() {
    let candidate = |market: &str, condition: &str, first_token: &str| {
        EvidenceMarketIdentity::try_new(
            market.to_string(),
            condition.to_string(),
            "question-market-a".to_string(),
            [
                outcome(0, "up", first_token),
                outcome(1, "down", "down-market-a"),
            ],
        )
    };
    assert!(candidate("   ", "condition-market-a", "up-market-a").is_err());
    assert!(candidate("market-a", " condition-market-a", "up-market-a").is_err());
    assert!(candidate("market-a", "condition-market-a", "up-market-a ").is_err());
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
