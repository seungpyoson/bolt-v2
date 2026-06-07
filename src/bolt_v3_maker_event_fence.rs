//! Shared maker venue-event identity fence.

use crate::bolt_v3_quote_lifecycle::LegEvent;

/// Client-order identity carried by venue reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOrderId(String);

impl ClientOrderId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One maker order identity for a leg generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIdentity {
    client_order_id: ClientOrderId,
    generation: u64,
}

impl OrderIdentity {
    pub fn new(client_order_id: ClientOrderId, generation: u64) -> Self {
        Self {
            client_order_id,
            generation,
        }
    }

    pub fn client_order_id(&self) -> &ClientOrderId {
        &self.client_order_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Untrusted venue report after the shell extracts its shared identity fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueReport {
    pub client_order_id: ClientOrderId,
    pub generation: u64,
    pub kind: VenueReportKind,
}

/// Venue-originated report kinds that can drive a maker leg lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueReportKind {
    Accepted,
    Rejected,
    Canceled,
    Modified,
    ModifyRejected,
    CancelRejected,
    Filled,
}

impl VenueReportKind {
    fn into_leg_event(self) -> LegEvent {
        match self {
            Self::Accepted => LegEvent::Accepted,
            Self::Rejected => LegEvent::Rejected,
            Self::Canceled => LegEvent::Canceled,
            Self::Modified => LegEvent::Modified,
            Self::ModifyRejected => LegEvent::ModifyRejected,
            Self::CancelRejected => LegEvent::CancelRejected,
            Self::Filled => LegEvent::Filled,
        }
    }
}

/// Reason a venue report was not allowed to reach the lifecycle machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceReject {
    ForeignClientId,
    StaleGeneration,
    UnknownOrder,
}

/// Current per-leg order identity expected by the maker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedIdentity {
    expected: Option<OrderIdentity>,
}

impl ExpectedIdentity {
    pub fn idle() -> Self {
        Self { expected: None }
    }

    pub fn submitting(identity: OrderIdentity) -> Self {
        Self {
            expected: Some(identity),
        }
    }

    pub fn expected(&self) -> Option<&OrderIdentity> {
        self.expected.as_ref()
    }

    pub fn requote_to(&mut self, next: OrderIdentity) -> bool {
        match &self.expected {
            Some(current) if next.generation <= current.generation => return false,
            _ => {}
        }
        self.expected = Some(next);
        true
    }

    pub fn clear(&mut self) {
        self.expected = None;
    }

    pub fn admit(&self, report: &VenueReport) -> Result<LegEvent, FenceReject> {
        let expected = self.expected.as_ref().ok_or(FenceReject::UnknownOrder)?;
        if report.client_order_id != expected.client_order_id {
            return Err(FenceReject::ForeignClientId);
        }
        if report.generation < expected.generation {
            return Err(FenceReject::StaleGeneration);
        }
        if report.generation > expected.generation {
            return Err(FenceReject::UnknownOrder);
        }
        Ok(report.kind.into_leg_event())
    }
}
