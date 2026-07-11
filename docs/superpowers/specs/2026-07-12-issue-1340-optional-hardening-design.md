# Issue #1340 Optional Hardening Design

## Goal

Close the remaining provenance and validator-hardening gaps in PR #1362 without changing production strategy, configuration, order semantics, or CI workflows.

## Authoritative sources

- The BVS dependency proof, built from BVS's embedded `Cargo.toml` and `Cargo.lock`, is the sole source for the NautilusTrader revision recorded by BVS manifests and fixtures.
- The proof must report that every locked NautilusTrader package resolves to the declared revision before that revision is recorded.
- The configured settlement price and typed terminal fill remain the only settlement authorities.

## Execution validation

- Reconstruct market execution with NautilusTrader's shared market-order fill simulation using an unrestricted, side-appropriate price bound. Do not derive the simulation bound from an observed fill.
- Continue accepting deterministic partial market fills when executable depth is insufficient. Do not require summed fills to equal the requested quantity.
- Continue requiring observed fill prices and quantities to equal the deterministic shared-primitives result.
- Add a dedicated mutation proving non-market entry fills fail at the market-only contract guard.

Example: a request for 10 units against 7 units of executable depth may validly fill 7. The validator must prove those 7 units came from the modeled book; it must not invent the missing 3 or reject the trace solely for being partial.

## Provenance cleanup

- Replace stale hardcoded NautilusTrader revisions across BVS manifests and tests with the shared BVS dependency-proof owner.
- Keep unrelated manifest meaning and test behavior unchanged.
- Describe canonical configuration provenance as an integrity agreement between canonical bytes, applied overrides, and the recorded hash. It is not a frozen configuration golden or a detector of every live configuration change.

## Evidence

- Differential mutation for a non-market entry.
- Mutation proving a fill that stops at the observed last price cannot hide deeper executable market liquidity.
- A deterministic partial-depth trace is accepted when settlement closes the
  actual filled quantity. The redundant closed-position assertion is removed
  because exact entry binding plus an equal opposite-side close already proves
  closure.
- Static search shows no stale BVS NautilusTrader revision literals remain.
- Cheap local formatting and source-fence gates pass.
- Rust compilation and test execution remain remote-first and must be reported separately from static eligibility.

## Exclusions

- No full-fill requirement for market orders.
- No production strategy, production configuration, sizing, order submission, matching-engine, or CI workflow changes.
- No dynamic fee or rebate implementation; #843 item 10 remains authoritative.
