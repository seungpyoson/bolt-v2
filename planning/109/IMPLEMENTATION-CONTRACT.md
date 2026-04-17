# Issue #109 Implementation Contract

## Allowed Production Files

- `src/platform/resolution_basis.rs`
- `src/platform/polymarket_catalog.rs`
- `src/platform/ruleset.rs`
- `src/validate.rs`

## Required Test Files

- `tests/polymarket_catalog.rs`
- `tests/ruleset_selector.rs`
- `tests/platform_runtime.rs` if type changes require it
- `src/validate/tests.rs`

## Mandatory Behaviors

1. Config basis strings must parse into a canonical representation or fail validation.
2. Polymarket metadata must parse into the same representation or the market is dropped.
3. Selector matching must use structural or canonical comparison.
4. Recognized family prefixes must still imply the same reference venue requirement as before.
5. BTC coverage must remain green.
6. ETH coverage must prove the new generic path.

## Implemented Shape

- Internal model: `ResolutionBasis` parser in `src/platform/resolution_basis.rs`
- Stored catalog value: canonical normalized string such as `chainlink_ethusd` or `binance_ethusdt_1m`
- Selector rule: parse both sides and compare the structured result, fail closed on parse failure

## Out Of Scope

- strategy logic changes
- broader config redesign
- multiple-ruleset orchestration
- non-selector platform redesign
