# Issue #109 Invariant Ledger

1. `resolution_basis` must have one canonical representation.
Proof target: code paths compare parsed/canonical values rather than raw input strings.

2. Asset symbols are data, not code literals.
Proof target: no BTC- or ETH-specific selector logic in `src/platform/resolution_basis.rs`, `src/platform/polymarket_catalog.rs`, or `src/platform/ruleset.rs`.

3. A malformed ruleset basis must never reach live selection as an implicitly trusted string.
Fail-closed rule: runtime validation rejects the config before selector execution.
Proof target: validation test for malformed basis.

4. Ambiguous market metadata must never be coerced into a basis guess.
Fail-closed rule: translation drops the candidate market.
Proof target: catalog tests for unknown or ambiguous basis.

5. Family validation semantics stay family-based, not asset-based.
Fail-closed rule: if a recognized family implies a required reference venue kind, missing configuration halts validation.
Proof target: runtime validation tests for chainlink and exchange families.

6. A new asset inside an existing family pattern must succeed without code edits.
Proof target: ETH parsing and selector tests using the same parser and comparator as BTC.

7. Mismatch rejection must remain explicit and auditable.
Proof target: selector tests still surface `resolution_basis_mismatch`.
