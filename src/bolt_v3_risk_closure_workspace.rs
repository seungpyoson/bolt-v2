//! Fixed-capacity memory workspaces retained across risk closure.

use std::{
    collections::BTreeMap,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskClosureWorkspaceConfig {
    arena_bytes: usize,
    slot_bytes: usize,
    capacity: usize,
    production_activation_enabled: bool,
}

impl RiskClosureWorkspaceConfig {
    pub const fn arena_bytes(self) -> usize {
        self.arena_bytes
    }

    pub const fn slot_bytes(self) -> usize {
        self.slot_bytes
    }

    pub const fn capacity(self) -> usize {
        self.capacity
    }

    pub const fn production_activation_enabled(self) -> bool {
        self.production_activation_enabled
    }
}

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
    StorageInUse,
    LeaseIdentityMismatch,
    InvalidLease,
    LeaseIdExhausted,
    StatePoisoned,
}

#[derive(Debug, Clone)]
pub struct RiskClosureWorkspaceAuthority {
    inner: Arc<Mutex<WorkspaceState>>,
}

impl RiskClosureWorkspaceAuthority {
    pub fn new() -> Result<Self, RiskClosureWorkspaceError> {
        Self::with_config(RISK_CLOSURE_WORKSPACE_CONFIG)
    }

    fn with_config(config: RiskClosureWorkspaceConfig) -> Result<Self, RiskClosureWorkspaceError> {
        Ok(Self {
            inner: Arc::new(Mutex::new(WorkspaceState::allocate(config)?)),
        })
    }

    pub fn checkout_new_risk(
        &self,
    ) -> Result<RiskClosureWorkspaceReservation, RiskClosureWorkspaceError> {
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
        Ok(RiskClosureWorkspaceReservation {
            inner: Arc::clone(&self.inner),
            slot_index,
            lease_id,
            active: true,
            not_clone: PhantomData,
        })
    }

    pub fn checkout_recovery(
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
        match &state.slots[slot_index] {
            SlotState::RetainedIdle {
                closure_identity: retained,
            } if retained == closure_identity => {}
            SlotState::RecoveryCheckedOut { .. } => {
                return Err(RiskClosureWorkspaceError::ClosureAlreadyCheckedOut);
            }
            _ => return Err(RiskClosureWorkspaceError::InvalidLease),
        }
        let lease_id = state.take_lease_id()?;
        state.slots[slot_index] = SlotState::RecoveryCheckedOut {
            closure_identity: closure_identity.clone(),
            lease_id,
        };
        Ok(RiskClosureWorkspaceLease {
            inner: Arc::clone(&self.inner),
            closure_identity: closure_identity.clone(),
            slot_index,
            lease_id,
            active: true,
            not_clone: PhantomData,
        })
    }

    pub fn reserved_bytes(&self) -> Result<usize, RiskClosureWorkspaceError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        Ok(state.storage.len())
    }

    pub fn replace_storage_from_generated_config(&self) -> Result<(), RiskClosureWorkspaceError> {
        self.replace_storage(RISK_CLOSURE_WORKSPACE_CONFIG)
    }

    fn replace_storage(
        &self,
        config: RiskClosureWorkspaceConfig,
    ) -> Result<(), RiskClosureWorkspaceError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        if state
            .slots
            .iter()
            .any(|slot| !matches!(slot, SlotState::Free))
        {
            return Err(RiskClosureWorkspaceError::StorageInUse);
        }
        let replacement = WorkspaceState::allocate(config)?;
        *state = replacement;
        Ok(())
    }
}

#[derive(Debug)]
struct WorkspaceState {
    config: RiskClosureWorkspaceConfig,
    storage: Box<[u8]>,
    slots: Vec<SlotState>,
    logical_slots: BTreeMap<ClosureIdentity, usize>,
    next_lease_id: u64,
}

impl WorkspaceState {
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
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(config.arena_bytes)
            .map_err(|_| RiskClosureWorkspaceError::AllocationFailed)?;
        storage.resize(config.arena_bytes, u8::MAX);
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(config.capacity)
            .map_err(|_| RiskClosureWorkspaceError::AllocationFailed)?;
        slots.resize_with(config.capacity, || SlotState::Free);
        Ok(Self {
            config,
            storage: storage.into_boxed_slice(),
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

    fn workspace_range(&self, slot_index: usize) -> std::ops::Range<usize> {
        let start = slot_index * self.config.slot_bytes;
        start..start + self.config.slot_bytes
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
    },
    RecoveryCheckedOut {
        closure_identity: ClosureIdentity,
        lease_id: u64,
    },
}

/// An uncommitted reservation over one real workspace slot.
///
/// The reservation is intentionally neither `Clone` nor `Copy`.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_risk_closure_workspace::RiskClosureWorkspaceAuthority;
///
/// let authority = RiskClosureWorkspaceAuthority::new().unwrap();
/// let reservation = authority.checkout_new_risk().unwrap();
/// let _duplicate = reservation.clone();
/// ```
///
/// Commit consumes the reservation, so it cannot be reused.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_risk_closure_workspace::{
///     ClosureIdentity, RiskClosureWorkspaceAuthority,
/// };
///
/// let authority = RiskClosureWorkspaceAuthority::new().unwrap();
/// let reservation = authority.checkout_new_risk().unwrap();
/// reservation.commit(ClosureIdentity::new("closure").unwrap()).unwrap();
/// let _ = reservation.workspace_len();
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
        Ok(state.config.slot_bytes)
    }

    pub fn with_workspace_mut<T>(
        &mut self,
        use_workspace: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<T, RiskClosureWorkspaceError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        let range = state.workspace_range(self.slot_index);
        Ok(use_workspace(&mut state.storage[range]))
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
        state.slots[self.slot_index] = SlotState::RetainedIdle { closure_identity };
        self.active = false;
        Ok(())
    }

    fn validate(&self, state: &WorkspaceState) -> Result<(), RiskClosureWorkspaceError> {
        match state.slots.get(self.slot_index) {
            Some(SlotState::Reserved { lease_id }) if *lease_id == self.lease_id => Ok(()),
            _ => Err(RiskClosureWorkspaceError::InvalidLease),
        }
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
/// use bolt_v2::bolt_v3_risk_closure_workspace::{
///     ClosureIdentity, RiskClosureWorkspaceAuthority,
/// };
///
/// let authority = RiskClosureWorkspaceAuthority::new().unwrap();
/// let identity = ClosureIdentity::new("closure").unwrap();
/// authority.checkout_new_risk().unwrap().commit(identity.clone()).unwrap();
/// let lease = authority.checkout_recovery(&identity).unwrap();
/// let _duplicate = lease.clone();
/// ```
///
/// Terminal release consumes both the lease and its permit.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_risk_closure_workspace::{
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
    closure_identity: ClosureIdentity,
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
        Ok(state.config.slot_bytes)
    }

    pub fn with_workspace_mut<T>(
        &mut self,
        use_workspace: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<T, RiskClosureWorkspaceError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        let range = state.workspace_range(self.slot_index);
        Ok(use_workspace(&mut state.storage[range]))
    }

    pub fn release_terminal(
        mut self,
        permit: TerminalReleasePermit,
    ) -> Result<(), RiskClosureWorkspaceError> {
        if permit.closure_identity != self.closure_identity {
            return Err(RiskClosureWorkspaceError::LeaseIdentityMismatch);
        }
        let inner = Arc::clone(&self.inner);
        let mut state = inner
            .lock()
            .map_err(|_| RiskClosureWorkspaceError::StatePoisoned)?;
        self.validate(&state)?;
        state.logical_slots.remove(&self.closure_identity);
        state.slots[self.slot_index] = SlotState::Free;
        self.active = false;
        Ok(())
    }

    fn validate(&self, state: &WorkspaceState) -> Result<(), RiskClosureWorkspaceError> {
        match state.slots.get(self.slot_index) {
            Some(SlotState::RecoveryCheckedOut {
                closure_identity,
                lease_id,
            }) if closure_identity == &self.closure_identity && *lease_id == self.lease_id => {
                Ok(())
            }
            _ => Err(RiskClosureWorkspaceError::InvalidLease),
        }
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
            Some(SlotState::RecoveryCheckedOut { closure_identity, lease_id })
                if closure_identity == &self.closure_identity && *lease_id == self.lease_id
        ) {
            state.slots[self.slot_index] = SlotState::RetainedIdle {
                closure_identity: self.closure_identity.clone(),
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
/// use bolt_v2::bolt_v3_risk_closure_workspace::TerminalReleasePermit;
///
/// let _forged = TerminalReleasePermit {};
/// ```
pub struct TerminalReleasePermit {
    closure_identity: ClosureIdentity,
}

#[cfg(test)]
struct AuthoritativeDurableTerminalTransition {
    closure_identity: ClosureIdentity,
}

#[cfg(test)]
impl TerminalReleasePermit {
    fn after_authoritative_durable_terminal_transition(
        transition: AuthoritativeDurableTerminalTransition,
    ) -> Self {
        Self {
            closure_identity: transition.closure_identity,
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

    fn terminal_permit(identity: ClosureIdentity) -> TerminalReleasePermit {
        TerminalReleasePermit::after_authoritative_durable_terminal_transition(
            AuthoritativeDurableTerminalTransition {
                closure_identity: identity,
            },
        )
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
            test_config().arena_bytes()
        );
        assert_eq!(
            reservation.workspace_len().unwrap(),
            test_config().slot_bytes()
        );
        reservation
            .with_workspace_mut(|workspace| workspace.fill(u8::default()))
            .unwrap();
        drop(reservation);
        let replacements = (usize::default()..TEST_CAPACITY)
            .map(|_| authority.checkout_new_risk().unwrap())
            .collect::<Vec<_>>();
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
    }

    #[test]
    fn checked_out_or_retained_workspace_prevents_storage_replacement() {
        let authority = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
        let reservation = authority.checkout_new_risk().unwrap();
        assert_eq!(
            authority.replace_storage(test_config()),
            Err(RiskClosureWorkspaceError::StorageInUse)
        );
        drop(reservation);
        let closure_identity = identity(usize::default());
        authority
            .checkout_new_risk()
            .unwrap()
            .commit(closure_identity.clone())
            .unwrap();
        assert_eq!(
            authority.replace_storage(test_config()),
            Err(RiskClosureWorkspaceError::StorageInUse)
        );
        let recovery = authority.checkout_recovery(&closure_identity).unwrap();
        assert_eq!(
            authority.replace_storage(test_config()),
            Err(RiskClosureWorkspaceError::StorageInUse)
        );
        drop(recovery);
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
        authority
            .checkout_recovery(&closure_identity)
            .unwrap()
            .release_terminal(terminal_permit(closure_identity.clone()))
            .unwrap();
        assert_eq!(
            authority.checkout_recovery(&closure_identity).err(),
            Some(RiskClosureWorkspaceError::UnknownClosureIdentity)
        );
        assert!(authority.checkout_new_risk().is_ok());
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
