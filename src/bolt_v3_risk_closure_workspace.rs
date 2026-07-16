//! Fixed-capacity memory workspaces retained across risk closure.

use std::{
    collections::BTreeMap,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Mutex},
};

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RiskClosureWorkspaceConfig {
    arena_bytes: usize,
    slot_bytes: usize,
    capacity: usize,
    production_activation_enabled: bool,
}

#[cfg(test)]
include!("bolt_v3_risk_closure_workspace_generated.rs");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClosureIdentity(String);

impl ClosureIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, ClosureIdentityError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ClosureIdentityError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureIdentityError {
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskClosureWorkspaceError {
    InvalidConfiguration,
    AllocationFailed,
    CapacityExhausted,
    DuplicateClosureIdentity,
    UnknownClosureIdentity,
    ClosureAlreadyCheckedOut,
    LeaseIdentityMismatch,
    InvalidLease,
    LeaseIdExhausted,
    StatePoisoned,
}

#[derive(Debug)]
pub(super) struct RiskClosureWorkspaceAuthority {
    inner: Arc<Mutex<WorkspaceState>>,
}

impl RiskClosureWorkspaceAuthority {
    #[cfg(test)]
    pub(super) fn for_disabled_application_resource_ledger()
    -> Result<Self, RiskClosureWorkspaceError> {
        Self::with_config(RISK_CLOSURE_WORKSPACE_CONFIG)
    }

    #[cfg(test)]
    fn with_config(config: RiskClosureWorkspaceConfig) -> Result<Self, RiskClosureWorkspaceError> {
        Ok(Self {
            inner: Arc::new(Mutex::new(WorkspaceState::allocate(config)?)),
        })
    }

    pub(super) fn checkout_new_risk(
        &self,
    ) -> Result<RiskClosureWorkspaceReservation, RiskClosureWorkspaceError> {
        let (slot_index, lease_id) = {
            let mut state = self
                .inner
                .lock()
                .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
            let slot_index = state
                .slots
                .iter()
                .position(|slot| matches!(slot, SlotState::Free))
                .ok_or(RiskClosureWorkspaceError::CapacityExhausted)?;
            let lease_id = state.take_lease_id()?;
            state.slots[slot_index] = SlotState::Reserved { lease_id };
            (slot_index, lease_id)
        };
        let mut reservation = RiskClosureWorkspaceReservation {
            inner: Arc::clone(&self.inner),
            slot_index,
            lease_id,
            active: true,
            not_clone: PhantomData,
        };
        reservation.clear_workspace()?;
        Ok(reservation)
    }

    pub(super) fn checkout_recovery(
        &self,
        closure_identity: &ClosureIdentity,
    ) -> Result<RiskClosureWorkspaceLease, RiskClosureWorkspaceError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        let slot_index = *state
            .logical_slots
            .get(closure_identity)
            .ok_or(RiskClosureWorkspaceError::UnknownClosureIdentity)?;
        let closure_generation = match &state.slots[slot_index] {
            SlotState::RetainedIdle {
                closure_identity: retained,
                closure_generation,
            } if retained == closure_identity => *closure_generation,
            SlotState::RecoveryCheckedOut { .. } => {
                return Err(RiskClosureWorkspaceError::ClosureAlreadyCheckedOut);
            }
            _ => return Err(RiskClosureWorkspaceError::InvalidLease),
        };
        let lease_id = state.take_lease_id()?;
        state.slots[slot_index] = SlotState::RecoveryCheckedOut {
            closure_identity: closure_identity.clone(),
            closure_generation,
            lease_id,
        };
        Ok(RiskClosureWorkspaceLease {
            inner: Arc::clone(&self.inner),
            authority_identity: state.authority_identity.clone(),
            closure_identity: closure_identity.clone(),
            closure_generation,
            slot_index,
            lease_id,
            active: true,
            not_clone: PhantomData,
        })
    }

    #[cfg(test)]
    fn reserved_bytes(&self) -> Result<usize, RiskClosureWorkspaceError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        Ok(state.storage.reserved_bytes())
    }
}

#[cfg(test)]
pub(super) fn configured_capacity() -> usize {
    RISK_CLOSURE_WORKSPACE_CONFIG.capacity
}

#[derive(Debug, Clone)]
struct AuthorityIdentity(Arc<()>);

impl AuthorityIdentity {
    #[cfg(test)]
    fn new() -> Self {
        Self(Arc::new(()))
    }

    fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug)]
struct WorkspaceState {
    authority_identity: AuthorityIdentity,
    slot_bytes: usize,
    storage: Arc<WorkspaceStorage>,
    slots: Vec<SlotState>,
    logical_slots: BTreeMap<ClosureIdentity, usize>,
    next_lease_id: u64,
}

impl WorkspaceState {
    #[cfg(test)]
    fn allocate(config: RiskClosureWorkspaceConfig) -> Result<Self, RiskClosureWorkspaceError> {
        if config.arena_bytes == usize::default()
            || config.slot_bytes == usize::default()
            || config.capacity == usize::default()
            || config.arena_bytes % config.slot_bytes != usize::default()
            || config.arena_bytes / config.slot_bytes != config.capacity
            || config.production_activation_enabled
        {
            return Err(RiskClosureWorkspaceError::InvalidConfiguration);
        }
        let storage = Arc::new(WorkspaceStorage::allocate(config)?);
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(config.capacity)
            .map_err(|_| RiskClosureWorkspaceError::AllocationFailed)?;
        slots.resize_with(config.capacity, || SlotState::Free);
        Ok(Self {
            authority_identity: AuthorityIdentity::new(),
            slot_bytes: config.slot_bytes,
            storage,
            slots,
            logical_slots: BTreeMap::new(),
            next_lease_id: u64::default(),
        })
    }

    fn take_lease_id(&mut self) -> Result<u64, RiskClosureWorkspaceError> {
        self.next_lease_id = self
            .next_lease_id
            .checked_add(u64::from(true))
            .ok_or(RiskClosureWorkspaceError::LeaseIdExhausted)?;
        Ok(self.next_lease_id)
    }
}

#[derive(Debug)]
struct WorkspaceStorage {
    slots: Box<[Mutex<Box<[u8]>>]>,
    reserved_bytes: usize,
}

impl WorkspaceStorage {
    #[cfg(test)]
    fn allocate(config: RiskClosureWorkspaceConfig) -> Result<Self, RiskClosureWorkspaceError> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(config.capacity)
            .map_err(|_| RiskClosureWorkspaceError::AllocationFailed)?;
        for _ in usize::default()..config.capacity {
            let mut workspace = Vec::new();
            workspace
                .try_reserve_exact(config.slot_bytes)
                .map_err(|_| RiskClosureWorkspaceError::AllocationFailed)?;
            workspace.resize(config.slot_bytes, u8::MAX);
            slots.push(Mutex::new(workspace.into_boxed_slice()));
        }
        Ok(Self {
            slots: slots.into_boxed_slice(),
            reserved_bytes: config.arena_bytes,
        })
    }

    fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }

    fn clear_slot(&self, slot_index: usize) -> Result<(), RiskClosureWorkspaceError> {
        let mut workspace = self
            .slots
            .get(slot_index)
            .ok_or(RiskClosureWorkspaceError::InvalidLease)?
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        workspace.fill(u8::default());
        Ok(())
    }

    fn with_slot_mut<T>(
        &self,
        slot_index: usize,
        use_workspace: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<T, RiskClosureWorkspaceError> {
        let mut workspace = self
            .slots
            .get(slot_index)
            .ok_or(RiskClosureWorkspaceError::InvalidLease)?
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        let outcome = catch_unwind(AssertUnwindSafe(|| use_workspace(&mut workspace)));
        drop(workspace);
        match outcome {
            Ok(value) => Ok(value),
            Err(payload) => resume_unwind(payload),
        }
    }
}

#[derive(Debug)]
enum SlotState {
    Free,
    Reserved {
        lease_id: u64,
    },
    RetainedIdle {
        closure_identity: ClosureIdentity,
        closure_generation: u64,
    },
    RecoveryCheckedOut {
        closure_identity: ClosureIdentity,
        closure_generation: u64,
        lease_id: u64,
    },
}

/// An uncommitted reservation over one real workspace slot.
///
/// The reservation is intentionally neither `Clone` nor `Copy`.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_application_resource_ledger::RiskClosureWorkspaceReservation;
///
/// fn cannot_clone(reservation: RiskClosureWorkspaceReservation) {
///     let _duplicate = reservation.clone();
/// }
/// ```
///
/// Commit consumes the reservation, so it cannot be reused.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_application_resource_ledger::{
///     ClosureIdentity, RiskClosureWorkspaceReservation,
/// };
///
/// fn cannot_reuse(reservation: RiskClosureWorkspaceReservation, identity: ClosureIdentity) {
///     reservation.commit(identity).unwrap();
///     let _ = reservation.workspace_len();
/// }
/// ```
pub struct RiskClosureWorkspaceReservation {
    inner: Arc<Mutex<WorkspaceState>>,
    slot_index: usize,
    lease_id: u64,
    active: bool,
    not_clone: PhantomData<Mutex<()>>,
}

impl RiskClosureWorkspaceReservation {
    pub fn workspace_len(&self) -> Result<usize, RiskClosureWorkspaceError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        Ok(state.slot_bytes)
    }

    pub fn with_workspace_mut<T>(
        &mut self,
        use_workspace: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<T, RiskClosureWorkspaceError> {
        let storage = self.validated_storage()?;
        storage.with_slot_mut(self.slot_index, use_workspace)
    }

    pub fn commit(
        mut self,
        closure_identity: ClosureIdentity,
    ) -> Result<(), RiskClosureWorkspaceError> {
        let inner = Arc::clone(&self.inner);
        let mut state = inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        if state.logical_slots.contains_key(&closure_identity) {
            state.slots[self.slot_index] = SlotState::Free;
            self.active = false;
            return Err(RiskClosureWorkspaceError::DuplicateClosureIdentity);
        }
        state
            .logical_slots
            .insert(closure_identity.clone(), self.slot_index);
        state.slots[self.slot_index] = SlotState::RetainedIdle {
            closure_identity,
            closure_generation: self.lease_id,
        };
        self.active = false;
        Ok(())
    }

    fn validate(&self, state: &WorkspaceState) -> Result<(), RiskClosureWorkspaceError> {
        match state.slots.get(self.slot_index) {
            Some(SlotState::Reserved { lease_id }) if *lease_id == self.lease_id => Ok(()),
            _ => Err(RiskClosureWorkspaceError::InvalidLease),
        }
    }

    fn validated_storage(&self) -> Result<Arc<WorkspaceStorage>, RiskClosureWorkspaceError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        Ok(Arc::clone(&state.storage))
    }

    fn clear_workspace(&mut self) -> Result<(), RiskClosureWorkspaceError> {
        let storage = self.validated_storage()?;
        storage.clear_slot(self.slot_index)
    }
}

impl Drop for RiskClosureWorkspaceReservation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        if matches!(
            state.slots.get(self.slot_index),
            Some(SlotState::Reserved { lease_id }) if *lease_id == self.lease_id
        ) {
            state.slots[self.slot_index] = SlotState::Free;
        }
    }
}

/// Exclusive access to a workspace retained for recovery.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_application_resource_ledger::RiskClosureWorkspaceLease;
///
/// fn cannot_clone(lease: RiskClosureWorkspaceLease) {
///     let _duplicate = lease.clone();
/// }
/// ```
///
/// Terminal release consumes both the lease and its permit.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_application_resource_ledger::{
///     RiskClosureWorkspaceLease, TerminalReleasePermit,
/// };
///
/// fn cannot_reuse(lease: RiskClosureWorkspaceLease, permit: TerminalReleasePermit) {
///     lease.release_terminal(permit).unwrap();
///     let _ = lease.workspace_len();
/// }
/// ```
pub struct RiskClosureWorkspaceLease {
    inner: Arc<Mutex<WorkspaceState>>,
    authority_identity: AuthorityIdentity,
    closure_identity: ClosureIdentity,
    closure_generation: u64,
    slot_index: usize,
    lease_id: u64,
    active: bool,
    not_clone: PhantomData<Mutex<()>>,
}

impl RiskClosureWorkspaceLease {
    pub fn workspace_len(&self) -> Result<usize, RiskClosureWorkspaceError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        Ok(state.slot_bytes)
    }

    pub fn with_workspace_mut<T>(
        &mut self,
        use_workspace: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<T, RiskClosureWorkspaceError> {
        let storage = self.validated_storage()?;
        storage.with_slot_mut(self.slot_index, use_workspace)
    }

    pub fn release_terminal(
        mut self,
        permit: TerminalReleasePermit,
    ) -> Result<(), TerminalReleaseFailure> {
        if !permit.authority_identity.matches(&self.authority_identity)
            || permit.closure_identity != self.closure_identity
            || permit.closure_generation != self.closure_generation
        {
            return Err(TerminalReleaseFailure::new(
                RiskClosureWorkspaceError::LeaseIdentityMismatch,
                self,
                permit,
            ));
        }
        let inner = Arc::clone(&self.inner);
        let mut state = match inner.lock() {
            Ok(state) => state,
            Err(_) => {
                return Err(TerminalReleaseFailure::new(
                    RiskClosureWorkspaceError::StatePoisoned,
                    self,
                    permit,
                ));
            }
        };
        if let Err(error) = self.validate(&state) {
            drop(state);
            return Err(TerminalReleaseFailure::new(error, self, permit));
        }
        state.logical_slots.remove(&self.closure_identity);
        state.slots[self.slot_index] = SlotState::Free;
        self.active = false;
        Ok(())
    }

    fn validate(&self, state: &WorkspaceState) -> Result<(), RiskClosureWorkspaceError> {
        match state.slots.get(self.slot_index) {
            Some(SlotState::RecoveryCheckedOut {
                closure_identity,
                closure_generation,
                lease_id,
            }) if closure_identity == &self.closure_identity
                && *closure_generation == self.closure_generation
                && *lease_id == self.lease_id
                && state.authority_identity.matches(&self.authority_identity) =>
            {
                Ok(())
            }
            _ => Err(RiskClosureWorkspaceError::InvalidLease),
        }
    }

    fn validated_storage(&self) -> Result<Arc<WorkspaceStorage>, RiskClosureWorkspaceError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        Ok(Arc::clone(&state.storage))
    }
}

impl Drop for RiskClosureWorkspaceLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        if matches!(
            state.slots.get(self.slot_index),
            Some(SlotState::RecoveryCheckedOut {
                closure_identity,
                closure_generation,
                lease_id,
            }) if closure_identity == &self.closure_identity
                && *closure_generation == self.closure_generation
                && *lease_id == self.lease_id
                && state.authority_identity.matches(&self.authority_identity)
        ) {
            state.slots[self.slot_index] = SlotState::RetainedIdle {
                closure_identity: self.closure_identity.clone(),
                closure_generation: self.closure_generation,
            };
        }
    }
}

/// One-use authority to release a terminal closure workspace.
///
/// Its fields and constructor are private. The production durable-transition
/// integration is intentionally absent while production activation is disabled.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_application_resource_ledger::TerminalReleasePermit;
///
/// let _forged = TerminalReleasePermit {};
/// ```
pub struct TerminalReleasePermit {
    authority_identity: AuthorityIdentity,
    closure_identity: ClosureIdentity,
    closure_generation: u64,
}

/// A failed terminal release that preserves both one-use authorities for recovery.
pub struct TerminalReleaseFailure {
    error: RiskClosureWorkspaceError,
    lease: RiskClosureWorkspaceLease,
    permit: TerminalReleasePermit,
}

impl TerminalReleaseFailure {
    fn new(
        error: RiskClosureWorkspaceError,
        lease: RiskClosureWorkspaceLease,
        permit: TerminalReleasePermit,
    ) -> Self {
        Self {
            error,
            lease,
            permit,
        }
    }

    pub const fn error(&self) -> RiskClosureWorkspaceError {
        self.error
    }

    pub fn into_parts(self) -> (RiskClosureWorkspaceLease, TerminalReleasePermit) {
        (self.lease, self.permit)
    }
}

impl std::fmt::Debug for TerminalReleaseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
struct AuthoritativeDurableTerminalTransition {
    authority_identity: AuthorityIdentity,
    closure_identity: ClosureIdentity,
    closure_generation: u64,
}

#[cfg(test)]
impl TerminalReleasePermit {
    fn after_authoritative_durable_terminal_transition(
        transition: AuthoritativeDurableTerminalTransition,
    ) -> Self {
        Self {
            authority_identity: transition.authority_identity,
            closure_identity: transition.closure_identity,
            closure_generation: transition.closure_generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CAPACITY: usize = [false, false, false].len();

    fn test_config() -> RiskClosureWorkspaceConfig {
        let slot_bytes = std::mem::size_of::<u64>();
        RiskClosureWorkspaceConfig {
            arena_bytes: TEST_CAPACITY * slot_bytes,
            slot_bytes,
            capacity: TEST_CAPACITY,
            production_activation_enabled: false,
        }
    }

    fn identity(index: usize) -> ClosureIdentity {
        ClosureIdentity::new(format!("closure-{index}")).unwrap()
    }

    fn terminal_permit(lease: &RiskClosureWorkspaceLease) -> TerminalReleasePermit {
        TerminalReleasePermit::after_authoritative_durable_terminal_transition(
            AuthoritativeDurableTerminalTransition {
                authority_identity: lease.authority_identity.clone(),
                closure_identity: lease.closure_identity.clone(),
                closure_generation: lease.closure_generation,
            },
        )
    }

    #[test]
    fn reservations_leases_and_terminal_permits_are_one_use_types() {
        trait AmbiguousIfClone<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        struct Invalid;
        impl<T: ?Sized + Clone> AmbiguousIfClone<Invalid> for T {}

        let _ = <RiskClosureWorkspaceReservation as AmbiguousIfClone<_>>::check;
        let _ = <RiskClosureWorkspaceLease as AmbiguousIfClone<_>>::check;
        let _ = <TerminalReleasePermit as AmbiguousIfClone<_>>::check;
        let _ = <RiskClosureWorkspaceAuthority as AmbiguousIfClone<_>>::check;

        let _: fn(
            RiskClosureWorkspaceReservation,
            ClosureIdentity,
        ) -> Result<(), RiskClosureWorkspaceError> = RiskClosureWorkspaceReservation::commit;
        let _: fn(
            RiskClosureWorkspaceLease,
            TerminalReleasePermit,
        ) -> Result<(), TerminalReleaseFailure> = RiskClosureWorkspaceLease::release_terminal;
    }

    #[test]
    fn capacity_accepts_limit_minus_one_and_limit_then_rejects_limit_plus_one() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let mut reservations = Vec::new();
        for _ in usize::default()..TEST_CAPACITY.saturating_sub(usize::from(true)) {
            reservations.push(authority.checkout_new_risk().unwrap());
        }
        assert_eq!(
            reservations.len(),
            TEST_CAPACITY.saturating_sub(usize::from(true))
        );
        reservations.push(authority.checkout_new_risk().unwrap());
        assert_eq!(reservations.len(), TEST_CAPACITY);
        assert_eq!(
            authority.checkout_new_risk().err(),
            Some(RiskClosureWorkspaceError::CapacityExhausted)
        );
    }

    #[test]
    fn generated_configuration_allocates_and_touches_exact_capacity() {
        let authority =
            RiskClosureWorkspaceAuthority::with_config(RISK_CLOSURE_WORKSPACE_CONFIG).unwrap();
        assert_eq!(
            authority.reserved_bytes().unwrap(),
            RISK_CLOSURE_WORKSPACE_CONFIG.arena_bytes
        );
        let mut reservations = (usize::default()..RISK_CLOSURE_WORKSPACE_CONFIG.capacity)
            .map(|_| authority.checkout_new_risk().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(reservations.len(), RISK_CLOSURE_WORKSPACE_CONFIG.capacity);
        let mut observed_arena_bytes = usize::default();
        for reservation in &mut reservations {
            let observed_slot_bytes = reservation
                .with_workspace_mut(|workspace| {
                    assert_eq!(workspace.len(), RISK_CLOSURE_WORKSPACE_CONFIG.slot_bytes);
                    workspace.fill(u8::MAX);
                    workspace.len()
                })
                .unwrap();
            observed_arena_bytes = observed_arena_bytes
                .checked_add(observed_slot_bytes)
                .unwrap();
        }
        assert_eq!(
            observed_arena_bytes,
            RISK_CLOSURE_WORKSPACE_CONFIG.arena_bytes
        );
        assert_eq!(
            authority.checkout_new_risk().err(),
            Some(RiskClosureWorkspaceError::CapacityExhausted)
        );
    }

    #[test]
    fn ordinary_exhaustion_blocks_new_risk_while_retained_recovery_remains_available() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let identities = (usize::default()..TEST_CAPACITY)
            .map(identity)
            .collect::<Vec<_>>();
        for closure_identity in &identities {
            authority
                .checkout_new_risk()
                .unwrap()
                .commit(closure_identity.clone())
                .unwrap();
        }
        assert_eq!(
            authority.checkout_new_risk().err(),
            Some(RiskClosureWorkspaceError::CapacityExhausted)
        );
        let admission_started = std::cell::Cell::new(false);
        if authority.checkout_new_risk().is_ok() {
            admission_started.set(true);
        }
        assert!(!admission_started.get());
        let recovery = authority.checkout_recovery(&identities[usize::default()]);
        assert!(recovery.is_ok());
    }

    #[test]
    fn dropped_uncommitted_reservation_returns_actual_storage_capacity() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let mut reservation = authority.checkout_new_risk().unwrap();
        assert_eq!(
            authority.reserved_bytes().unwrap(),
            test_config().arena_bytes
        );
        assert_eq!(
            reservation.workspace_len().unwrap(),
            test_config().slot_bytes
        );
        reservation
            .with_workspace_mut(|workspace| workspace.fill(u8::MAX))
            .unwrap();
        drop(reservation);
        let mut first_replacement = authority.checkout_new_risk().unwrap();
        first_replacement
            .with_workspace_mut(|workspace| {
                assert!(workspace.iter().all(|byte| *byte == u8::default()));
            })
            .unwrap();
        let mut replacements = vec![first_replacement];
        replacements.extend(
            (usize::from(true)..TEST_CAPACITY).map(|_| authority.checkout_new_risk().unwrap()),
        );
        assert_eq!(replacements.len(), TEST_CAPACITY);
    }

    #[test]
    fn duplicate_commit_is_atomic_and_does_not_leak_the_reservation() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let closure_identity = identity(usize::default());
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        assert_eq!(
            authority
                .checkout_new_risk()
                .unwrap()
                .commit(closure_identity),
            Err(RiskClosureWorkspaceError::DuplicateClosureIdentity)
        );
        let remaining = (usize::default()..TEST_CAPACITY.saturating_sub(usize::from(true)))
            .map(|_| authority.checkout_new_risk().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            remaining.len(),
            TEST_CAPACITY.saturating_sub(usize::from(true))
        );
        assert_eq!(
            authority
                .inner
                .lock()
                .unwrap()
                .logical_slots
                .values()
                .filter(|slot| **slot == usize::from(true))
                .count(),
            usize::default()
        );
    }

    #[test]
    fn workspace_callback_does_not_hold_the_authority_lock() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let mut reservation = authority.checkout_new_risk().unwrap();

        reservation
            .with_workspace_mut(|_| {
                assert!(authority.inner.try_lock().is_ok());
            })
            .unwrap();
    }

    #[test]
    fn blocked_workspace_callback_does_not_block_another_slot() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let mut first = authority.checkout_new_risk().unwrap();
        let mut second = authority.checkout_new_risk().unwrap();
        let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel();
        let (second_finished_tx, second_finished_rx) = std::sync::mpsc::channel();

        let first_thread = std::thread::spawn(move || {
            first
                .with_workspace_mut(|_| {
                    first_started_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                })
                .unwrap();
        });
        first_started_rx.recv().unwrap();
        let second_thread = std::thread::spawn(move || {
            second.with_workspace_mut(|_| ()).unwrap();
            second_finished_tx.send(()).unwrap();
        });

        let second_finished = second_finished_rx
            .recv_timeout(std::time::Duration::from_secs(u64::from(true)))
            .is_ok();
        release_first_tx.send(()).unwrap();
        first_thread.join().unwrap();
        second_thread.join().unwrap();
        assert!(second_finished);
    }

    #[test]
    fn panicking_workspace_callback_does_not_poison_recovery_authority() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let mut reservation = authority.checkout_new_risk().unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = reservation.with_workspace_mut::<()>(|_| panic!("callback panic"));
        }));

        assert!(panic.is_err());
        assert_eq!(
            reservation.workspace_len().unwrap(),
            test_config().slot_bytes
        );
        reservation
            .with_workspace_mut(|workspace| workspace.fill(u8::MAX))
            .unwrap();
    }

    #[test]
    fn terminal_release_consumes_matching_recovery_lease_and_durable_transition_permit() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let closure_identity = identity(usize::default());
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        let lease = authority.checkout_recovery(&closure_identity).unwrap();
        let permit = terminal_permit(&lease);
        lease.release_terminal(permit).unwrap();
        assert_eq!(
            authority.checkout_recovery(&closure_identity).err(),
            Some(RiskClosureWorkspaceError::UnknownClosureIdentity)
        );
        assert!(authority.checkout_new_risk().is_ok());
    }

    #[test]
    fn terminal_release_mismatch_returns_both_lease_and_permit() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let first = identity(usize::default());
        let second = identity(usize::from(true));
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(first.clone())
            .unwrap();
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(second.clone())
            .unwrap();
        let first_lease = authority.checkout_recovery(&first).unwrap();
        let second_lease = authority.checkout_recovery(&second).unwrap();
        let second_permit = terminal_permit(&second_lease);

        let failure = first_lease.release_terminal(second_permit).unwrap_err();

        assert_eq!(
            failure.error(),
            RiskClosureWorkspaceError::LeaseIdentityMismatch
        );
        let (first_lease, second_permit) = failure.into_parts();
        drop(first_lease);
        second_lease.release_terminal(second_permit).unwrap();
        assert!(authority.checkout_recovery(&first).is_ok());
        assert_eq!(
            authority.checkout_recovery(&second).err(),
            Some(RiskClosureWorkspaceError::UnknownClosureIdentity)
        );
    }

    #[test]
    fn terminal_permit_cannot_cross_authority_instances() {
        let first_authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let second_authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let closure_identity = identity(usize::default());
        first_authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        second_authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        let first_lease = first_authority
            .checkout_recovery(&closure_identity)
            .unwrap();
        let second_lease = second_authority
            .checkout_recovery(&closure_identity)
            .unwrap();
        let first_permit = terminal_permit(&first_lease);

        let failure = second_lease.release_terminal(first_permit).unwrap_err();

        assert_eq!(
            failure.error(),
            RiskClosureWorkspaceError::LeaseIdentityMismatch
        );
        let (second_lease, first_permit) = failure.into_parts();
        drop(second_lease);
        first_lease.release_terminal(first_permit).unwrap();
    }

    #[test]
    fn stale_terminal_permit_cannot_release_reused_closure_identity() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let closure_identity = identity(usize::default());
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        let old_lease = authority.checkout_recovery(&closure_identity).unwrap();
        let release_permit = terminal_permit(&old_lease);
        let stale_permit = terminal_permit(&old_lease);
        old_lease.release_terminal(release_permit).unwrap();
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        let new_lease = authority.checkout_recovery(&closure_identity).unwrap();

        let failure = new_lease.release_terminal(stale_permit).unwrap_err();

        assert_eq!(
            failure.error(),
            RiskClosureWorkspaceError::LeaseIdentityMismatch
        );
        let (new_lease, stale_permit) = failure.into_parts();
        drop(new_lease);
        drop(stale_permit);
        let current_lease = authority.checkout_recovery(&closure_identity).unwrap();
        let current_permit = terminal_permit(&current_lease);
        current_lease.release_terminal(current_permit).unwrap();
    }

    #[test]
    fn logical_identity_mapping_is_distinct_from_physical_slot_selection() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let first = identity(usize::default());
        let second = identity(usize::from(true));
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(first.clone())
            .unwrap();
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(second.clone())
            .unwrap();
        let state = authority.inner.lock().unwrap();
        assert_ne!(state.logical_slots[&first], state.logical_slots[&second]);
        assert_ne!(first.as_str(), state.logical_slots[&first].to_string());
    }
}
