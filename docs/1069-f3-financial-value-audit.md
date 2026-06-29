# #1069 F3 Financial Value Audit

## FinancialValue coverage

Runtime financial newtypes marked with `FinancialValue` in this slice:

- `bolt_v3_numeric::ProbabilityValue`
- `bolt_v3_maker_mu_estimator::UsableMu`
- `bolt_v3_realized_volatility::ValidRealizedVol`
- `bolt_v3_realized_volatility::ReadyRealizedVol`

No local price/quantity wrapper newtype exists in `src/`; NT `Price`/`Quantity`
are foreign types and are not marked because the F3 fence targets local financial
newtypes.

## Default re-add fence

`FinancialValue` is sealed through `financial_value_private::{Sealed,
NoDefaultProbe}`. Each marked financial value implements one private
`financial_value_default_readd_fence` probe method, while a blanket private
`DefaultProbe` adds the same method name for every `T: Default`. Adding
`#[derive(Default)]`, `#[cfg_attr(..., derive(Default))]`, macro-generated
`Default`, or a manual `impl Default` to a marked financial type makes the
`assert_financial_value_not_default!` marker const ambiguous during Rust
compilation.

The legacy Python fence remains responsible for production `*_default()` style
calls, plus the existing default-call residue patterns.

Clippy HIR status: this repo has no existing custom Clippy/Dylint extension
point, and adding a new rustc-private lint lane would create a new build/lint
path for this slice. The implemented compiler fence is the viable replacement:
it is keyed on `FinancialValue`, fires after macro and `cfg_attr` expansion, and
is covered by a synthetic compile-fail test.

## Clamp caller classification

All former `clamp_probability` callers were audited. No caller required a
silent fail-open conversion.

| Former site | Classification | Replacement |
|---|---|---|
| `bolt_v3_taker_updown_signal::price_agreement_corr` | Relied-upon lower saturation: extreme price disagreement should become zero correlation. | `bounded_probability_from_finite(...).map(ProbabilityValue::get)` |
| `bolt_v3_taker_updown_signal::price_gap_probability` | Relied-upon upper saturation: gap at or beyond reference should become full gap probability. | `bounded_probability_from_finite(...).map(ProbabilityValue::get)` |
| `bolt_v3_taker_updown_signal::time_uncertainty_probability` | Relied-upon upper saturation: diffusion band can exceed unit probability. | `bounded_probability_from_finite(...).map(ProbabilityValue::get)` |
| `bolt_v3_taker_updown_signal::compute_theta_scaler` | Relied-upon upper saturation: time-to-cadence ratio can exceed one. | `bounded_probability_from_finite(...)?` |
| `bolt_v3_taker_updown_signal::compute_worst_case_ev_bps` | Relied-upon lower/upper saturation around fair probability minus/plus uncertainty. | `bounded_probability_from_finite(...)?` |
| `bolt_v3_taker_pricing::observe_signal_quote` | Relied-upon upper saturation for jitter penalty; conversion failure marks the fast venue incoherent. | Precomputed `Option<f64>` from `bounded_probability_from_finite(...)` |
| `bolt_v3_sizing::choose_robust_size` | Relied-upon upper saturation: strong EV reaches the operator target. | Explicit `None` branch returns zero size. |
| `bolt_v3_sizing::maker_robust_size` | Relied-upon upper saturation: edge at/above reference reaches the operator target. | Explicit `None` branch returns zero size. |
| `binary_oracle_edge_taker::uncertainty_band_probability_for_seconds` | Fee uncertainty may saturate at one; non-finite conversion propagates `None`. | `bounded_probability_from_finite(...)?` |
| `binary_oracle_edge_taker::adjusted_probability_up_for_fee_uncertainty` | Relied-upon lower/upper saturation around fair probability plus fee uncertainty. | `bounded_probability_from_finite(...)?` |
| `binary_oracle_edge_taker::entry_evaluation_at` adjusted probabilities | Relied-upon lower/upper saturation for initial edge evaluation. | Failed conversion records `UncertaintyBandUnavailable` and returns no entry. |

## Fail-closed path check

The new `None` paths do not use `unwrap()` or `expect()`. In the strategy entry
path, `None` from uncertainty-band construction is translated into
`EntryPricingBlockReason::UncertaintyBandUnavailable`; the entry evaluation then
returns without a submission. Shared taker/sizing functions return `None` or
zero-size intent rather than fabricating an accepted probability.

## Static init and serde audit

Searches for `lazy_static`, `OnceCell`, `OnceLock`, `LazyLock`, and `static`
financial-value initialization found no financial newtype static/lazy
initializers. Searches for `#[serde(default)]` on `UsableMu`,
`ValidRealizedVol`, `ReadyRealizedVol`, or `ProbabilityValue` found no matches.

## Orphan-rule check

All `FinancialValue`, `Sealed`, and `DefaultReaddFence` impls are local-trait
impls for local runtime newtypes. No foreign type receives a `FinancialValue`
impl, so the slice does not rely on an orphan-rule exception that downstream
crates could not reproduce.
