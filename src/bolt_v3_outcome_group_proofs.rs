//! Provider-native outcome-group proof payloads.
//!
//! This is a cycle-free leaf: proof payload data and provider-native identity
//! formatting live here without importing outcome-group core or provider
//! modules. The closed `GroupingProof` sum type remains in outcome-group core.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketDiscoveryScopeEvidence {
    pub source_id: String,
    pub event_slugs: Vec<String>,
    pub market_slugs: Vec<String>,
    pub gamma_query_fingerprint: Option<String>,
    pub cache_key_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegRiskGroupingProof {
    pub neg_risk_market_id: String,
    pub discovery_scope: PolymarketDiscoveryScopeEvidence,
    pub market_slugs: Vec<String>,
    pub proof_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredOutcomeGroupingProof {
    pub question: u32,
    pub outcome_indices: Vec<u32>,
    pub proof_fingerprint: String,
}

impl NegRiskGroupingProof {
    pub fn native_identity(&self) -> String {
        format!("polymarket:{}", self.neg_risk_market_id)
    }
}

impl StructuredOutcomeGroupingProof {
    pub fn native_identity(&self) -> String {
        format!("hyperliquid:{}", self.question)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neg_risk_payload_formats_native_identity() {
        let proof = NegRiskGroupingProof {
            neg_risk_market_id: "market-1".to_string(),
            discovery_scope: PolymarketDiscoveryScopeEvidence {
                source_id: "source-1".to_string(),
                event_slugs: Vec::new(),
                market_slugs: Vec::new(),
                gamma_query_fingerprint: None,
                cache_key_fingerprint: "c".repeat(64),
            },
            market_slugs: vec!["a".to_string(), "b".to_string()],
            proof_fingerprint: "p".repeat(64),
        };

        assert_eq!(proof.native_identity(), "polymarket:market-1");
    }

    #[test]
    fn structured_outcome_payload_formats_native_identity() {
        let proof = StructuredOutcomeGroupingProof {
            question: 42,
            outcome_indices: vec![3, 7],
            proof_fingerprint: "f".repeat(64),
        };

        assert_eq!(proof.native_identity(), "hyperliquid:42");
    }
}
