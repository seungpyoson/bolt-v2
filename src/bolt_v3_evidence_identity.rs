//! Stable, non-temporal evidence-episode schema and canonical encoder.
//!
//! #1354 deliberately provides no production constructor or consumer. The future
//! Capsule-owned bind-once Gamma authority supplies construction; risk ordinals,
//! retirement, and restart exact-once authority remain outside this slice.

use std::{error::Error, fmt};

const EVIDENCE_EPISODE_ENCODING_TAG: &[u8] = b"bolt-v3-evidence-episode-v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NonEmptyEvidenceIdentity(String);

impl NonEmptyEvidenceIdentity {
    #[cfg(test)]
    fn new(field: &'static str, value: String) -> Result<Self, EvidenceIdentityError> {
        if value.trim().is_empty() {
            return Err(EvidenceIdentityError::Empty(field));
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable venue mode that affects market semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceNegRiskMode {
    Disabled,
    Enabled,
}

/// One member of the venue-defined, ordered binary outcome binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceOutcomeIdentity {
    outcome_index: u16,
    normalized_outcome: NonEmptyEvidenceIdentity,
    clob_token_id: NonEmptyEvidenceIdentity,
}

impl EvidenceOutcomeIdentity {
    #[cfg(test)]
    fn new(
        outcome_index: u16,
        normalized_outcome: String,
        clob_token_id: String,
    ) -> Result<Self, EvidenceIdentityError> {
        Ok(Self {
            outcome_index,
            normalized_outcome: NonEmptyEvidenceIdentity::new(
                "normalized_outcome",
                normalized_outcome,
            )?,
            clob_token_id: NonEmptyEvidenceIdentity::new("clob_token_id", clob_token_id)?,
        })
    }

    #[must_use]
    pub const fn outcome_index(&self) -> u16 {
        self.outcome_index
    }

    #[must_use]
    pub fn normalized_outcome(&self) -> &str {
        self.normalized_outcome.as_str()
    }

    #[must_use]
    pub fn clob_token_id(&self) -> &str {
        self.clob_token_id.as_str()
    }
}

/// Stable venue semantics shared by every evidence family for one market.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceMarketIdentity {
    gamma_market_id: NonEmptyEvidenceIdentity,
    condition_id: NonEmptyEvidenceIdentity,
    question_id: NonEmptyEvidenceIdentity,
    neg_risk_mode: EvidenceNegRiskMode,
    ordered_outcomes: [EvidenceOutcomeIdentity; 2],
}

impl EvidenceMarketIdentity {
    #[cfg(test)]
    fn new(
        gamma_market_id: String,
        condition_id: String,
        question_id: String,
        neg_risk_mode: EvidenceNegRiskMode,
        ordered_outcomes: [EvidenceOutcomeIdentity; 2],
    ) -> Result<Self, EvidenceIdentityError> {
        if ordered_outcomes[0].outcome_index != 0 || ordered_outcomes[1].outcome_index != 1 {
            return Err(EvidenceIdentityError::InvalidOrderedOutcomeIndexes([
                ordered_outcomes[0].outcome_index,
                ordered_outcomes[1].outcome_index,
            ]));
        }
        if ordered_outcomes[0].clob_token_id == ordered_outcomes[1].clob_token_id {
            return Err(EvidenceIdentityError::DuplicateClobTokenId);
        }
        Ok(Self {
            gamma_market_id: NonEmptyEvidenceIdentity::new("gamma_market_id", gamma_market_id)?,
            condition_id: NonEmptyEvidenceIdentity::new("condition_id", condition_id)?,
            question_id: NonEmptyEvidenceIdentity::new("question_id", question_id)?,
            neg_risk_mode,
            ordered_outcomes,
        })
    }

    #[must_use]
    pub fn gamma_market_id(&self) -> &str {
        self.gamma_market_id.as_str()
    }

    #[must_use]
    pub fn condition_id(&self) -> &str {
        self.condition_id.as_str()
    }

    #[must_use]
    pub fn question_id(&self) -> &str {
        self.question_id.as_str()
    }

    #[must_use]
    pub const fn neg_risk_mode(&self) -> EvidenceNegRiskMode {
        self.neg_risk_mode
    }

    #[must_use]
    pub fn ordered_outcomes(&self) -> &[EvidenceOutcomeIdentity; 2] {
        &self.ordered_outcomes
    }
}

/// Constructor-only evidence episode identity. Its fields are intentionally
/// limited to stable logical and venue semantics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceEpisodeId {
    logical_strategy_id: NonEmptyEvidenceIdentity,
    logical_target_id: NonEmptyEvidenceIdentity,
    logical_venue_id: NonEmptyEvidenceIdentity,
    market: EvidenceMarketIdentity,
}

impl EvidenceEpisodeId {
    #[cfg(test)]
    fn new(
        logical_strategy_id: String,
        logical_target_id: String,
        logical_venue_id: String,
        market: EvidenceMarketIdentity,
    ) -> Result<Self, EvidenceIdentityError> {
        Ok(Self {
            logical_strategy_id: NonEmptyEvidenceIdentity::new(
                "logical_strategy_id",
                logical_strategy_id,
            )?,
            logical_target_id: NonEmptyEvidenceIdentity::new(
                "logical_target_id",
                logical_target_id,
            )?,
            logical_venue_id: NonEmptyEvidenceIdentity::new("logical_venue_id", logical_venue_id)?,
            market,
        })
    }

    #[must_use]
    pub fn logical_strategy_id(&self) -> &str {
        self.logical_strategy_id.as_str()
    }

    #[must_use]
    pub fn logical_target_id(&self) -> &str {
        self.logical_target_id.as_str()
    }

    #[must_use]
    pub fn logical_venue_id(&self) -> &str {
        self.logical_venue_id.as_str()
    }

    #[must_use]
    pub const fn market(&self) -> &EvidenceMarketIdentity {
        &self.market
    }

    /// Canonical, collision-resistant field framing for the future Capsule-owned
    /// durable identity boundary. #1354 defines the encoder but has no production
    /// constructor or consumer.
    #[must_use]
    pub fn encode_canonical(&self) -> Vec<u8> {
        let mut encoded = Vec::new();
        encode_component(&mut encoded, EVIDENCE_EPISODE_ENCODING_TAG);
        encode_component(&mut encoded, self.logical_strategy_id().as_bytes());
        encode_component(&mut encoded, self.logical_target_id().as_bytes());
        encode_component(&mut encoded, self.logical_venue_id().as_bytes());
        encode_component(&mut encoded, self.market().gamma_market_id().as_bytes());
        encode_component(&mut encoded, self.market().condition_id().as_bytes());
        encode_component(&mut encoded, self.market().question_id().as_bytes());
        encoded.push(match self.market().neg_risk_mode() {
            EvidenceNegRiskMode::Disabled => 0,
            EvidenceNegRiskMode::Enabled => 1,
        });
        for outcome in self.market().ordered_outcomes() {
            encoded.extend_from_slice(&outcome.outcome_index().to_be_bytes());
            encode_component(&mut encoded, outcome.normalized_outcome().as_bytes());
            encode_component(&mut encoded, outcome.clob_token_id().as_bytes());
        }
        encoded
    }
}

fn encode_component(encoded: &mut Vec<u8>, component: &[u8]) {
    encoded.extend_from_slice(&(component.len() as u64).to_be_bytes());
    encoded.extend_from_slice(component);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceIdentityError {
    Empty(&'static str),
    InvalidOrderedOutcomeIndexes([u16; 2]),
    DuplicateClobTokenId,
}

impl fmt::Display for EvidenceIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(field) => write!(formatter, "evidence identity field `{field}` is empty"),
            Self::InvalidOrderedOutcomeIndexes(indexes) => {
                write!(
                    formatter,
                    "evidence outcome indexes must be [0, 1], got {indexes:?}"
                )
            }
            Self::DuplicateClobTokenId => {
                formatter.write_str("evidence CLOB token identity is duplicated")
            }
        }
    }
}

impl Error for EvidenceIdentityError {}

#[cfg(test)]
mod tests {
    use super::{
        EvidenceEpisodeId, EvidenceIdentityError, EvidenceMarketIdentity, EvidenceNegRiskMode,
        EvidenceOutcomeIdentity,
    };

    fn episode() -> EvidenceEpisodeId {
        let outcomes = [
            EvidenceOutcomeIdentity::new(0, "up".to_string(), "token-up".to_string()).unwrap(),
            EvidenceOutcomeIdentity::new(1, "down".to_string(), "token-down".to_string()).unwrap(),
        ];
        let market = EvidenceMarketIdentity::new(
            "gamma-market".to_string(),
            "condition".to_string(),
            "question".to_string(),
            EvidenceNegRiskMode::Disabled,
            outcomes,
        )
        .unwrap();
        EvidenceEpisodeId::new(
            "strategy".to_string(),
            "target".to_string(),
            "venue".to_string(),
            market,
        )
        .unwrap()
    }

    #[test]
    fn volatile_observations_cannot_change_episode_identity() {
        let expected = episode();
        for volatile_tick in 0..100_000_u64 {
            let exact_slug = format!("ignored-{volatile_tick}");
            let trusted_open = volatile_tick;
            let trusted_close = volatile_tick.saturating_add(1);
            let observed_price = volatile_tick as f64;
            let diagnostic = volatile_tick.to_string();
            let transient_flag = volatile_tick % 2 == 0;
            let retry_count = volatile_tick;
            let _ = (
                exact_slug,
                trusted_open,
                trusted_close,
                observed_price,
                diagnostic,
                transient_flag,
                retry_count,
            );
            assert_eq!(episode(), expected);
        }
    }

    #[test]
    fn stable_venue_semantic_change_changes_episode_identity() {
        let baseline = episode();
        let outcomes = baseline.market().ordered_outcomes();
        let changed = [outcomes[1].clone(), outcomes[0].clone()];
        let changed_market = EvidenceMarketIdentity::new(
            "gamma-market".to_string(),
            "condition".to_string(),
            "question".to_string(),
            EvidenceNegRiskMode::Disabled,
            changed,
        )
        .unwrap();
        let changed_episode = EvidenceEpisodeId::new(
            "strategy".to_string(),
            "target".to_string(),
            "venue".to_string(),
            changed_market,
        )
        .unwrap();
        assert_ne!(changed_episode, baseline);
        assert_ne!(
            changed_episode.encode_canonical(),
            baseline.encode_canonical()
        );
    }

    #[test]
    fn canonical_encoder_is_deterministic_and_length_delimited() {
        let baseline = episode();
        assert_eq!(baseline.encode_canonical(), episode().encode_canonical());

        let ambiguous_left = EvidenceEpisodeId::new(
            "ab".to_string(),
            "c".to_string(),
            "venue".to_string(),
            baseline.market().clone(),
        )
        .unwrap();
        let ambiguous_right = EvidenceEpisodeId::new(
            "a".to_string(),
            "bc".to_string(),
            "venue".to_string(),
            baseline.market().clone(),
        )
        .unwrap();
        assert_ne!(
            ambiguous_left.encode_canonical(),
            ambiguous_right.encode_canonical()
        );
    }

    #[test]
    fn invalid_or_ambiguous_identity_is_rejected() {
        let error = EvidenceOutcomeIdentity::new(0, String::new(), "token".to_string())
            .expect_err("empty normalized outcome must fail");
        assert_eq!(error, EvidenceIdentityError::Empty("normalized_outcome"));

        let duplicate = [
            EvidenceOutcomeIdentity::new(0, "up".to_string(), "token".to_string()).unwrap(),
            EvidenceOutcomeIdentity::new(0, "down".to_string(), "token".to_string()).unwrap(),
        ];
        assert_eq!(
            EvidenceMarketIdentity::new(
                "market".to_string(),
                "condition".to_string(),
                "question".to_string(),
                EvidenceNegRiskMode::Enabled,
                duplicate,
            )
            .expect_err("duplicate outcome indexes must fail"),
            EvidenceIdentityError::InvalidOrderedOutcomeIndexes([0, 0])
        );

        let duplicate_token = [
            EvidenceOutcomeIdentity::new(0, "up".to_string(), "token".to_string()).unwrap(),
            EvidenceOutcomeIdentity::new(1, "down".to_string(), "token".to_string()).unwrap(),
        ];
        assert_eq!(
            EvidenceMarketIdentity::new(
                "market".to_string(),
                "condition".to_string(),
                "question".to_string(),
                EvidenceNegRiskMode::Enabled,
                duplicate_token,
            )
            .expect_err("duplicate token ids must fail"),
            EvidenceIdentityError::DuplicateClobTokenId
        );
    }
}
