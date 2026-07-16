//! Stable market-episode identity and finite in-process semantic novelty.
//!
//! This is the safe pre-Capsule slice from #1354. It deliberately provides no
//! restart exact-once claim and owns no persistence. Within one fully identified
//! market episode, however, a registered producer's semantic state can be emitted
//! at most once. The set never evicts and never resets on time or input churn.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::bolt_v3_target_identity::stable_identity_field_is_canonical;

#[rustfmt::skip]
mod generated;

pub use generated::*;

/// The complete non-temporal input surface from which a market episode may be built.
///
/// Prices, timestamps, ages, counters, feed flags, slugs, and diagnostics are
/// structurally absent. Ordered outcome/token identity is represented by a fixed
/// two-element array whose coordinates must be exactly `0` then `1`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceOutcomeIdentity {
    pub index: u8,
    pub normalized_outcome: String,
    pub clob_token_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceEpisodeParts {
    pub strategy_id: String,
    pub target_id: String,
    pub venue_id: String,
    pub gamma_market_id: String,
    pub condition_id: String,
    pub question_id: String,
    pub negative_risk: bool,
    pub outcomes: [EvidenceOutcomeIdentity; 2],
}

/// Typed identity for one stable market episode.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceEpisodeId {
    strategy_id: String,
    target_id: String,
    venue_id: String,
    gamma_market_id: String,
    condition_id: String,
    question_id: String,
    negative_risk: bool,
    outcomes: [EvidenceOutcomeIdentity; 2],
}

impl TryFrom<EvidenceEpisodeParts> for EvidenceEpisodeId {
    type Error = anyhow::Error;

    fn try_from(parts: EvidenceEpisodeParts) -> Result<Self> {
        let required = [
            ("strategy_id", parts.strategy_id.as_str()),
            ("target_id", parts.target_id.as_str()),
            ("venue_id", parts.venue_id.as_str()),
            ("gamma_market_id", parts.gamma_market_id.as_str()),
            ("condition_id", parts.condition_id.as_str()),
            ("question_id", parts.question_id.as_str()),
            (
                "outcomes[0].normalized_outcome",
                parts.outcomes[0].normalized_outcome.as_str(),
            ),
            (
                "outcomes[0].clob_token_id",
                parts.outcomes[0].clob_token_id.as_str(),
            ),
            (
                "outcomes[1].normalized_outcome",
                parts.outcomes[1].normalized_outcome.as_str(),
            ),
            (
                "outcomes[1].clob_token_id",
                parts.outcomes[1].clob_token_id.as_str(),
            ),
        ];
        if let Some((field, _)) = required
            .iter()
            .find(|(_, value)| !stable_identity_field_is_canonical(value))
        {
            bail!("evidence episode requires non-empty, unpadded stable field `{field}`");
        }
        if parts.outcomes[0].index != 0 || parts.outcomes[1].index != 1 {
            bail!("evidence episode requires canonical ordered outcome indices 0 and 1");
        }
        if parts.outcomes[0].normalized_outcome == parts.outcomes[1].normalized_outcome {
            bail!("evidence episode requires distinct normalized outcomes");
        }
        if parts.outcomes[0].clob_token_id == parts.outcomes[1].clob_token_id {
            bail!("evidence episode requires distinct CLOB token identities");
        }
        Ok(Self {
            strategy_id: parts.strategy_id,
            target_id: parts.target_id,
            venue_id: parts.venue_id,
            gamma_market_id: parts.gamma_market_id,
            condition_id: parts.condition_id,
            question_id: parts.question_id,
            negative_risk: parts.negative_risk,
            outcomes: parts.outcomes,
        })
    }
}

/// Finite monotonic novelty for one registered producer across market episodes.
///
/// Every episode retains a fixed-size bitmap for the lifetime of this guard. Writer
/// failure, oscillation, volatile input churn, and later episodes never remove a
/// claimed canonical state.
pub struct EvidenceNoveltyGuard {
    owner: EvidenceStateOwner,
    seen_by_episode: BTreeMap<EvidenceEpisodeId, Vec<u64>>,
}

impl EvidenceNoveltyGuard {
    pub fn for_owner(owner: EvidenceStateOwner) -> Result<Self> {
        Ok(Self {
            owner,
            seen_by_episode: BTreeMap::new(),
        })
    }

    /// Mark `state` before payload construction and append.
    ///
    /// Returns `Ok(false)` for a duplicate without invoking `emit`. A failed emit
    /// remains seen so a broken telemetry sink cannot turn into a retry storm.
    pub fn emit_once(
        &mut self,
        episode: &EvidenceEpisodeId,
        state: EvidenceCanonicalState,
        emit: impl FnOnce() -> Result<()>,
    ) -> Result<bool> {
        if !self.claim_once(episode, state)? {
            return Ok(false);
        }
        emit()?;
        Ok(true)
    }

    /// Claim a semantic state before its caller constructs a payload.
    pub fn claim_once(
        &mut self,
        episode: &EvidenceEpisodeId,
        state: EvidenceCanonicalState,
    ) -> Result<bool> {
        let (word, mask) = self.state_bit(state)?;
        let words = self
            .seen_by_episode
            .entry(episode.clone())
            .or_insert_with(|| vec![0; EVIDENCE_NOVELTY_WORD_COUNT]);
        if words[word] & mask != 0 {
            return Ok(false);
        }
        words[word] |= mask;
        Ok(true)
    }

    /// Check a registered state without creating or mutating an episode bitmap.
    pub fn has_claimed(
        &self,
        episode: &EvidenceEpisodeId,
        state: EvidenceCanonicalState,
    ) -> Result<bool> {
        let (word, mask) = self.state_bit(state)?;
        Ok(self
            .seen_by_episode
            .get(episode)
            .is_some_and(|words| words[word] & mask == mask))
    }

    fn state_bit(&self, state: EvidenceCanonicalState) -> Result<(usize, u64)> {
        let registration = canonical_state_registration(state);
        if registration.owner != self.owner {
            bail!(
                "evidence novelty owner mismatch for registered state {}.{}",
                registration.producer_kind,
                registration.semantic_state
            );
        }
        if registration.id >= EVIDENCE_NOVELTY_FAMILY_CAPACITY {
            bail!("evidence novelty state id exceeds the registered family capacity");
        }
        let word_bits = u64::BITS as usize;
        let word = registration.id / word_bits;
        let mask = 1_u64 << (registration.id % word_bits);
        Ok((word, mask))
    }

    #[must_use]
    pub fn seen_episode_count(&self) -> usize {
        self.seen_by_episode.len()
    }

    #[must_use]
    pub fn seen_state_count(&self, episode: &EvidenceEpisodeId) -> usize {
        self.seen_by_episode
            .get(episode)
            .map(|words| words.iter().map(|word| word.count_ones() as usize).sum())
            .unwrap_or_default()
    }
}

pub fn registered_evidence_state(
    producer_kind: &str,
    semantic_state: &str,
) -> Result<&'static EvidenceStateRegistration> {
    EVIDENCE_STATE_REGISTRATIONS
        .iter()
        .find(|registration| {
            registration.producer_kind == producer_kind
                && registration.semantic_state == semantic_state
        })
        .ok_or_else(|| {
            anyhow::anyhow!("unregistered evidence semantic state {producer_kind}.{semantic_state}")
        })
}

pub fn registered_evidence_state_by_id(id: usize) -> Result<&'static EvidenceStateRegistration> {
    evidence_state_registration_by_id(id)
        .ok_or_else(|| anyhow::anyhow!("unregistered evidence semantic state id {id}"))
}
