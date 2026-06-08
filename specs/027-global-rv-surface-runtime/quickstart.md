# Quickstart: Global Multi-Venue Robust RV Runtime

## Goal

Implement a process-level realized-volatility runtime that is usable outside the binary oracle taker, subscribes to every configured available RV source, and publishes mathematically more robust snapshots for pricing and evidence consumers.

## Development Order

1. Confirm the feature artifacts are current:
   - `specs/027-global-rv-surface-runtime/spec.md`
   - `specs/027-global-rv-surface-runtime/plan.md`
   - `specs/027-global-rv-surface-runtime/tasks.md`
   - `specs/027-global-rv-surface-runtime/contracts/*.md`
2. Run or inspect the exact PR head CI before external review. Do not ask external models to review an unpushed or failing head.
3. Implement with TDD only after Claude, Gemini, Grok, and GLM approve the plan/tasks.
4. Prefer CI checks over local broad cargo test runs when validating the final PR head.

## Acceptance Checks

### Global Runtime

Expected implementation result:

- `src/bolt_v3_realized_volatility_runtime.rs` or equivalent owns global RV lifecycle.
- No `RealizedVolEngine` field exists under `src/strategies/**`.
- Strategies receive snapshots/accessors by `realized_volatility_surface_id`.
- Missing/not-ready snapshots block pricing without fallback.

Useful verification:

```sh
rg 'RealizedVolEngine|realized_vol_engine' src/strategies
rg 'ready_realized_vol\(' src/bolt_v3_taker_pricing.rs src/strategies
```

### Multi-Venue Sources

Expected implementation result:

- Production surfaces use every configured available public market-data source for each asset.
- Runtime deduplicates physical subscriptions and fans out observations to all matching surface sources.
- Disabled and non-quorum sources remain visible in diagnostics.

Useful verification:

```sh
rg 'realized_volatility_surfaces' config/root.toml
rg 'source_id|data_client_id|instrument_id' config/root.toml
```

### Mathematical Robustness

Expected implementation result:

- Noise-robust mode can reduce bid/ask-bounce volatility without hiding base fixed-grid RV.
- Jump separation emits continuous and jump components instead of deleting jumps.
- Cross-source aggregation supports median/trimmed/upper-quantile style policies and dispersion blockers.
- Multi-horizon and forecast modes are future scope.

### Evidence

Expected implementation result:

- Evidence schema version is bumped.
- Evidence includes runtime surface provenance, sources used, per-source diagnostics, horizon estimates, robust estimator metadata, blockers, and config fingerprint.
- Zero RV serializes as zero, not as missing.

## CI Preference

For final validation, use GitHub CI on the exact PR head rather than broad local cargo tests. Required checks should include:

- fmt/check
- clippy
- cargo-deny or dependency audit
- source-fence tests
- nextest shards
- source integrity
- GitHub gate/check rollup

Local focused tests are appropriate during red/green TDD, but final approval should cite CI evidence for the pushed head.

## Review Prompt Reminder

Ask reviewers to challenge whether the design truly solves all three root issues:

1. RV runtime is globally usable outside the taker.
2. Production RV surfaces use multiple available venues/sources.
3. The estimator is mathematically more robust than single-window fixed-grid RV while preserving auditability.
