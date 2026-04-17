# Issue #109 Adversarial Review

## Findings

### F1

- Finding: market parsing is BTC-specific and can reject valid ETH markets unless code is edited.
- Attack: feed `Chainlink ETH/USD` metadata into the current parser.
- Expected failure mode: current code returns `None` or the wrong basis, causing safe rejection but blocking intended support.
- Owner: Implementer
- Evidence required: parser and selector tests for ETH Chainlink and ETH Binance forms.
- Terminal status: FIXED
- Evidence: `src/platform/resolution_basis.rs:95-166`, `tests/polymarket_catalog.rs:233-253`

### F2

- Finding: raw-string equality can reject equivalent bases that differ only by case or formatting.
- Attack: compare `BINANCE_BTCUSDT_1M` config against a market parsed from mixed-case metadata.
- Expected failure mode: selector must still match after canonical parsing.
- Owner: Implementer
- Evidence required: selector test proving canonical structural equality.
- Terminal status: FIXED
- Evidence: `src/platform/ruleset.rs:124-160`, `tests/ruleset_selector.rs:66-99`

### F3

- Finding: malformed ruleset basis strings can bypass runtime validation today because only non-empty values are checked.
- Attack: use a malformed basis such as `chainlink` or `_btcusd`.
- Expected failure mode: config load must halt with a validation error.
- Owner: Implementer
- Evidence required: runtime validation test for invalid format.
- Terminal status: FIXED
- Evidence: `src/validate.rs:78-90`, `src/validate.rs:728-733`, `src/validate.rs:1477-1494`, `src/validate/tests.rs:2219-2231`

### F4

- Finding: a loose metadata parser can guess the wrong basis from descriptive prose.
- Attack: supply a description that names a family but no safe symbol pair.
- Expected failure mode: parser returns `None`; candidate is dropped.
- Owner: Implementer
- Evidence required: catalog test for ambiguous metadata rejection.
- Terminal status: FIXED
- Evidence: `src/platform/resolution_basis.rs:121-214`, `src/platform/polymarket_catalog.rs:82-100`, `tests/polymarket_catalog.rs:256-264`

### F5

- Finding: changing the family parser could accidentally weaken existing reference-venue validation.
- Attack: use `kraken_btcusd_1m` or `chainlink_ethusd` without the matching reference venue kind configured.
- Expected failure mode: runtime validation still rejects the config.
- Owner: Implementer
- Evidence required: runtime validation tests for recognized families.
- Terminal status: FIXED
- Evidence: `src/platform/resolution_basis.rs:106-118`, `src/validate.rs:1477-1494`, `src/validate/tests.rs:2204-2246`
