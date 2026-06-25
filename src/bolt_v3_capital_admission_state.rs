use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

use crate::bolt_v3_loss_governor::LossSnapshot;
use crate::bolt_v3_position_sizer::ProductSizingSnapshot;

pub const VENUE_SPENDABILITY_SOURCE_SCHEMA_VERSION: u32 = 1;
pub const VENUE_SPENDABILITY_SOURCE_RECORD_KIND: &str = "bolt_v3.venue_spendability_source.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionStateError {
    MissingNtState,
    StaleNtState(CapitalAdmissionStateEvidenceKind),
    UnattributedState(CapitalAdmissionStateEvidenceKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionStateEvidenceKind {
    State,
    Portfolio,
    VenueSpendability,
    OrderLifecycle,
    ProductState,
    ReservationLedger,
    LossSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtDerivedCapitalAdmissionState {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioCapitalAdmissionSnapshot,
    pub venue_spendability: VenueSpendabilitySnapshot,
    pub order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot,
    pub product_state: ProductSizingSnapshot,
    pub reservation_snapshot: ReservationLedgerSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortfolioCapitalAdmissionSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub free_collateral: Decimal,
    pub total_equity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueSpendabilitySnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub spendable_collateral: Decimal,
    pub collateral_allowance: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueSpendabilityIdentity<'a> {
    pub venue_id: &'a str,
    pub account_id: &'a str,
    pub collateral_currency: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueSpendabilitySourceError {
    ReadFailed,
    FileTooLarge,
    InvalidSha256,
    Sha256Mismatch,
    InvalidPayload,
    InvalidSchemaVersion,
    InvalidRecordKind,
    EmptyField { field: &'static str },
    InvalidDecimal { field: &'static str },
    NegativeDecimal { field: &'static str },
    IdentityMismatch { field: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub struct VenueSpendabilitySourceFileRequest<'a> {
    pub path: &'a Path,
    pub max_bytes: u64,
    pub expected_sha256: &'a str,
    pub identity: VenueSpendabilityIdentity<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VenueSpendabilitySourceArtifact {
    schema_version: u32,
    record_kind: String,
    source: String,
    observed_at_ns: u64,
    venue_id: String,
    account_id: String,
    collateral_currency: String,
    spendable_collateral: String,
    collateral_allowance: String,
}

pub fn venue_spendability_snapshot_from_json_bytes(
    bytes: &[u8],
    expected: VenueSpendabilityIdentity<'_>,
) -> Result<VenueSpendabilitySnapshot, VenueSpendabilitySourceError> {
    let source: VenueSpendabilitySourceArtifact =
        serde_json::from_slice(bytes).map_err(|_| VenueSpendabilitySourceError::InvalidPayload)?;
    if source.schema_version != VENUE_SPENDABILITY_SOURCE_SCHEMA_VERSION {
        return Err(VenueSpendabilitySourceError::InvalidSchemaVersion);
    }
    if source.record_kind != VENUE_SPENDABILITY_SOURCE_RECORD_KIND {
        return Err(VenueSpendabilitySourceError::InvalidRecordKind);
    }
    require_non_empty("source", &source.source)?;
    require_non_empty("venue_id", &source.venue_id)?;
    require_non_empty("account_id", &source.account_id)?;
    require_non_empty("collateral_currency", &source.collateral_currency)?;
    require_identity("venue_id", &source.venue_id, expected.venue_id)?;
    require_identity("account_id", &source.account_id, expected.account_id)?;
    require_identity(
        "collateral_currency",
        &source.collateral_currency,
        expected.collateral_currency,
    )?;
    let spendable_collateral =
        parse_non_negative_decimal("spendable_collateral", &source.spendable_collateral)?;
    let collateral_allowance =
        parse_non_negative_decimal("collateral_allowance", &source.collateral_allowance)?;

    Ok(VenueSpendabilitySnapshot {
        source: source.source,
        observed_at_ns: source.observed_at_ns,
        venue_id: source.venue_id,
        account_id: source.account_id,
        collateral_currency: source.collateral_currency,
        spendable_collateral,
        collateral_allowance,
    })
}

pub fn venue_spendability_snapshot_from_json_file(
    request: VenueSpendabilitySourceFileRequest<'_>,
) -> Result<VenueSpendabilitySnapshot, VenueSpendabilitySourceError> {
    if !is_lowercase_sha256(request.expected_sha256) {
        return Err(VenueSpendabilitySourceError::InvalidSha256);
    }
    let bytes = read_file_bounded(request.path, request.max_bytes)?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != request.expected_sha256 {
        return Err(VenueSpendabilitySourceError::Sha256Mismatch);
    }
    venue_spendability_snapshot_from_json_bytes(&bytes, request.identity)
}

fn read_file_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, VenueSpendabilitySourceError> {
    let mut file = File::open(path).map_err(|_| VenueSpendabilitySourceError::ReadFailed)?;
    let mut bytes = Vec::new();
    let length = file
        .by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| VenueSpendabilitySourceError::ReadFailed)?;
    if length as u64 > max_bytes {
        return Err(VenueSpendabilitySourceError::FileTooLarge);
    }
    Ok(bytes)
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), VenueSpendabilitySourceError> {
    if value.trim().is_empty() {
        return Err(VenueSpendabilitySourceError::EmptyField { field });
    }
    Ok(())
}

fn require_identity(
    field: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), VenueSpendabilitySourceError> {
    if actual != expected {
        return Err(VenueSpendabilitySourceError::IdentityMismatch { field });
    }
    Ok(())
}

fn parse_non_negative_decimal(
    field: &'static str,
    value: &str,
) -> Result<Decimal, VenueSpendabilitySourceError> {
    let decimal = value
        .parse::<Decimal>()
        .map_err(|_| VenueSpendabilitySourceError::InvalidDecimal { field })?;
    if decimal < Decimal::ZERO {
        return Err(VenueSpendabilitySourceError::NegativeDecimal { field });
    }
    Ok(decimal)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLifecycleCapitalAdmissionSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub open_order_count: usize,
    pub all_open_orders_attributed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationLedgerSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub all_live_reservations_attributed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionStateEvidenceSource {
    pub kind: CapitalAdmissionStateEvidenceKind,
    pub source: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionStateEvidence {
    pub sources: Vec<CapitalAdmissionStateEvidenceSource>,
}

pub fn validate_nt_derived_capital_admission_state(
    state: Option<&NtDerivedCapitalAdmissionState>,
    now_ns: u64,
    max_snapshot_age_ns: u64,
) -> Result<CapitalAdmissionStateEvidence, CapitalAdmissionStateError> {
    let Some(state) = state else {
        return Err(CapitalAdmissionStateError::MissingNtState);
    };

    validate_source(&state.source, CapitalAdmissionStateEvidenceKind::State)?;
    validate_freshness(
        state.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::State,
    )?;
    validate_source(
        &state.portfolio.source,
        CapitalAdmissionStateEvidenceKind::Portfolio,
    )?;
    validate_freshness(
        state.portfolio.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::Portfolio,
    )?;
    validate_source(
        &state.venue_spendability.source,
        CapitalAdmissionStateEvidenceKind::VenueSpendability,
    )?;
    validate_freshness(
        state.venue_spendability.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::VenueSpendability,
    )?;
    validate_source(
        &state.order_lifecycle.source,
        CapitalAdmissionStateEvidenceKind::OrderLifecycle,
    )?;
    validate_freshness(
        state.order_lifecycle.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::OrderLifecycle,
    )?;
    if !state.order_lifecycle.all_open_orders_attributed {
        return Err(CapitalAdmissionStateError::UnattributedState(
            CapitalAdmissionStateEvidenceKind::OrderLifecycle,
        ));
    }

    let (product_source, product_observed_at_ns) = match &state.product_state {
        ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
            (snapshot.source.as_str(), snapshot.observed_at_ns)
        }
    };
    validate_source(
        product_source,
        CapitalAdmissionStateEvidenceKind::ProductState,
    )?;
    validate_freshness(
        product_observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::ProductState,
    )?;

    validate_source(
        &state.reservation_snapshot.source,
        CapitalAdmissionStateEvidenceKind::ReservationLedger,
    )?;
    validate_freshness(
        state.reservation_snapshot.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::ReservationLedger,
    )?;
    if !state.reservation_snapshot.all_live_reservations_attributed {
        return Err(CapitalAdmissionStateError::UnattributedState(
            CapitalAdmissionStateEvidenceKind::ReservationLedger,
        ));
    }

    if let Some(loss_snapshot) = &state.loss_snapshot {
        validate_source(
            &loss_snapshot.source,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        )?;
        validate_freshness(
            loss_snapshot.observed_at_ns,
            now_ns,
            max_snapshot_age_ns,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        )?;
    }

    Ok(CapitalAdmissionStateEvidence {
        sources: evidence_sources(state),
    })
}

fn validate_source(
    source: &str,
    kind: CapitalAdmissionStateEvidenceKind,
) -> Result<(), CapitalAdmissionStateError> {
    if source.trim().is_empty() {
        return Err(CapitalAdmissionStateError::UnattributedState(kind));
    }
    Ok(())
}

fn validate_freshness(
    observed_at_ns: u64,
    now_ns: u64,
    max_snapshot_age_ns: u64,
    kind: CapitalAdmissionStateEvidenceKind,
) -> Result<(), CapitalAdmissionStateError> {
    if observed_at_ns > now_ns || now_ns - observed_at_ns > max_snapshot_age_ns {
        return Err(CapitalAdmissionStateError::StaleNtState(kind));
    }
    Ok(())
}

fn evidence_sources(
    state: &NtDerivedCapitalAdmissionState,
) -> Vec<CapitalAdmissionStateEvidenceSource> {
    let (product_source, product_observed_at_ns) = match &state.product_state {
        ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
            (snapshot.source.clone(), snapshot.observed_at_ns)
        }
    };
    let mut sources = vec![
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::State,
            source: state.source.clone(),
            observed_at_ns: state.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::Portfolio,
            source: state.portfolio.source.clone(),
            observed_at_ns: state.portfolio.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::VenueSpendability,
            source: state.venue_spendability.source.clone(),
            observed_at_ns: state.venue_spendability.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::OrderLifecycle,
            source: state.order_lifecycle.source.clone(),
            observed_at_ns: state.order_lifecycle.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::ProductState,
            source: product_source,
            observed_at_ns: product_observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::ReservationLedger,
            source: state.reservation_snapshot.source.clone(),
            observed_at_ns: state.reservation_snapshot.observed_at_ns,
        },
    ];
    if let Some(loss_snapshot) = &state.loss_snapshot {
        sources.push(CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::LossSnapshot,
            source: loss_snapshot.source.clone(),
            observed_at_ns: loss_snapshot.observed_at_ns,
        });
    }
    sources
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use sha2::{Digest, Sha256};

    use crate::bolt_v3_loss_governor::{LossSnapshot, LossSourceObservationTimestamps};
    use crate::bolt_v3_position_sizer::{PredictionMarketSizingSnapshot, ProductSizingSnapshot};

    use super::{
        CapitalAdmissionStateError, CapitalAdmissionStateEvidenceKind,
        NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
        PortfolioCapitalAdmissionSnapshot, ReservationLedgerSnapshot, VenueSpendabilityIdentity,
        VenueSpendabilitySnapshot, VenueSpendabilitySourceError,
        VenueSpendabilitySourceFileRequest, validate_nt_derived_capital_admission_state,
        venue_spendability_snapshot_from_json_bytes, venue_spendability_snapshot_from_json_file,
    };

    #[test]
    fn capital_admission_state_missing_nt_snapshot_fails_closed() {
        let decision = validate_nt_derived_capital_admission_state(None, 1_000, 100)
            .expect_err("missing NT-derived sizing state must fail closed");

        assert_eq!(decision, CapitalAdmissionStateError::MissingNtState);
    }

    fn state() -> NtDerivedCapitalAdmissionState {
        NtDerivedCapitalAdmissionState {
            source: "nt_sizing_state".to_string(),
            observed_at_ns: 1_000,
            portfolio: PortfolioCapitalAdmissionSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 1_000,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-A".to_string(),
                collateral_currency: "USD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            venue_spendability: VenueSpendabilitySnapshot {
                source: "operator-venue-spendability".to_string(),
                observed_at_ns: 1_000,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-A".to_string(),
                collateral_currency: "USD".to_string(),
                spendable_collateral: Decimal::new(100, 0),
                collateral_allowance: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 1_000,
                open_order_count: 1,
                all_open_orders_attributed: true,
            },
            product_state: ProductSizingSnapshot::PredictionMarketBinary(
                PredictionMarketSizingSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns: 1_000,
                    yes_instrument_id: "instrument-1".to_string(),
                    no_instrument_id: "instrument-1-no".to_string(),
                    yes_position: Decimal::new(10, 0),
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::new(100, 0),
                    conditional_token_allowance: Decimal::new(10, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            reservation_snapshot: ReservationLedgerSnapshot {
                source: "bolt_reservation_ledger".to_string(),
                observed_at_ns: 1_000,
                all_live_reservations_attributed: true,
            },
            loss_snapshot: None,
        }
    }

    fn loss_snapshot() -> LossSnapshot {
        LossSnapshot {
            source: "bolt_loss_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::ZERO),
            daily_pnl: Some(Decimal::ZERO),
            rolling_pnl: Some(Decimal::ZERO),
            current_equity: Some(Decimal::new(100, 0)),
            peak_equity: Some(Decimal::new(100, 0)),
            source_observations: LossSourceObservationTimestamps::unobserved(),
        }
    }

    fn state_with_observed_at(
        kind: CapitalAdmissionStateEvidenceKind,
        observed_at_ns: u64,
    ) -> NtDerivedCapitalAdmissionState {
        let mut candidate = state();
        match kind {
            CapitalAdmissionStateEvidenceKind::State => candidate.observed_at_ns = observed_at_ns,
            CapitalAdmissionStateEvidenceKind::Portfolio => {
                candidate.portfolio.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::VenueSpendability => {
                candidate.venue_spendability.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::OrderLifecycle => {
                candidate.order_lifecycle.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::ProductState => {
                let ProductSizingSnapshot::PredictionMarketBinary(snapshot) =
                    &mut candidate.product_state;
                snapshot.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::ReservationLedger => {
                candidate.reservation_snapshot.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::LossSnapshot => {
                let mut snapshot = loss_snapshot();
                snapshot.observed_at_ns = observed_at_ns;
                candidate.loss_snapshot = Some(snapshot);
            }
        }
        candidate
    }

    #[test]
    fn capital_admission_state_valid_snapshot_returns_expected_evidence_sources() {
        let evidence = validate_nt_derived_capital_admission_state(Some(&state()), 1_000, 100)
            .expect("fresh attributed NT-derived sizing state should be accepted");

        let kinds = evidence
            .sources
            .iter()
            .map(|source| source.kind)
            .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            vec![
                CapitalAdmissionStateEvidenceKind::State,
                CapitalAdmissionStateEvidenceKind::Portfolio,
                CapitalAdmissionStateEvidenceKind::VenueSpendability,
                CapitalAdmissionStateEvidenceKind::OrderLifecycle,
                CapitalAdmissionStateEvidenceKind::ProductState,
                CapitalAdmissionStateEvidenceKind::ReservationLedger,
            ]
        );
    }

    #[test]
    fn capital_admission_state_valid_loss_snapshot_is_included_in_evidence() {
        let mut candidate = state();
        candidate.loss_snapshot = Some(loss_snapshot());

        let evidence = validate_nt_derived_capital_admission_state(Some(&candidate), 1_000, 100)
            .expect("fresh attributed loss snapshot should be accepted");

        assert_eq!(evidence.sources.len(), 7);
        assert_eq!(
            evidence.sources.last().map(|source| source.kind),
            Some(CapitalAdmissionStateEvidenceKind::LossSnapshot)
        );
    }

    #[test]
    fn unattributed_state_transition_fails_closed() {
        let cases = [
            {
                let mut candidate = state();
                candidate.source = " ".to_string();
                (candidate, CapitalAdmissionStateEvidenceKind::State)
            },
            {
                let mut candidate = state();
                candidate.portfolio.source = " ".to_string();
                (candidate, CapitalAdmissionStateEvidenceKind::Portfolio)
            },
            {
                let mut candidate = state();
                candidate.venue_spendability.source = " ".to_string();
                (
                    candidate,
                    CapitalAdmissionStateEvidenceKind::VenueSpendability,
                )
            },
            {
                let mut candidate = state();
                candidate.order_lifecycle.all_open_orders_attributed = false;
                (candidate, CapitalAdmissionStateEvidenceKind::OrderLifecycle)
            },
            {
                let mut candidate = state();
                let ProductSizingSnapshot::PredictionMarketBinary(snapshot) =
                    &mut candidate.product_state;
                snapshot.source.clear();
                (candidate, CapitalAdmissionStateEvidenceKind::ProductState)
            },
            {
                let mut candidate = state();
                candidate
                    .reservation_snapshot
                    .all_live_reservations_attributed = false;
                (
                    candidate,
                    CapitalAdmissionStateEvidenceKind::ReservationLedger,
                )
            },
            {
                let mut candidate = state();
                let mut snapshot = loss_snapshot();
                snapshot.source = " ".to_string();
                candidate.loss_snapshot = Some(snapshot);
                (candidate, CapitalAdmissionStateEvidenceKind::LossSnapshot)
            },
        ];

        for (candidate, expected_kind) in cases {
            let decision =
                validate_nt_derived_capital_admission_state(Some(&candidate), 1_000, 100)
                    .expect_err("unattributed NT-derived state must fail closed");

            assert_eq!(
                decision,
                CapitalAdmissionStateError::UnattributedState(expected_kind)
            );
        }
    }

    #[test]
    fn stale_capital_admission_state_evidence_fails_closed_for_each_kind() {
        let kinds = [
            CapitalAdmissionStateEvidenceKind::State,
            CapitalAdmissionStateEvidenceKind::Portfolio,
            CapitalAdmissionStateEvidenceKind::VenueSpendability,
            CapitalAdmissionStateEvidenceKind::OrderLifecycle,
            CapitalAdmissionStateEvidenceKind::ProductState,
            CapitalAdmissionStateEvidenceKind::ReservationLedger,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        ];

        for kind in kinds {
            let stale = state_with_observed_at(kind, 899);
            let decision = validate_nt_derived_capital_admission_state(Some(&stale), 1_000, 100)
                .expect_err("stale NT-derived sizing state must fail closed");

            assert_eq!(decision, CapitalAdmissionStateError::StaleNtState(kind));
        }
    }

    #[test]
    fn future_capital_admission_state_evidence_fails_closed_for_each_kind() {
        let kinds = [
            CapitalAdmissionStateEvidenceKind::State,
            CapitalAdmissionStateEvidenceKind::Portfolio,
            CapitalAdmissionStateEvidenceKind::VenueSpendability,
            CapitalAdmissionStateEvidenceKind::OrderLifecycle,
            CapitalAdmissionStateEvidenceKind::ProductState,
            CapitalAdmissionStateEvidenceKind::ReservationLedger,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        ];

        for kind in kinds {
            let future = state_with_observed_at(kind, 1_001);
            let decision = validate_nt_derived_capital_admission_state(Some(&future), 1_000, 100)
                .expect_err("future NT-derived sizing state must fail closed");

            assert_eq!(decision, CapitalAdmissionStateError::StaleNtState(kind));
        }
    }

    #[test]
    fn venue_spendability_source_parses_and_rejects_identity_mismatch() {
        let source = br#"{
  "schema_version": 1,
  "record_kind": "bolt_v3.venue_spendability_source.v1",
  "source": "operator-venue-spendability",
  "observed_at_ns": 1200,
  "venue_id": "VENUE-A",
  "account_id": "ACCOUNT-001",
  "collateral_currency": "USD",
  "spendable_collateral": "30",
  "collateral_allowance": "25"
}"#;

        let snapshot = venue_spendability_snapshot_from_json_bytes(
            source,
            VenueSpendabilityIdentity {
                venue_id: "VENUE-A",
                account_id: "ACCOUNT-001",
                collateral_currency: "USD",
            },
        )
        .expect("matching venue spendability evidence should parse");

        assert_eq!(snapshot.source, "operator-venue-spendability");
        assert_eq!(snapshot.observed_at_ns, 1200);
        assert_eq!(snapshot.venue_id, "VENUE-A");
        assert_eq!(snapshot.account_id, "ACCOUNT-001");
        assert_eq!(snapshot.collateral_currency, "USD");
        assert_eq!(snapshot.spendable_collateral, Decimal::new(30, 0));
        assert_eq!(snapshot.collateral_allowance, Decimal::new(25, 0));

        let mismatch = venue_spendability_snapshot_from_json_bytes(
            source,
            VenueSpendabilityIdentity {
                venue_id: "VENUE-B",
                account_id: "ACCOUNT-001",
                collateral_currency: "USD",
            },
        )
        .expect_err("mismatched venue spendability evidence must fail closed");

        assert_eq!(
            mismatch,
            VenueSpendabilitySourceError::IdentityMismatch { field: "venue_id" }
        );
    }

    #[test]
    fn venue_spendability_source_file_checks_sha_and_size() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("venue-spendability.json");
        let source = br#"{
  "schema_version": 1,
  "record_kind": "bolt_v3.venue_spendability_source.v1",
  "source": "operator-venue-spendability",
  "observed_at_ns": 1200,
  "venue_id": "VENUE-A",
  "account_id": "ACCOUNT-001",
  "collateral_currency": "USD",
  "spendable_collateral": "30",
  "collateral_allowance": "25"
}"#;
        std::fs::write(&path, source).expect("fixture should write");
        let sha256 = hex::encode(Sha256::digest(source));
        let identity = VenueSpendabilityIdentity {
            venue_id: "VENUE-A",
            account_id: "ACCOUNT-001",
            collateral_currency: "USD",
        };

        let snapshot =
            venue_spendability_snapshot_from_json_file(VenueSpendabilitySourceFileRequest {
                path: &path,
                max_bytes: source.len() as u64,
                expected_sha256: &sha256,
                identity,
            })
            .expect("bounded hash-checked artifact should parse");
        assert_eq!(snapshot.collateral_allowance, Decimal::new(25, 0));

        let wrong_hash = "0".repeat(64);
        let mismatch =
            venue_spendability_snapshot_from_json_file(VenueSpendabilitySourceFileRequest {
                path: &path,
                max_bytes: source.len() as u64,
                expected_sha256: &wrong_hash,
                identity,
            })
            .expect_err("sha mismatch must fail closed");
        assert_eq!(mismatch, VenueSpendabilitySourceError::Sha256Mismatch);

        let too_large =
            venue_spendability_snapshot_from_json_file(VenueSpendabilitySourceFileRequest {
                path: &path,
                max_bytes: source.len() as u64 - 1,
                expected_sha256: &sha256,
                identity,
            })
            .expect_err("oversized spendability source must fail closed");
        assert_eq!(too_large, VenueSpendabilitySourceError::FileTooLarge);
    }
}
