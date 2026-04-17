# Issue #109 Walkthrough Simulation

## Scenario S1: ETH Chainlink Success

- Ruleset config: `chainlink_ethusd`
- Reference venues include kind `chainlink`
- Market metadata says `Resolution Source: information from Chainlink ETH / USD feeds.`
- Expected path:
  - config basis parses canonically
  - market metadata parses canonically
  - selector sees structural equality
  - candidate survives basis check

## Scenario S2: ETH Mismatch Rejection

- Ruleset config: `chainlink_ethusd`
- Market metadata resolves to `binance_ethusdt_1m`
- Expected path:
  - both sides parse
  - structural comparison differs by family and symbol
  - selector rejects candidate with `resolution_basis_mismatch`

## Scenario S3: Malformed Config Halt

- Ruleset config: `chainlink`
- Expected path:
  - runtime validation cannot parse the basis into the canonical shape
  - config load halts before selector execution

## Scenario S4: Ambiguous Metadata Drop

- Market description mentions `Chainlink` but no safe symbol pair
- Expected path:
  - metadata parser refuses to invent a pair
  - candidate translation returns `None`
  - market is excluded from the candidate set
