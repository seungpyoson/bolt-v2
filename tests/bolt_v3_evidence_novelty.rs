use std::cell::Cell;

use anyhow::Result;
use bolt_v2::bolt_v3_evidence_novelty::{
    EvidenceEpisodeId, EvidenceEpisodeParts, EvidenceNoveltyGuard, EvidenceStateOwner,
    registered_evidence_state, total_owned_state_capacity,
};

fn episode(market_id: &str) -> EvidenceEpisodeId {
    EvidenceEpisodeId::try_from(EvidenceEpisodeParts {
        strategy_id: "binary-oracle-btc".to_string(),
        target_id: "btc-updown-5m".to_string(),
        venue_id: "POLYMARKET".to_string(),
        market_id: market_id.to_string(),
        condition_id: format!("condition-{market_id}"),
        question_id: format!("question-{market_id}"),
        up_token_id: format!("up-{market_id}"),
        down_token_id: format!("down-{market_id}"),
    })
    .expect("complete stable market identity should construct an episode")
}

#[test]
fn stable_episode_identity_has_no_volatile_constructor_inputs() {
    let first = episode("market-a");

    // These are intentionally not accepted by EvidenceEpisodeParts. Changing every
    // incident-driving volatile observation therefore cannot re-key the episode.
    let first_volatile_sample = (101_000.0, 1_u64, 2_u64, 3_u64, false, "missing_snapshot");
    let second_volatile_sample = (
        99_000.0,
        999_999_u64,
        88_888_u64,
        77_777_u64,
        true,
        "rejected_stale",
    );

    let second = episode("market-a");
    assert_eq!(first, second);
    assert_ne!(first_volatile_sample, second_volatile_sample);
}

#[test]
fn true_market_change_changes_episode_identity() {
    assert_ne!(episode("market-a"), episode("market-b"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TestState {
    A,
    B,
}

#[test]
fn one_hundred_thousand_a_b_a_oscillations_reach_a_fixed_ceiling() -> Result<()> {
    let episode = episode("market-a");
    let mut entry_skip =
        EvidenceNoveltyGuard::<TestState>::for_owner(EvidenceStateOwner::EntrySkip)?;
    let mut blocked_snapshot = EvidenceNoveltyGuard::<TestState>::for_owner(
        EvidenceStateOwner::BlockedStrategyInputSnapshot,
    )?;
    let entry_payloads = Cell::new(0_u64);
    let entry_appends = Cell::new(0_u64);
    let snapshot_payloads = Cell::new(0_u64);
    let snapshot_appends = Cell::new(0_u64);

    for _ in 0..100_000 {
        for state in [TestState::A, TestState::B, TestState::A] {
            entry_skip.emit_once(&episode, state, || {
                entry_payloads.set(entry_payloads.get() + 1);
                entry_appends.set(entry_appends.get() + 1);
                Ok(())
            })?;
            blocked_snapshot.emit_once(&episode, state, || {
                snapshot_payloads.set(snapshot_payloads.get() + 1);
                snapshot_appends.set(snapshot_appends.get() + 1);
                Ok(())
            })?;
        }
    }

    assert_eq!(entry_payloads.get(), 2);
    assert_eq!(entry_appends.get(), 2);
    assert_eq!(snapshot_payloads.get(), 2);
    assert_eq!(snapshot_appends.get(), 2);
    assert_eq!(entry_skip.seen_state_count(), 2);
    assert_eq!(blocked_snapshot.seen_state_count(), 2);
    assert_eq!(
        total_owned_state_capacity(),
        entry_skip.state_capacity() + blocked_snapshot.state_capacity()
    );
    Ok(())
}

#[test]
fn duplicate_rejection_skips_payload_construction_and_append() -> Result<()> {
    let episode = episode("market-a");
    let mut guard = EvidenceNoveltyGuard::<TestState>::for_owner(EvidenceStateOwner::EntrySkip)?;
    let payloads = Cell::new(0_u64);
    let appends = Cell::new(0_u64);

    for _ in 0..10 {
        guard.emit_once(&episode, TestState::A, || {
            payloads.set(payloads.get() + 1);
            appends.set(appends.get() + 1);
            Ok(())
        })?;
    }

    assert_eq!(payloads.get(), 1);
    assert_eq!(appends.get(), 1);
    Ok(())
}

#[test]
fn unknown_semantic_state_mapping_fails_closed() {
    assert!(registered_evidence_state("entry_skip", "unknown_state").is_err());
    assert!(registered_evidence_state("unknown_producer", "entry_skip.semantic").is_err());
}
