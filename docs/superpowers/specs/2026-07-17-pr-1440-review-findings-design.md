# PR #1440 Review Findings Design

## Goal

Close the confirmed adversarial-review findings in PR #1440 while preserving
the official NautilusTrader pin, provider-owned capability facts, startup, and
all existing-risk recovery, reconciliation, exit, and settlement paths.

## Scope and authority

- This remains the first executable slice of issue #1383; it does not close the
  issue or absorb any remaining bounded-runtime work.
- Root `Cargo.toml` remains the canonical NautilusTrader revision authority.
  A coherent future pin change is valid only when every governed surface and
  capability fact changes with it; no second permanent SHA authority is added.
- `ci/nautilus-source-capabilities.toml` remains the provider-source capability
  authority and cannot be overridden through operator TOML.
- Shared submit admission owns new-risk admissibility. Strategies may produce
  intent and maintain signal state, but cannot authorize submission.

## Shared capability admission

`ProviderBinding` gains an explicit new-risk market-data capability function.
Every binding declares either its provider-owned evaluator or an explicit
always-available evaluator, so adding a provider cannot inherit an implicit
`Ok(true)` default.

The Binance evaluator treats every Spot data client as unavailable at the
selected official pin. `spot_market_data_mode` selects the WebSocket transport,
but the pinned Spot HTTP instrument-loading path still requests and decodes SBE
schema 3:4, so JSON WebSocket mode does not restore the missing schema 3:5
capability. Non-Spot Binance products retain their current availability.

The derived signal capability is carried into `BoltV3SubmitAdmissionRequest`.
For entry intent, shared admission returns a typed provider-capability rejection
before constructing a permit. Risk-reducing exit intent ignores that entry-only
capability field and continues through existing exit admission. The strategy's
reference pricing spot is not cleared: Chainlink reference data remains usable
for intent calculation and existing-risk exits, but cannot substitute for the
missing signal capability when admission considers new risk.

## Runtime transport validation

Realized-volatility transport membership is validated against configured,
enabled sources minus the runtime's explicit capability-unavailable source set.
The derived subscription list is not used as the exemption authority. This
keeps the intended startup exception narrow while ensuring a route or
subscription derivation regression cannot suppress its own missing-transport
diagnostic.

## Governed evidence

The current-status fence rejects additive reintroduction of the known stale
positive Binance capability claim while retaining the two required unavailable
claims. The same positive forms and obsolete fork authority are forbidden in
the current runtime-contract capability sections. Mutation tests add stale text
without deleting truthful text, proving the additive case fails.

The issue-required Polymarket fixture remains. The boundary verifier derives
the exact pinned checkout from the canonical revision, reads the declared
upstream path, verifies the full-source SHA-256, reconstructs the declared
source ranges, and compares their bytes with the fixture body. A changed
revision, digest, range, upstream byte, or extracted fixture byte fails closed.
No network fetch or alternate source authority is introduced.

The existing mechanically scanned boundary registry remains the authority for
the Binance WebSocket feeders. No redundant prose-symbol fence is added.

## Evidence and testing

- A production-shaped strategy regression supplies a valid Chainlink reference
  price and ready non-Binance realized volatility while the Binance signal
  capability is unavailable. It proves entry intent cannot obtain a shared
  submit permit and records the typed capability rejection.
- A paired admission test proves the same unavailable fact does not reject a
  risk-reducing exit.
- Provider-binding tests prove every provider declares capability behavior and
  Binance Spot JSON and SBE configurations are unavailable at this pin.
- Transport-membership mutation coverage proves only explicitly unavailable
  sources receive the startup exception.
- Python mutation tests prove additive stale capability prose and one-byte
  Polymarket fixture drift are rejected.
- Python fences and formatting run locally. Rust behavior is implemented with
  targeted regression coverage and verified through the governed remote probe
  or exact-head CI path; red-first sequencing is optional under the repository's
  Evidence-Driven Verification policy. No local compile-heavy Rust command is
  used.
- Completion requires fresh exact-head root and backtester evidence, resolved
  findings, independent adversarial review, and the required native reviewer.

## Exclusions

- No merge, deployment, live launch, trade, or issue closure.
- No NautilusTrader source modification or alternate repository.
- No strategy-local submit gate, reference-price deletion, fallback provider,
  timestamp restamping, or operator-configurable capability override.
- No permanent duplicate pin constant and no unrelated #1383 runtime-boundary
  work.
