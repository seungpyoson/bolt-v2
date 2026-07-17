//! Exact historical identities for the retired sample backfill lane.
//!
//! This is provenance-only data, not a runtime source-selection registry. Keep
//! venue and instrument literals confined here so generic ingestion code stays
//! source-agnostic while the signed retirement inventory remains verifiable.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub(crate) const EXPECTED_INSTRUMENT_ID: &str = "BNBUSDC";
pub(crate) const RETIRED_DATE_RANGES: &[(RetiredBackfillVenue, &str, &str, usize)] = &[
    (
        RetiredBackfillVenue::Binance,
        "2026-03-01",
        "2026-05-31",
        92,
    ),
    (RetiredBackfillVenue::Bybit, "2026-03-01", "2026-06-01", 93),
];
pub(crate) const RETIRED_AGGREGATE_ARTIFACTS: &[(&str, &str, &str)] = &[
    (
        "backfill-conversion-batches",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-conversion-batch-plan.toml",
    ),
    (
        "backfill-conversion-batches",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "plan/backfill-conversion-batch-plan.json",
    ),
    (
        "backfill-conversion-batches",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-conversion-batch-plan.toml",
    ),
    (
        "backfill-conversion-batches",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "plan/backfill-conversion-batch-plan.json",
    ),
    (
        "backfill-coverage-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-coverage-ledger.toml",
    ),
    (
        "backfill-coverage-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "ledger/backfill-coverage-ledger.json",
    ),
    (
        "backfill-coverage-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-coverage-ledger.toml",
    ),
    (
        "backfill-coverage-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "ledger/backfill-coverage-ledger.json",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-conversion-completion-ledger.toml",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "binance-bnbusdc-2026-03-01-2026-05-31",
        "ledger/backfill-conversion-completion-ledger.json",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-conversion-completion-ledger.toml",
    ),
    (
        "backfill-conversion-completion-ledgers",
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        "ledger/backfill-conversion-completion-ledger.json",
    ),
];

const RETIRED_GATE_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/backfill-gates/";
const ACTIVE_GOLDEN_PROFILE: &str =
    "backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetiredBackfillVenue {
    Binance,
    Bybit,
}

impl RetiredBackfillVenue {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
        }
    }

    pub(crate) const fn source_binding(self) -> &'static str {
        match self {
            Self::Binance => "binance-spot-native-trades",
            Self::Bybit => "bybit-spot-tick-trades",
        }
    }
}

pub(crate) fn retired_record_id(venue: RetiredBackfillVenue, archive_date: &str) -> String {
    format!(
        "retired-backfill-evidence-{}-bnbusdc-{archive_date}",
        venue.as_str()
    )
}

pub(crate) fn retired_gate_root(venue: RetiredBackfillVenue, archive_date: &str) -> String {
    format!(
        "{RETIRED_GATE_PREFIX}{}-bnbusdc-{archive_date}/",
        venue.as_str()
    )
}

pub(crate) fn daily_profile_should_be_retired(
    venue: RetiredBackfillVenue,
    archive_date: &str,
) -> bool {
    !(venue == RetiredBackfillVenue::Binance && archive_date == "2026-03-01")
}

pub(crate) fn retired_gate_venue_and_date(scope: &str) -> Option<(RetiredBackfillVenue, &str)> {
    if let Some(date) = scope.strip_prefix("binance-bnbusdc-") {
        Some((RetiredBackfillVenue::Binance, date))
    } else {
        scope
            .strip_prefix("bybit-bnbusdc-")
            .map(|date| (RetiredBackfillVenue::Bybit, date))
    }
}

pub(crate) fn is_retired_venue_date(venue: RetiredBackfillVenue, date: &str) -> bool {
    if date.len() != "YYYY-MM-DD".len() || NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return false;
    }
    RETIRED_DATE_RANGES
        .iter()
        .find(|(candidate, _, _, _)| *candidate == venue)
        .is_some_and(|(_, first, last, _)| date >= *first && date <= *last)
}

pub(crate) fn is_retired_daily_run_spec_name(file_name: &str) -> bool {
    if file_name == ACTIVE_GOLDEN_PROFILE {
        return false;
    }
    let (venue, date_and_suffix) = if let Some(value) =
        file_name.strip_prefix("backtesting-vertical-slice-run-spec.binance-bnbusdc-")
    {
        (RetiredBackfillVenue::Binance, value)
    } else if let Some(value) =
        file_name.strip_prefix("backtesting-vertical-slice-run-spec.bybit-bnbusdc-")
    {
        (RetiredBackfillVenue::Bybit, value)
    } else {
        return false;
    };
    date_and_suffix
        .strip_suffix(".toml")
        .is_some_and(|date| is_retired_venue_date(venue, date))
}
