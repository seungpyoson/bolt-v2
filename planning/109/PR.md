# Draft PR Body

## Scope

This PR is an isolated pre-#109 implementation slice for issue `#109` only. It generalizes the resolution-basis parsing and selector seam so ETH and other assets in the same family pattern do not require asset-specific selector literals.

## What Changed

- replaced BTC-specific declared-basis parsing with a canonical parser that extracts `family + symbol pair + optional cadence`
- made selector matching structural via parsed canonical bases instead of raw-string equality
- added fail-closed validation for malformed `ruleset.resolution_basis`
- preserved recognized family-to-reference-venue validation semantics
- added BTC and ETH parsing/matching tests plus malformed/mismatch rejection coverage

## Trust Map

The final state-to-mechanism ledger is in `planning/109/MECHANISM-MAP.md`.

## Verification

- `cargo test --test polymarket_catalog`
- `cargo test --test ruleset_selector`
- `cargo test phase1_runtime_resolution_basis_requires_matching_reference_venue_family`
- `cargo test phase1_runtime_rejects_invalid_resolution_basis_format`
- `cargo test phase1_runtime_eth_chainlink_basis_requires_matching_reference_venue_family`
- `cargo test --test platform_runtime`

## Review Notes

- `planning/109/REDTEAM.md` records the adversarial findings and their terminal `FIXED` status.
- External adversarial review is still user-gated and intentionally not executed yet.
