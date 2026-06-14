//! Shared feed-health forced-flat predicates.
//!
//! Hoisted from `binary_oracle_edge_taker::exposure` so the taker AND the maker
//! admission gate evaluate ONE shared predicate set (Rule #6, no dual-state).
//! The `SelectionPhase` coupling is severed: callers pass a `frozen: bool`
//! instead of the taker-private `SelectionPhase`, so this module holds no
//! `crate::strategies` reference and the dependency-direction fence stays green.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForcedFlatReason {
    Freeze,
    StaleReference,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForcedFlatInputs {
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub last_reference_ts_ms: Option<u64>,
    pub now_ms: u64,
    pub stale_reference_after_ms: u64,
    pub liquidity_available: Option<f64>,
    pub min_liquidity_required: f64,
    pub fast_venue_incoherent: bool,
}

pub fn evaluate_forced_flat_predicates(inputs: &ForcedFlatInputs) -> Vec<ForcedFlatReason> {
    let mut reasons = Vec::new();
    // Defense-in-depth (A14): a MISSING reference timestamp is the maximally
    // stale condition — the strategy has never observed a reference quote — so
    // it must classify as stale, not fresh. `is_none_or` returns `true` for the
    // `None` case (no reference ever) AND for an observed-but-aged reference,
    // and `false` only for a reference observed within the freshness bound.
    let reference_stale = inputs.last_reference_ts_ms.is_none_or(|last_ts_ms| {
        inputs.now_ms.saturating_sub(last_ts_ms) > inputs.stale_reference_after_ms
    });

    if inputs.frozen {
        reasons.push(ForcedFlatReason::Freeze);
    }
    if reference_stale {
        reasons.push(ForcedFlatReason::StaleReference);
    }
    if inputs
        .liquidity_available
        .is_none_or(|liquidity| !liquidity.is_finite() || liquidity < inputs.min_liquidity_required)
    {
        reasons.push(ForcedFlatReason::ThinBook);
    }
    if !inputs.metadata_matches_selection {
        reasons.push(ForcedFlatReason::MetadataMismatch);
    }
    if inputs.fast_venue_incoherent && reference_stale {
        reasons.push(ForcedFlatReason::FastVenueIncoherent);
    }

    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_ok() -> ForcedFlatInputs {
        ForcedFlatInputs {
            frozen: false,
            metadata_matches_selection: true,
            last_reference_ts_ms: Some(1_000),
            now_ms: 1_010,
            stale_reference_after_ms: 100,
            liquidity_available: Some(50.0),
            min_liquidity_required: 10.0,
            fast_venue_incoherent: false,
        }
    }

    #[test]
    fn clean_inputs_yield_no_reasons() {
        assert_eq!(evaluate_forced_flat_predicates(&fresh_ok()), vec![]);
    }

    #[test]
    fn all_reasons_in_canonical_order() {
        let inputs = ForcedFlatInputs {
            frozen: true,
            metadata_matches_selection: false,
            last_reference_ts_ms: Some(0),
            now_ms: 10_000,
            stale_reference_after_ms: 100,
            liquidity_available: Some(1.0),
            min_liquidity_required: 10.0,
            fast_venue_incoherent: true,
        };
        assert_eq!(
            evaluate_forced_flat_predicates(&inputs),
            vec![
                ForcedFlatReason::Freeze,
                ForcedFlatReason::StaleReference,
                ForcedFlatReason::ThinBook,
                ForcedFlatReason::MetadataMismatch,
                ForcedFlatReason::FastVenueIncoherent,
            ]
        );
    }

    #[test]
    fn none_reference_ts_is_maximally_stale() {
        let mut inputs = fresh_ok();
        inputs.last_reference_ts_ms = None;
        assert!(
            evaluate_forced_flat_predicates(&inputs).contains(&ForcedFlatReason::StaleReference)
        );
    }

    #[test]
    fn non_finite_liquidity_is_thin_book() {
        let mut inputs = fresh_ok();
        inputs.liquidity_available = Some(f64::NAN);
        assert!(evaluate_forced_flat_predicates(&inputs).contains(&ForcedFlatReason::ThinBook));
    }

    #[test]
    fn fast_venue_incoherent_requires_staleness() {
        let mut inputs = fresh_ok();
        inputs.fast_venue_incoherent = true; // but reference is fresh
        assert!(
            !evaluate_forced_flat_predicates(&inputs)
                .contains(&ForcedFlatReason::FastVenueIncoherent)
        );
    }
}
