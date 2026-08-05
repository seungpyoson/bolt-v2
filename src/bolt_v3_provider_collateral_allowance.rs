//! Provider collateral allowance input and capture-failure classification.

use std::{error::Error, fmt, future::Future, pin::Pin};

use nautilus_core::UnixNanos;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_capital_admission_state::ProviderCollateralAllowanceSnapshot;

pub type ProviderCollateralAllowanceSnapshotFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<ProviderCollateralAllowanceSnapshot>> + Send + 'a>>;

pub trait ProviderCollateralAllowanceSnapshotSource: fmt::Debug + Send + Sync {
    fn snapshot(&self, captured_at: UnixNanos) -> ProviderCollateralAllowanceSnapshotFuture<'_>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCollateralAllowanceCaptureEndpoint {
    ProviderCollateralAllowanceSnapshot,
    ClobBalanceAllowance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCollateralAllowanceCaptureErrorClass {
    Unknown,
    TransportOrDecode,
}

macro_rules! impl_domain_display {
    ($ty:ty, {$($variant:path => $label:literal),+ $(,)?}) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self {
                    $($variant => $label,)+
                })
            }
        }
    };
}

impl_domain_display!(ProviderCollateralAllowanceCaptureEndpoint, {
    ProviderCollateralAllowanceCaptureEndpoint::ProviderCollateralAllowanceSnapshot => "provider_collateral_allowance_snapshot",
    ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance => "clob_balance_allowance",
});

impl_domain_display!(ProviderCollateralAllowanceCaptureErrorClass, {
    ProviderCollateralAllowanceCaptureErrorClass::Unknown => "unknown",
    ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode => "transport_or_decode",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCollateralAllowanceCaptureFailureEvidence {
    pub source: String,
    pub observed_at_ns: u64,
    pub endpoint: ProviderCollateralAllowanceCaptureEndpoint,
    pub error_class: ProviderCollateralAllowanceCaptureErrorClass,
    pub captures_missed: u64,
}

#[derive(Debug)]
pub struct ProviderCollateralAllowanceCaptureEndpointError {
    endpoint: ProviderCollateralAllowanceCaptureEndpoint,
    error_class: ProviderCollateralAllowanceCaptureErrorClass,
    source: anyhow::Error,
}

impl ProviderCollateralAllowanceCaptureEndpointError {
    pub fn new(
        endpoint: ProviderCollateralAllowanceCaptureEndpoint,
        error_class: ProviderCollateralAllowanceCaptureErrorClass,
        source: anyhow::Error,
    ) -> Self {
        Self {
            endpoint,
            error_class,
            source,
        }
    }

    #[must_use]
    pub const fn endpoint(&self) -> ProviderCollateralAllowanceCaptureEndpoint {
        self.endpoint
    }

    #[must_use]
    pub const fn error_class(&self) -> ProviderCollateralAllowanceCaptureErrorClass {
        self.error_class
    }
}

impl fmt::Display for ProviderCollateralAllowanceCaptureEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider collateral allowance capture endpoint `{}` failed with error class `{}`: {:#}",
            self.endpoint, self.error_class, self.source
        )
    }
}

impl Error for ProviderCollateralAllowanceCaptureEndpointError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.source()
    }
}

#[must_use]
pub fn provider_collateral_allowance_capture_failure_parts(
    error: &anyhow::Error,
) -> (
    ProviderCollateralAllowanceCaptureEndpoint,
    ProviderCollateralAllowanceCaptureErrorClass,
) {
    error
        .downcast_ref::<ProviderCollateralAllowanceCaptureEndpointError>()
        .map(|error| (error.endpoint(), error.error_class()))
        .unwrap_or((
            ProviderCollateralAllowanceCaptureEndpoint::ProviderCollateralAllowanceSnapshot,
            ProviderCollateralAllowanceCaptureErrorClass::Unknown,
        ))
}
