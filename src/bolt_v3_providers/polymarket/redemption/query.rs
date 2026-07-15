use super::request::PreparedRequestPair;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactQuery {
    RelayerTransaction {
        path: String,
        transaction_id: String,
    },
    SafeNonce {
        safe_address: String,
        calldata: String,
    },
    SafeExecution {
        safe_address: String,
        safe_transaction_hash: [u8; 32],
    },
    FinalizedReceipt {
        chain_transaction_hash: [u8; 32],
        required_confirmations: u64,
        max_logs: usize,
    },
    RawPostState {
        target: String,
        condition_id: [u8; 32],
        expected_pre_balances: [[u8; 32]; 2],
    },
    SafeBoundary {
        safe_address: String,
        implementation: String,
        fallback_handler: String,
        guard: String,
        modules: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactQuerySet {
    pub queries: Vec<ExactQuery>,
}

impl ExactQuerySet {
    pub fn for_response_loss(
        profile: &super::config::ValidatedRedemptionProfile,
        pair: &PreparedRequestPair,
        relayer_transaction_id: Option<&str>,
    ) -> Self {
        let mut queries = Vec::with_capacity(6);
        if let Some(transaction_id) = relayer_transaction_id {
            queries.push(ExactQuery::RelayerTransaction {
                path: profile.config.relayer.transaction_path.clone(),
                transaction_id: transaction_id.to_string(),
            });
        }
        queries.extend([
            ExactQuery::SafeNonce {
                safe_address: profile.config.wallet.safe_address.clone(),
                calldata: profile.manifest.safe.nonce_selector.clone(),
            },
            ExactQuery::SafeExecution {
                safe_address: profile.config.wallet.safe_address.clone(),
                safe_transaction_hash: pair.original.safe_transaction_hash(),
            },
            ExactQuery::SafeExecution {
                safe_address: profile.config.wallet.safe_address.clone(),
                safe_transaction_hash: pair.fence.safe_transaction_hash(),
            },
            ExactQuery::RawPostState {
                target: pair.original.identity().target.clone(),
                condition_id: pair.condition_id,
                expected_pre_balances: pair.pre_balances,
            },
            ExactQuery::SafeBoundary {
                safe_address: profile.config.wallet.safe_address.clone(),
                implementation: profile.config.wallet.safe_implementation.clone(),
                fallback_handler: profile.config.wallet.fallback_handler.clone(),
                guard: profile.config.wallet.guard.clone(),
                modules: profile.config.wallet.modules.clone(),
            },
        ]);
        Self { queries }
    }

    pub fn finalized_receipt(
        profile: &super::config::ValidatedRedemptionProfile,
        chain_transaction_hash: [u8; 32],
    ) -> ExactQuery {
        ExactQuery::FinalizedReceipt {
            chain_transaction_hash,
            required_confirmations: profile.config.rpc.finality_confirmations,
            max_logs: profile.config.rpc.max_receipt_logs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionProof {
    pub safe_transaction_hash: [u8; 32],
    pub finalized: bool,
    pub safe_execution_succeeded: bool,
    pub compatible_logs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostStateRelation {
    Redeemed,
    Unchanged,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionObservation {
    pub prepared_nonce: u64,
    pub on_chain_nonce: u64,
    pub original_safe_transaction_hash: [u8; 32],
    pub fence_safe_transaction_hash: [u8; 32],
    pub original_execution: Option<ExecutionProof>,
    pub fence_execution: Option<ExecutionProof>,
    pub post_state: PostStateRelation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionResolution {
    Unresolved,
    RedemptionFinalized,
    PermanentlyFencedNoEffect,
    IntegrityFailure,
}

pub fn resolve_competing_nonce(observation: &ResolutionObservation) -> RedemptionResolution {
    if observation.on_chain_nonce < observation.prepared_nonce {
        return RedemptionResolution::IntegrityFailure;
    }
    if observation.on_chain_nonce == observation.prepared_nonce {
        return if observation.original_execution.is_none()
            && observation.fence_execution.is_none()
            && observation.post_state != PostStateRelation::Drifted
        {
            RedemptionResolution::Unresolved
        } else {
            RedemptionResolution::IntegrityFailure
        };
    }
    let Some(expected_next_nonce) = observation.prepared_nonce.checked_add(1) else {
        return RedemptionResolution::IntegrityFailure;
    };
    if observation.on_chain_nonce != expected_next_nonce {
        return RedemptionResolution::IntegrityFailure;
    }
    match (
        observation.original_execution.as_ref(),
        observation.fence_execution.as_ref(),
    ) {
        (Some(original), None)
            if proof_matches(original, observation.original_safe_transaction_hash)
                && observation.post_state == PostStateRelation::Redeemed =>
        {
            RedemptionResolution::RedemptionFinalized
        }
        (None, Some(fence))
            if proof_matches(fence, observation.fence_safe_transaction_hash)
                && observation.post_state == PostStateRelation::Unchanged =>
        {
            RedemptionResolution::PermanentlyFencedNoEffect
        }
        _ => RedemptionResolution::IntegrityFailure,
    }
}

fn proof_matches(proof: &ExecutionProof, expected_hash: [u8; 32]) -> bool {
    proof.safe_transaction_hash == expected_hash
        && proof.finalized
        && proof.safe_execution_succeeded
        && proof.compatible_logs
}
