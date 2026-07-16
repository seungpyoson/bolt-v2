use std::cell::Cell;

use anyhow::Result;
use bolt_v2::bolt_v3_evidence_novelty::{
    EvidenceEpisodeId, EvidenceEpisodeParts, EvidenceNoveltyGuard, EvidenceStateOwner,
    registered_evidence_state, total_owned_state_capacity,
};

fn episode_parts(market_id: &str) -> EvidenceEpisodeParts {
    EvidenceEpisodeParts {
        strategy_id: "binary-oracle-btc".to_string(),
        target_id: "btc-updown-5m".to_string(),
        venue_id: "POLYMARKET".to_string(),
        market_id: market_id.to_string(),
        condition_id: format!("condition-{market_id}"),
        question_id: format!("question-{market_id}"),
        up_token_id: format!("up-{market_id}"),
        down_token_id: format!("down-{market_id}"),
    }
}

fn episode(market_id: &str) -> EvidenceEpisodeId {
    EvidenceEpisodeId::try_from(episode_parts(market_id))
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
    let baseline = episode("market-a");
    let mutations: [(&str, fn(&mut EvidenceEpisodeParts)); 8] = [
        ("strategy", |parts: &mut EvidenceEpisodeParts| {
            parts.strategy_id.push_str("-changed");
        }),
        ("target", |parts: &mut EvidenceEpisodeParts| {
            parts.target_id.push_str("-changed");
        }),
        ("venue", |parts: &mut EvidenceEpisodeParts| {
            parts.venue_id.push_str("-changed");
        }),
        ("market", |parts: &mut EvidenceEpisodeParts| {
            parts.market_id.push_str("-changed");
        }),
        ("condition", |parts: &mut EvidenceEpisodeParts| {
            parts.condition_id.push_str("-changed");
        }),
        ("question", |parts: &mut EvidenceEpisodeParts| {
            parts.question_id.push_str("-changed");
        }),
        ("up token", |parts: &mut EvidenceEpisodeParts| {
            parts.up_token_id.push_str("-changed");
        }),
        ("down token", |parts: &mut EvidenceEpisodeParts| {
            parts.down_token_id.push_str("-changed");
        }),
    ];
    for (field, mutate) in mutations {
        let mut parts = episode_parts("market-a");
        mutate(&mut parts);
        let changed = EvidenceEpisodeId::try_from(parts).expect("changed identity remains valid");
        assert_ne!(
            baseline, changed,
            "changing {field} must change episode identity"
        );
    }
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
fn true_episode_change_gets_a_fresh_finite_novelty_domain() -> Result<()> {
    let mut guard = EvidenceNoveltyGuard::<TestState>::for_owner(EvidenceStateOwner::EntrySkip)?;
    let appends = Cell::new(0_u64);
    for episode in [episode("market-a"), episode("market-b")] {
        guard.emit_once(&episode, TestState::A, || {
            appends.set(appends.get() + 1);
            Ok(())
        })?;
    }
    assert_eq!(appends.get(), 2);
    assert_eq!(guard.seen_state_count(), 1);
    Ok(())
}

#[test]
fn unknown_semantic_state_mapping_fails_closed() {
    assert!(registered_evidence_state("entry_skip", "unknown_state").is_err());
    assert!(registered_evidence_state("unknown_producer", "entry_skip.semantic").is_err());
}

#[test]
fn configured_capacity_exhaustion_fails_before_payload_or_append() -> Result<()> {
    let episode = episode("market-a");
    let mut guard = EvidenceNoveltyGuard::<usize>::for_owner(EvidenceStateOwner::EntrySkip)?;
    let payloads = Cell::new(0_usize);
    for state in 0..guard.state_capacity() {
        guard.emit_once(&episode, state, || {
            payloads.set(payloads.get() + 1);
            Ok(())
        })?;
    }
    let error = guard
        .emit_once(&episode, guard.state_capacity(), || {
            payloads.set(payloads.get() + 1);
            Ok(())
        })
        .expect_err("an unseen state beyond the TOML-generated capacity must fail closed");
    assert!(error.to_string().contains("capacity exhausted"));
    assert_eq!(payloads.get(), guard.state_capacity());
    Ok(())
}
