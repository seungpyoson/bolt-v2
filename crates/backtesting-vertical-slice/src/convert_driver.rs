//! Per-object fault isolation and run reporting for the staged-object ->
//! NautilusTrader catalog convert loop.
//!
//! Contract: a single bad staged object (malformed archive, parse failure,
//! unsupported row) is recorded and skipped, never aborting the run, so every
//! other object and every later `(venue, family)` binding is still converted.
//! The run exits non-zero iff any object failed, with a loud per-object failure
//! report — fail loud without abandoning good data.

use std::fmt::Write as _;

/// What one staged object produced when it converted successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStats {
    /// Records (ticks/bars/deltas) written across all instruments in the object.
    pub records: usize,
    /// NautilusTrader instrument ids the object wrote.
    pub instruments: Vec<String>,
}

/// The outcome of attempting one staged object — or one indivisible binding unit
/// such as a mark-price batch, or a per-binding listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectOutcome {
    /// `"venue/family"` label of the binding the object belongs to.
    pub binding: String,
    /// The staged object key (or a binding-level label for listing/batch units).
    pub object_key: String,
    /// `Ok(stats)` on success, `Err(error chain)` on failure.
    pub outcome: Result<ObjectStats, String>,
}

/// Every object's outcome across one convert run — the single source of truth
/// for the run's exit status.
#[derive(Debug, Default)]
pub struct ConvertReport {
    outcomes: Vec<ObjectOutcome>,
}

impl ConvertReport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one object's outcome. Used for the per-object path (via
    /// [`run_objects`]) and for binding-level units (a failed listing, a batch)
    /// the caller records directly.
    pub fn record(&mut self, outcome: ObjectOutcome) {
        self.outcomes.push(outcome);
    }

    /// Every recorded outcome, in attempt order.
    pub fn outcomes(&self) -> &[ObjectOutcome] {
        &self.outcomes
    }

    /// Count of objects that converted successfully.
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.outcome.is_ok()).count()
    }

    /// Count of objects that failed to convert.
    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.outcome.is_err()).count()
    }

    /// Total records written across all successful objects.
    pub fn total_records(&self) -> usize {
        self.outcomes
            .iter()
            .filter_map(|o| o.outcome.as_ref().ok())
            .map(|stats| stats.records)
            .sum()
    }

    /// The run is a failure iff any object failed. Drives the process exit code
    /// so the operator (and the per-venue run wrapper) is forced to look.
    pub fn is_failure(&self) -> bool {
        self.failed() > 0
    }

    /// One `FAILED [binding] key: error` line per failed object (newline
    /// terminated), or an empty string when nothing failed.
    pub fn failure_report(&self) -> String {
        let mut out = String::new();
        for outcome in &self.outcomes {
            if let Err(err) = &outcome.outcome {
                let _ = writeln!(
                    out,
                    "FAILED [{}] {}: {}",
                    outcome.binding, outcome.object_key, err
                );
            }
        }
        out
    }
}

/// Convert one binding's objects with per-object fault isolation.
///
/// Every key is run through `convert_one` regardless of any prior key's failure
/// (so one bad object never aborts the run); each per-object outcome is passed to
/// `emit` as it completes (for incremental, durable progress) and recorded into
/// `report`. An `Err` from `convert_one` is captured as a failure outcome (with
/// its full error chain) and the loop continues to the next key.
///
/// `pub(crate)`: the production batch runner converts through
/// `operator::run_from_run_spec`, so this per-object fault-isolation loop has no
/// production caller today; only this module's tests exercise it. Scoping it to
/// the crate keeps it off the public API without deleting test-covered logic.
pub(crate) fn run_objects<F, E>(
    report: &mut ConvertReport,
    binding: &str,
    object_keys: &[String],
    mut convert_one: F,
    mut emit: E,
) where
    F: FnMut(&str) -> anyhow::Result<ObjectStats>,
    E: FnMut(&ObjectOutcome),
{
    for key in object_keys {
        let outcome = ObjectOutcome {
            binding: binding.to_string(),
            object_key: key.clone(),
            outcome: convert_one(key).map_err(|err| format!("{err:#}")),
        };
        emit(&outcome);
        report.record(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::{ConvertReport, ObjectOutcome, ObjectStats, run_objects};
    use std::cell::RefCell;

    fn stats(records: usize, instruments: &[&str]) -> ObjectStats {
        ObjectStats {
            records,
            instruments: instruments.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn run_objects_isolates_a_bad_object_and_still_attempts_the_rest() {
        // The early-abort failure mode: convert() `?`-aborted on the first bad
        // object, silently dropping every later object. Here the middle object
        // errors and the third must still be attempted and converted.
        let keys = vec![
            "good-a".to_string(),
            "bad".to_string(),
            "good-c".to_string(),
        ];
        let attempted = RefCell::new(Vec::new());
        let mut report = ConvertReport::new();
        run_objects(
            &mut report,
            "exchange-a/candlesticks",
            &keys,
            |key| {
                attempted.borrow_mut().push(key.to_string());
                if key == "bad" {
                    anyhow::bail!("single-bar strike cannot derive interval");
                }
                Ok(stats(2, &[key]))
            },
            |_| {},
        );

        assert_eq!(
            *attempted.borrow(),
            vec!["good-a", "bad", "good-c"],
            "every object must be attempted; the bad one must not abort the run"
        );
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 1);
        assert!(report.is_failure());
        assert_eq!(report.total_records(), 4, "only the good objects' records");
        let failures = report.failure_report();
        assert!(failures.contains("exchange-a/candlesticks"), "{failures}");
        assert!(failures.contains("bad"), "{failures}");
        assert!(failures.contains("cannot derive interval"), "{failures}");
    }

    #[test]
    fn downstream_binding_is_not_dropped_when_an_earlier_binding_fails() {
        let attempted = RefCell::new(Vec::new());
        let mut report = ConvertReport::new();
        let mut convert = |key: &str| {
            attempted.borrow_mut().push(key.to_string());
            if key == "candles-bad" {
                anyhow::bail!("bad candle object");
            }
            Ok(stats(1, &[key]))
        };
        run_objects(
            &mut report,
            "exchange-a/candlesticks",
            &["candles-bad".to_string()],
            &mut convert,
            |_| {},
        );
        run_objects(
            &mut report,
            "exchange-b/order_book",
            &["book-good".to_string()],
            &mut convert,
            |_| {},
        );

        assert!(
            attempted.borrow().iter().any(|k| k == "book-good"),
            "the later binding's object must still be attempted"
        );
        assert_eq!(report.failed(), 1);
        assert_eq!(report.succeeded(), 1);
    }

    #[test]
    fn a_clean_run_reports_no_failure() {
        let mut report = ConvertReport::new();
        run_objects(
            &mut report,
            "exchange-c/bars_1m",
            &["o1".to_string(), "o2".to_string()],
            |key| Ok(stats(3, &[key])),
            |_| {},
        );
        assert_eq!(report.failed(), 0);
        assert!(!report.is_failure());
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.total_records(), 6);
        assert!(report.failure_report().is_empty());
    }

    #[test]
    fn each_object_outcome_is_emitted_in_order() {
        let emitted = RefCell::new(Vec::new());
        let mut report = ConvertReport::new();
        run_objects(
            &mut report,
            "exchange-d/tick_trades",
            &["x".to_string(), "y".to_string()],
            |key| Ok(stats(1, &[key])),
            |outcome: &ObjectOutcome| emitted.borrow_mut().push(outcome.object_key.clone()),
        );
        assert_eq!(*emitted.borrow(), vec!["x", "y"]);
    }
}
