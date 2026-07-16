//! Mechanically disabled owner and distributor of application resource capabilities.

use std::sync::Arc;

mod risk_closure_workspace;

use risk_closure_workspace::RiskClosureWorkspaceAuthority;
pub use risk_closure_workspace::{
    ClosureIdentity, ClosureIdentityError, RiskClosureWorkspaceError, RiskClosureWorkspaceLease,
    RiskClosureWorkspaceReservation, TerminalReleaseFailure, TerminalReleasePermit,
};

/// Sole application owner of the risk-closure workspace authority.
///
/// Production code cannot construct this ledger while activation remains disabled.
pub struct ApplicationResourceLedger {
    risk_closure_authority: Arc<RiskClosureWorkspaceAuthority>,
}

#[cfg(test)]
impl ApplicationResourceLedger {
    fn new_disabled() -> Result<Self, RiskClosureWorkspaceError> {
        Ok(Self {
            risk_closure_authority: Arc::new(
                RiskClosureWorkspaceAuthority::for_disabled_application_resource_ledger()?,
            ),
        })
    }
}

impl ApplicationResourceLedger {
    pub fn new_risk_workspace_handle(&self) -> NewRiskWorkspaceHandle {
        NewRiskWorkspaceHandle {
            authority: Arc::clone(&self.risk_closure_authority),
        }
    }

    pub fn recovery_workspace_handle(&self) -> RecoveryWorkspaceHandle {
        RecoveryWorkspaceHandle {
            authority: Arc::clone(&self.risk_closure_authority),
        }
    }
}

/// Capability limited to reserving workspace before new risk begins.
#[derive(Clone)]
pub struct NewRiskWorkspaceHandle {
    authority: Arc<RiskClosureWorkspaceAuthority>,
}

impl NewRiskWorkspaceHandle {
    pub fn reserve_new_risk_workspace(
        &self,
    ) -> Result<RiskClosureWorkspaceReservation, RiskClosureWorkspaceError> {
        self.authority.checkout_new_risk()
    }
}

/// Capability limited to checking out workspace retained for recovery.
#[derive(Clone)]
pub struct RecoveryWorkspaceHandle {
    authority: Arc<RiskClosureWorkspaceAuthority>,
}

impl RecoveryWorkspaceHandle {
    pub fn checkout_retained_recovery_workspace(
        &self,
        closure_identity: &ClosureIdentity,
    ) -> Result<RiskClosureWorkspaceLease, RiskClosureWorkspaceError> {
        self.authority.checkout_recovery(closure_identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(index: usize) -> ClosureIdentity {
        ClosureIdentity::new(format!("ledger-closure-{index}")).unwrap()
    }

    #[test]
    fn handles_share_configured_capacity_and_preserve_retained_recovery() {
        let ledger = ApplicationResourceLedger::new_disabled().unwrap();
        let first_new_risk = ledger.new_risk_workspace_handle();
        let final_new_risk = ledger.new_risk_workspace_handle();
        let recovery = ledger.recovery_workspace_handle();
        let capacity = risk_closure_workspace::configured_capacity();
        let limit_minus_one = capacity.saturating_sub(usize::from(true));
        let mut retained = Vec::new();

        for index in usize::default()..limit_minus_one {
            let closure_identity = identity(index);
            first_new_risk
                .reserve_new_risk_workspace()
                .unwrap()
                .commit(closure_identity.clone())
                .unwrap();
            retained.push(closure_identity);
        }
        assert_eq!(retained.len(), limit_minus_one);

        let final_identity = identity(limit_minus_one);
        final_new_risk
            .reserve_new_risk_workspace()
            .unwrap()
            .commit(final_identity.clone())
            .unwrap();
        retained.push(final_identity);
        assert_eq!(retained.len(), capacity);
        assert_eq!(
            first_new_risk.reserve_new_risk_workspace().err(),
            Some(RiskClosureWorkspaceError::CapacityExhausted)
        );
        assert_eq!(
            final_new_risk.reserve_new_risk_workspace().err(),
            Some(RiskClosureWorkspaceError::CapacityExhausted)
        );

        let recovered = recovery.checkout_retained_recovery_workspace(&retained[usize::default()]);
        assert!(recovered.is_ok());
    }
}
