//! Stable market-episode identity and finite in-process semantic novelty.
//!
//! This is the safe pre-Capsule slice from #1354. It deliberately provides no
//! restart exact-once claim and owns no persistence. Within one fully identified
//! market episode, however, a registered producer's semantic state can be emitted
//! at most once. The set never evicts and never resets on time or input churn.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

include!("bolt_v3_evidence_novelty_generated.rs");

/// The complete non-temporal input surface from which a market episode may be built.
///
/// Prices, timestamps, ages, counters, feed flags, slugs, and diagnostics are
/// structurally absent. Ordered outcome/token identity is represented by the
/// explicit up/down token fields rather than a caller-supplied collection.
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
        if let Some((field, _)) = required.iter().find(|(_, value)| value.is_empty()) {
            bail!("evidence episode requires non-empty stable field `{field}`");
        }
        if parts.outcomes[0].index == parts.outcomes[1].index {
            bail!("evidence episode requires distinct ordered outcome indices");
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

/// Finite monotonic novelty for one registered producer and current market episode.
///
/// A genuine episode change replaces the current set. Within an episode, states are
/// only inserted: writer failure, oscillation, and volatile input churn never remove
/// a state. Capacity exhaustion rejects the unseen state before invoking `emit`.
pub struct EvidenceNoveltyGuard<State> {
    registration: &'static EvidenceStateRegistration,
    current_episode: Option<EvidenceEpisodeId>,
    seen_states: BTreeSet<State>,
}

impl<State> EvidenceNoveltyGuard<State>
where
    State: Ord,
{
    pub fn for_owner(owner: EvidenceStateOwner) -> Result<Self> {
        let registration = evidence_state_registration(owner);
        Ok(Self {
            registration,
            current_episode: None,
            seen_states: BTreeSet::new(),
        })
    }

    /// Mark `state` before payload construction and append.
    ///
    /// Returns `Ok(false)` for a duplicate without invoking `emit`. A failed emit
    /// remains seen so a broken telemetry sink cannot turn into a retry storm.
    pub fn emit_once(
        &mut self,
        episode: &EvidenceEpisodeId,
        state: State,
        emit: impl FnOnce() -> Result<()>,
    ) -> Result<bool> {
        if !self.claim_once(episode, state)? {
            return Ok(false);
        }
        emit()?;
        Ok(true)
    }

    /// Claim a semantic state before its caller constructs a payload.
    pub fn claim_once(&mut self, episode: &EvidenceEpisodeId, state: State) -> Result<bool> {
        if self.current_episode.as_ref() != Some(episode) {
            self.current_episode = Some(episode.clone());
            self.seen_states.clear();
        }
        if self.seen_states.contains(&state) {
            return Ok(false);
        }
        if self.seen_states.len() >= self.registration.state_capacity {
            bail!(
                "evidence novelty capacity exhausted for registered state {}.{}",
                self.registration.producer_kind,
                self.registration.semantic_state
            );
        }
        self.seen_states.insert(state);
        Ok(true)
    }

    #[must_use]
    pub fn seen_state_count(&self) -> usize {
        self.seen_states.len()
    }

    #[must_use]
    pub fn state_capacity(&self) -> usize {
        self.registration.state_capacity
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

#[must_use]
pub const fn total_owned_state_capacity() -> usize {
    EVIDENCE_NOVELTY_OWNED_STATE_CAPACITY
}
