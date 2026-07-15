use zeroize::Zeroize;

use super::nonce::SafeNonce;

const ADDRESS_BYTES: usize = 20;
const WORD_BYTES: usize = 32;

/// Capsule-owned exclusive snapshot of one condition and its exact pre-state.
///
/// This linear value has no production constructor in AO-REDEEM. AO-CAPSULE will
/// mint it after durably acquiring the condition mutation lease.
pub struct ExactConditionSnapshotLease {
    condition_id: [u8; WORD_BYTES],
    pre_claim_balances: [[u8; WORD_BYTES]; 2],
    pre_collateral_balance: [u8; WORD_BYTES],
    expected_redeemed_collateral_balance: [u8; WORD_BYTES],
    snapshot_generation: u64,
}

/// Capsule-owned reservation of the account-global Safe nonce and both bodies.
///
/// The permit is consumed by request preparation, so two conditions cannot use
/// the same reservation. Its production issuer belongs to AO-CAPSULE.
pub struct SafeNonceBodyCapacityPermit {
    safe_address: [u8; ADDRESS_BYTES],
    safe_nonce: SafeNonce,
    original_body_capacity: usize,
    fence_body_capacity: usize,
    lane_generation: u64,
}

/// Fresh Capsule validation performed immediately before original authorization.
pub struct FreshPreSendValidation {
    action_binding: [u8; WORD_BYTES],
    snapshot_generation: u64,
    lane_generation: u64,
}

/// Quorum-durable evidence that the exact original body may have started.
pub struct OriginalMayHaveStartedPermit {
    action_binding: [u8; WORD_BYTES],
    original_body_hash: [u8; WORD_BYTES],
    durable_generation: u64,
}

/// Quorum-durable evidence that the exact same-nonce fence may have started.
///
/// The original hash is part of the permit so a fence can only follow the exact
/// original lineage committed by the Capsule.
pub struct FenceMayHaveStartedPermit {
    action_binding: [u8; WORD_BYTES],
    original_body_hash: [u8; WORD_BYTES],
    fence_body_hash: [u8; WORD_BYTES],
    durable_generation: u64,
}

impl ExactConditionSnapshotLease {
    pub(super) fn parts(
        &self,
    ) -> (
        [u8; WORD_BYTES],
        [[u8; WORD_BYTES]; 2],
        [u8; WORD_BYTES],
        [u8; WORD_BYTES],
        u64,
    ) {
        (
            self.condition_id,
            self.pre_claim_balances,
            self.pre_collateral_balance,
            self.expected_redeemed_collateral_balance,
            self.snapshot_generation,
        )
    }
}

impl SafeNonceBodyCapacityPermit {
    pub(super) fn parts(&self) -> ([u8; ADDRESS_BYTES], SafeNonce, usize, usize, u64) {
        (
            self.safe_address,
            self.safe_nonce,
            self.original_body_capacity,
            self.fence_body_capacity,
            self.lane_generation,
        )
    }
}

impl FreshPreSendValidation {
    pub(super) fn matches(
        &self,
        action_binding: [u8; WORD_BYTES],
        snapshot_generation: u64,
        lane_generation: u64,
    ) -> bool {
        self.action_binding == action_binding
            && self.snapshot_generation == snapshot_generation
            && self.lane_generation == lane_generation
    }
}

impl OriginalMayHaveStartedPermit {
    pub(super) fn matches(
        &self,
        action_binding: [u8; WORD_BYTES],
        original_body_hash: [u8; WORD_BYTES],
    ) -> bool {
        self.action_binding == action_binding
            && self.original_body_hash == original_body_hash
            && self.durable_generation != 0
    }
}

impl FenceMayHaveStartedPermit {
    pub(super) fn matches(
        &self,
        action_binding: [u8; WORD_BYTES],
        original_body_hash: [u8; WORD_BYTES],
        fence_body_hash: [u8; WORD_BYTES],
    ) -> bool {
        self.action_binding == action_binding
            && self.original_body_hash == original_body_hash
            && self.fence_body_hash == fence_body_hash
            && self.durable_generation != 0
    }
}

macro_rules! zeroize_on_drop {
    ($type:ty, $($field:ident),+ $(,)?) => {
        impl Drop for $type {
            fn drop(&mut self) {
                $(self.$field.zeroize();)+
            }
        }
    };
}

zeroize_on_drop!(
    ExactConditionSnapshotLease,
    condition_id,
    pre_claim_balances,
    pre_collateral_balance,
    expected_redeemed_collateral_balance,
    snapshot_generation
);
zeroize_on_drop!(
    SafeNonceBodyCapacityPermit,
    safe_address,
    safe_nonce,
    original_body_capacity,
    fence_body_capacity,
    lane_generation
);
zeroize_on_drop!(
    FreshPreSendValidation,
    action_binding,
    snapshot_generation,
    lane_generation
);
zeroize_on_drop!(
    OriginalMayHaveStartedPermit,
    action_binding,
    original_body_hash,
    durable_generation
);
zeroize_on_drop!(
    FenceMayHaveStartedPermit,
    action_binding,
    original_body_hash,
    fence_body_hash,
    durable_generation
);

#[cfg(test)]
pub(super) mod hermetic {
    use super::*;

    pub(super) fn snapshot(
        condition_id: [u8; WORD_BYTES],
        pre_claim_balances: [[u8; WORD_BYTES]; 2],
        pre_collateral_balance: [u8; WORD_BYTES],
        expected_redeemed_collateral_balance: [u8; WORD_BYTES],
        snapshot_generation: u64,
    ) -> ExactConditionSnapshotLease {
        ExactConditionSnapshotLease {
            condition_id,
            pre_claim_balances,
            pre_collateral_balance,
            expected_redeemed_collateral_balance,
            snapshot_generation,
        }
    }

    pub(super) fn nonce_capacity(
        safe_address: [u8; ADDRESS_BYTES],
        safe_nonce: SafeNonce,
        original_body_capacity: usize,
        fence_body_capacity: usize,
        lane_generation: u64,
    ) -> SafeNonceBodyCapacityPermit {
        SafeNonceBodyCapacityPermit {
            safe_address,
            safe_nonce,
            original_body_capacity,
            fence_body_capacity,
            lane_generation,
        }
    }

    pub(super) fn fresh(
        action_binding: [u8; WORD_BYTES],
        snapshot_generation: u64,
        lane_generation: u64,
    ) -> FreshPreSendValidation {
        FreshPreSendValidation {
            action_binding,
            snapshot_generation,
            lane_generation,
        }
    }

    pub(super) fn original_durable(
        action_binding: [u8; WORD_BYTES],
        original_body_hash: [u8; WORD_BYTES],
        durable_generation: u64,
    ) -> OriginalMayHaveStartedPermit {
        OriginalMayHaveStartedPermit {
            action_binding,
            original_body_hash,
            durable_generation,
        }
    }

    pub(super) fn fence_durable(
        action_binding: [u8; WORD_BYTES],
        original_body_hash: [u8; WORD_BYTES],
        fence_body_hash: [u8; WORD_BYTES],
        durable_generation: u64,
    ) -> FenceMayHaveStartedPermit {
        FenceMayHaveStartedPermit {
            action_binding,
            original_body_hash,
            fence_body_hash,
            durable_generation,
        }
    }
}
