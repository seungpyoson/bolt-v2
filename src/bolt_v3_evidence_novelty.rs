//! Stable market-episode identity and finite in-process semantic novelty.
//!
//! This is the safe pre-Capsule slice of #1354. It makes no restart exact-once
//! claim. Each approved non-recovery producer owns one complete typed key. A
//! key is attempted at most once per fully identified market episode, and seen
//! state never evicts or resets because of time, input churn, or later episodes.
//! TOML-generated formulas report the finite per-episode state bound, including
//! the registered RV source roster. Each retained key corresponds to at most one
//! writer attempt, so novelty memory cannot grow faster than attempted evidence
//! within an episode. Episode count remains monotone and process memory is not
//! globally bounded; restart re-emits current states. Retirement, persistence,
//! and restart exact-once remain owned by #1385.

use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

use anyhow::{Result, bail, ensure};
use nautilus_model::identifiers::{InstrumentId, Venue};

use crate::{
    bolt_v3_decision_evidence::BoltV3OutcomeSide,
    bolt_v3_realized_volatility::{
        RealizedVolBlockReason, RealizedVolSourceRejectReason, RealizedVolSourceStatus,
    },
};

pub mod generator;

#[rustfmt::skip]
mod generated;

pub use generated::*;

pub(crate) mod private {
    pub trait Sealed {}
}

pub trait NoveltyEligibleProducer: private::Sealed {
    type Key: Clone + Ord;

    const OWNER: EvidenceProducerOwner;
    const PRODUCER_KIND: &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvidenceEpisodeRejection {
    StrategyIdentityMissing,
    StrategyIdentityNonCanonical,
    TargetIdentityMissing,
    TargetIdentityNonCanonical,
    MarketIdentityMissing,
    MarketIdentityNonCanonical,
    ConditionIdentityMissing,
    ConditionIdentityNonCanonical,
    QuestionIdentityMissing,
    QuestionIdentityNonCanonical,
    SelectedMarketSourceIdentityMissing,
    UpOutcomeInstrumentMissing,
    DownOutcomeInstrumentMissing,
    OutcomeOrderInvalid,
    OutcomeInstrumentDuplicate,
    OutcomeVenueMismatch,
}

macro_rules! stable_identity {
    ($name:ident, $missing:ident, $noncanonical:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, EvidenceEpisodeRejection> {
                let value = value.into();
                if value.is_empty() {
                    return Err(EvidenceEpisodeRejection::$missing);
                }
                if value.trim() != value {
                    return Err(EvidenceEpisodeRejection::$noncanonical);
                }
                Ok(Self(value))
            }
        }
    };
}

stable_identity!(
    EvidenceStrategyIdentity,
    StrategyIdentityMissing,
    StrategyIdentityNonCanonical
);
stable_identity!(
    EvidenceTargetIdentity,
    TargetIdentityMissing,
    TargetIdentityNonCanonical
);
stable_identity!(
    EvidenceMarketIdentity,
    MarketIdentityMissing,
    MarketIdentityNonCanonical
);
stable_identity!(
    EvidenceConditionIdentity,
    ConditionIdentityMissing,
    ConditionIdentityNonCanonical
);
stable_identity!(
    EvidenceQuestionIdentity,
    QuestionIdentityMissing,
    QuestionIdentityNonCanonical
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceOutcomeIdentity {
    side: BoltV3OutcomeSide,
    instrument_id: InstrumentId,
}

impl EvidenceOutcomeIdentity {
    #[must_use]
    pub const fn new(side: BoltV3OutcomeSide, instrument_id: InstrumentId) -> Self {
        Self {
            side,
            instrument_id,
        }
    }
}

/// Complete stable identity for one logical binary-market episode.
///
/// No constructor parameter can carry a price, timestamp, slug, window,
/// diagnostic, config/deploy revision, retry count, or capacity policy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceEpisodeId {
    strategy: EvidenceStrategyIdentity,
    target: EvidenceTargetIdentity,
    venue: Venue,
    market: EvidenceMarketIdentity,
    condition: EvidenceConditionIdentity,
    question: EvidenceQuestionIdentity,
    outcomes: [EvidenceOutcomeIdentity; 2],
}

impl EvidenceEpisodeId {
    pub fn try_binary_market(
        strategy: EvidenceStrategyIdentity,
        target: EvidenceTargetIdentity,
        venue: Venue,
        market: EvidenceMarketIdentity,
        condition: EvidenceConditionIdentity,
        question: EvidenceQuestionIdentity,
        outcomes: [EvidenceOutcomeIdentity; 2],
    ) -> Result<Self, EvidenceEpisodeRejection> {
        if outcomes[0].side != BoltV3OutcomeSide::Up || outcomes[1].side != BoltV3OutcomeSide::Down
        {
            return Err(EvidenceEpisodeRejection::OutcomeOrderInvalid);
        }
        if outcomes[0].instrument_id == outcomes[1].instrument_id {
            return Err(EvidenceEpisodeRejection::OutcomeInstrumentDuplicate);
        }
        if outcomes
            .iter()
            .any(|outcome| outcome.instrument_id.venue != venue)
        {
            return Err(EvidenceEpisodeRejection::OutcomeVenueMismatch);
        }
        Ok(Self {
            strategy,
            target,
            venue,
            market,
            condition,
            question,
            outcomes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalSet<T>(BTreeSet<T>);

impl<T: Ord> CanonicalSet<T> {
    pub fn try_from_iter(values: impl IntoIterator<Item = T>) -> Result<Self> {
        let mut canonical = BTreeSet::new();
        for value in values {
            ensure!(
                canonical.insert(value),
                "semantic state contains a duplicate set component"
            );
        }
        Ok(Self(canonical))
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegisteredRvSourceId(String);

impl RegisteredRvSourceId {
    fn try_new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        ensure!(
            !value.is_empty() && value.trim() == value,
            "RV source identity must be non-empty and unpadded"
        );
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RvSourceSemanticStateInput {
    pub source_id: String,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub status: RealizedVolSourceStatus,
    pub block_reason: Option<RealizedVolBlockReason>,
    pub last_rejected_reason: Option<RealizedVolSourceRejectReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RvSourceSemanticState {
    enablement: EvidenceEnablement,
    quorum_participation: EvidenceQuorumParticipation,
    status: RealizedVolSourceStatus,
    block_reason: Option<RealizedVolBlockReason>,
    last_rejected_reason: Option<RealizedVolSourceRejectReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalSourceStates(BTreeMap<RegisteredRvSourceId, RvSourceSemanticState>);

impl CanonicalSourceStates {
    pub fn try_new(
        registered_source_ids: impl IntoIterator<Item = String>,
        states: impl IntoIterator<Item = RvSourceSemanticStateInput>,
        unknown_source_ids: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        if unknown_source_ids.into_iter().next().is_some() {
            bail!("RV semantic state contains an unregistered source identity");
        }
        let mut registered = BTreeSet::new();
        for source_id in registered_source_ids {
            let source_id = RegisteredRvSourceId::try_new(source_id)?;
            ensure!(
                registered.insert(source_id),
                "RV source roster contains a duplicate identity"
            );
        }
        let mut canonical = BTreeMap::new();
        for state in states {
            let source_id = RegisteredRvSourceId::try_new(state.source_id)?;
            ensure!(
                registered.contains(&source_id),
                "RV semantic state contains a source outside the registered roster"
            );
            generated::validate_rv_source_status(&state.status)?;
            if let Some(block_reason) = &state.block_reason {
                generated::validate_rv_blocker(block_reason)?;
            }
            if let Some(last_rejected_reason) = &state.last_rejected_reason {
                generated::validate_rv_source_rejection(last_rejected_reason)?;
            }
            let semantic_state = RvSourceSemanticState {
                enablement: state.enabled.into(),
                quorum_participation: state.counts_toward_quorum.into(),
                status: state.status,
                block_reason: state.block_reason,
                last_rejected_reason: state.last_rejected_reason,
            };
            ensure!(
                canonical.insert(source_id, semantic_state).is_none(),
                "RV semantic state contains duplicate diagnostics for one source"
            );
        }
        ensure!(
            canonical.len() == registered.len(),
            "RV semantic state must cover every registered source exactly once"
        );
        Ok(Self(canonical))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug)]
pub enum EvidenceAttemptOutcome {
    Appended,
    PreviouslyAttempted,
    AttemptFailedAndRetained(anyhow::Error),
    IdentityRejectedFirst(EvidenceEpisodeRejection),
    IdentityRejectedPreviously(EvidenceEpisodeRejection),
    SemanticKeyRejectedFirst(anyhow::Error),
    SemanticKeyRejectedPreviously,
}

pub struct EvidenceNoveltyGuard<P: NoveltyEligibleProducer> {
    seen_by_episode: BTreeMap<EvidenceEpisodeId, BTreeSet<P::Key>>,
    rejected_identities: BTreeSet<EvidenceEpisodeRejection>,
    rejected_semantic_key_episodes: BTreeSet<EvidenceEpisodeId>,
    marker: PhantomData<P>,
}

impl<P: NoveltyEligibleProducer> EvidenceNoveltyGuard<P> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen_by_episode: BTreeMap::new(),
            rejected_identities: BTreeSet::new(),
            rejected_semantic_key_episodes: BTreeSet::new(),
            marker: PhantomData,
        }
    }

    /// Marks the complete key before invoking the writer and bounds invalid
    /// identity or semantic projections without retaining their free text.
    ///
    /// A failed writer therefore remains attempted and cannot cause a per-tick
    /// retry flood. The returned outcome distinguishes failure from a durable
    /// append and never describes the failed attempt as appended.
    pub fn attempt_once(
        &mut self,
        episode: Result<EvidenceEpisodeId, EvidenceEpisodeRejection>,
        key: Result<P::Key>,
        emit: impl FnOnce() -> Result<()>,
    ) -> EvidenceAttemptOutcome {
        let episode = match episode {
            Ok(episode) => episode,
            Err(rejection) => {
                return if self.rejected_identities.insert(rejection) {
                    EvidenceAttemptOutcome::IdentityRejectedFirst(rejection)
                } else {
                    EvidenceAttemptOutcome::IdentityRejectedPreviously(rejection)
                };
            }
        };
        let key = match key {
            Ok(key) => key,
            Err(error) => {
                return if self.rejected_semantic_key_episodes.insert(episode) {
                    EvidenceAttemptOutcome::SemanticKeyRejectedFirst(error)
                } else {
                    EvidenceAttemptOutcome::SemanticKeyRejectedPreviously
                };
            }
        };
        if !self.seen_by_episode.entry(episode).or_default().insert(key) {
            return EvidenceAttemptOutcome::PreviouslyAttempted;
        }
        match emit() {
            Ok(()) => EvidenceAttemptOutcome::Appended,
            Err(error) => EvidenceAttemptOutcome::AttemptFailedAndRetained(error),
        }
    }

    #[must_use]
    pub fn seen_episode_count(&self) -> usize {
        self.seen_by_episode.len()
    }

    #[must_use]
    pub fn seen_state_count(&self, episode: &EvidenceEpisodeId) -> usize {
        self.seen_by_episode
            .get(episode)
            .map(BTreeSet::len)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn rejected_identity_count(&self) -> usize {
        self.rejected_identities.len()
    }

    #[must_use]
    pub fn rejected_semantic_key_episode_count(&self) -> usize {
        self.rejected_semantic_key_episodes.len()
    }
}

impl<P: NoveltyEligibleProducer> Default for EvidenceNoveltyGuard<P> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn registered_evidence_dimension_by_id(
    id: usize,
) -> Result<&'static EvidenceDimensionRegistration> {
    evidence_dimension_registration_by_id(id)
        .ok_or_else(|| anyhow::anyhow!("unregistered evidence semantic dimension id {id}"))
}
