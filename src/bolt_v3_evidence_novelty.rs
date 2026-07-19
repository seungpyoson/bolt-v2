//! Stable market-episode identity and finite in-process semantic novelty.
//!
//! This is a pre-Capsule slice of #1354. It makes no restart exact-once claim.
//! Within one fully identified market episode, a registered non-recovery
//! producer's semantic state is emitted at most once. Seen state never evicts or
//! resets on time, input churn, or a later market episode.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

#[rustfmt::skip]
mod generated;

pub use generated::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceOutcomeIdentity {
    pub index: u8,
    pub normalized_outcome: String,
    pub instrument_id: String,
}

/// Complete stable input surface for a market evidence episode.
///
/// Prices, timestamps, slugs, windows, diagnostics, transient flags, config or
/// deploy revisions, and retries are structurally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceEpisodeParts {
    pub strategy_id: String,
    pub target_id: String,
    pub venue_id: String,
    pub market_id: String,
    pub condition_id: String,
    pub question_id: String,
    pub outcomes: [EvidenceOutcomeIdentity; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceEpisodeId {
    strategy_id: String,
    target_id: String,
    venue_id: String,
    market_id: String,
    condition_id: String,
    question_id: String,
    outcomes: [EvidenceOutcomeIdentity; 2],
}

fn stable_field_is_canonical(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

impl TryFrom<EvidenceEpisodeParts> for EvidenceEpisodeId {
    type Error = anyhow::Error;

    fn try_from(parts: EvidenceEpisodeParts) -> Result<Self> {
        let [first_outcome, second_outcome] = &parts.outcomes;
        let required = [
            parts.strategy_id.as_str(),
            parts.target_id.as_str(),
            parts.venue_id.as_str(),
            parts.market_id.as_str(),
            parts.condition_id.as_str(),
            parts.question_id.as_str(),
            first_outcome.normalized_outcome.as_str(),
            first_outcome.instrument_id.as_str(),
            second_outcome.normalized_outcome.as_str(),
            second_outcome.instrument_id.as_str(),
        ];
        if required
            .iter()
            .any(|value| !stable_field_is_canonical(value))
        {
            bail!("evidence episode requires non-empty, unpadded stable fields");
        }
        if parts.outcomes[0].index != 0 || parts.outcomes[1].index != 1 {
            bail!("evidence episode requires canonical ordered outcome indices 0 and 1");
        }
        if parts.outcomes[0].normalized_outcome == parts.outcomes[1].normalized_outcome {
            bail!("evidence episode requires distinct normalized outcomes");
        }
        if parts.outcomes[0].instrument_id == parts.outcomes[1].instrument_id {
            bail!("evidence episode requires distinct outcome instruments");
        }
        Ok(Self {
            strategy_id: parts.strategy_id,
            target_id: parts.target_id,
            venue_id: parts.venue_id,
            market_id: parts.market_id,
            condition_id: parts.condition_id,
            question_id: parts.question_id,
            outcomes: parts.outcomes,
        })
    }
}

pub struct EvidenceNoveltyGuard<EpisodeId = EvidenceEpisodeId> {
    owner: EvidenceStateOwner,
    seen_by_episode: BTreeMap<EpisodeId, Vec<u64>>,
}

impl<EpisodeId> EvidenceNoveltyGuard<EpisodeId>
where
    EpisodeId: Clone + Ord,
{
    #[must_use]
    pub fn for_owner(owner: EvidenceStateOwner) -> Self {
        Self {
            owner,
            seen_by_episode: BTreeMap::new(),
        }
    }

    /// Claims before invoking the writer. Writer failure therefore stays seen.
    pub fn emit_once(
        &mut self,
        episode: &EpisodeId,
        state: EvidenceCanonicalState,
        emit: impl FnOnce() -> Result<()>,
    ) -> Result<bool> {
        if !self.claim_once(episode, state)? {
            return Ok(false);
        }
        emit()?;
        Ok(true)
    }

    pub fn claim_once(
        &mut self,
        episode: &EpisodeId,
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

    pub fn has_claimed(&self, episode: &EpisodeId, state: EvidenceCanonicalState) -> Result<bool> {
        let (word, mask) = self.state_bit(state)?;
        Ok(self
            .seen_by_episode
            .get(episode)
            .is_some_and(|words| words[word] & mask == mask))
    }

    #[must_use]
    pub fn seen_episode_count(&self) -> usize {
        self.seen_by_episode.len()
    }

    #[must_use]
    pub fn seen_state_count(&self, episode: &EpisodeId) -> usize {
        self.seen_by_episode
            .get(episode)
            .map(|words| words.iter().map(|word| word.count_ones() as usize).sum())
            .unwrap_or_default()
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
        let word_bits = u64::BITS as usize;
        let word = registration.id / word_bits;
        let mask = 1_u64 << (registration.id % word_bits);
        Ok((word, mask))
    }
}

pub fn registered_evidence_state_by_id(id: usize) -> Result<&'static EvidenceStateRegistration> {
    evidence_state_registration_by_id(id)
        .ok_or_else(|| anyhow::anyhow!("unregistered evidence semantic state id {id}"))
}
