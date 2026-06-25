use crate::{
    bolt_v3_capital_reservation::ReservationLedger,
    bolt_v3_risk_reservation_substrate::contracts::RiskStateVersion,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstrateReservationLedger {
    ledger: ReservationLedger,
    risk_state_version: RiskStateVersion,
}

impl SubstrateReservationLedger {
    pub fn from_existing_ledger(
        ledger: ReservationLedger,
        risk_state_version: RiskStateVersion,
    ) -> Self {
        Self {
            ledger,
            risk_state_version,
        }
    }

    pub fn ledger(&self) -> &ReservationLedger {
        &self.ledger
    }

    pub const fn risk_state_version(&self) -> RiskStateVersion {
        self.risk_state_version
    }
}
