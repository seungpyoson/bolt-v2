# Research: Bolt-v3 Nucleus Admission Audit

## Decision: Build a report-only admission audit first

**Rationale**: Current required CI already runs `just fmt-check`, and `fmt-check`
is required by `.github/workflows/ci.yml`. Wiring a new strict gate before the
known blockers are retired would intentionally make the branch fail. The useful
first slice is therefore report-only by default, with strict mode available for
self-tests and the later CI-promotion issue.

**Evidence**:

- `justfile:53-61` runs the existing runtime-literal and provider-leak verifiers
  inside `fmt-check`.
- `.github/workflows/ci.yml:73-90` runs `just fmt-check`.
- `.github/workflows/ci.yml:252-264` requires `fmt-check` success in the managed
  aggregate gate.

**Alternatives considered**:

- Add the audit directly to `fmt-check`: rejected because current `main` has
  known admission blockers and this would create a knowingly failing branch.
- Leave the audit out of `just`: rejected because the feature needs a stable
  command future agents can run.

## Decision: Treat the audit as a higher-level admission gate, not a replacement verifier

**Rationale**: Existing verifiers are useful but intentionally narrow. The
admission audit must catch architecture blockers even when a narrower verifier
allowlist accepts them.

**Evidence**:

- `scripts/verify_bolt_v3_provider_leaks.py:58-88` explicitly allowlists the
  current updown-shaped leaks.
- `src/bolt_v3_adapters.rs:20-31` imports `MarketIdentityPlan` and defines
  `BoltV3UpdownNowFn`.
- `src/bolt_v3_providers/mod.rs:24-63` carries those concrete types through the
  generic provider adapter context.

**Alternatives considered**:

- Only tighten `verify_bolt_v3_provider_leaks.py`: rejected because that would
  conflate a narrow verifier with the broader nucleus admission contract.
- Duplicate every existing verifier: rejected because it creates maintenance
  drift. The admission audit should report the higher-level blockers and call
  out bypasses, not replace all existing checks.

## Decision: Define explicit blocker classes

**Rationale**: A useful audit must be stable enough for future sessions to run
without reinterpreting the whole architecture debate. Stable blocker classes
make the output reviewable.

**Blocker classes**:

- `generic-contract-leak`: concrete provider, venue, family, archetype, symbol,
  feed, timeout, quantity, market, or strategy concept appears in generic core.
- `missing-contract-surface`: required nucleus contract surface is absent.
- `unowned-runtime-default`: generic V3 mapping relies on unowned defaults
  instead of config/catalog ownership.
- `unfenced-concrete-fixture`: concrete fixture data is present without an
  explicit allowed context.
- `narrow-verifier-bypass`: an existing verifier allowlist accepts a nucleus
  blocker.
- `scan-universe-unproven`: the audit cannot prove which files it inspected.

**Evidence**:

- `src/bolt_v3_archetypes/mod.rs:34-37` registers only
  `binary_oracle_edge_taker`.
- `src/bolt_v3_market_families/mod.rs:42-45` registers only `updown`.
- V3-specific search found no `DecisionEvent`, `CustomDataTrait`,
  `ensure_custom_data_registered`, `BacktestEngine`, `add_strategy`, or
  `conformance` surface in `src/bolt_v3*`, `tests/bolt_v3*`, or
  `tests/fixtures/bolt_v3`.
- `src/bolt_v3_providers/polymarket.rs:470` and `:552`,
  `src/bolt_v3_providers/binance.rs:293`, and
  `src/bolt_v3_client_registration.rs:272` still contain
  `Default::default()`.
- `tests/fixtures/bolt_v3/root.toml:105-150` and
  `tests/fixtures/bolt_v3/strategies/binary_oracle.toml:2-20` contain concrete
  provider, family, strategy, asset, and instrument values.

**Alternatives considered**:

- Use one generic "not admitted" blocker: rejected because it would hide the
  evidence and retirement path.
- Encode one-off blockers only for current files: rejected because the next
  session could route around the check by moving code.

## Decision: Require waiver metadata but avoid adding waivers in this slice

**Rationale**: Waivers are necessary for exceptional evidence or docs, but a
waiver without a retirement issue is another way to normalize drift. The
initial audit should define and test the waiver format without granting current
waivers.

**Evidence**:

- Existing provider-leak allowlists demonstrate the risk of narrow exceptions
  becoming persistent architecture.

**Alternatives considered**:

- No waiver mechanism: rejected because documentation and historical evidence
  need a way to name concrete concepts safely.
- Free-form waiver comments: rejected because they cannot be reliably reviewed
  or retired.

## Decision: Keep strict CI promotion as a separate issue

**Rationale**: Report-only mode moves the repository forward without claiming
the nucleus is admitted. Strict CI is the correct end state, but only after the
reported blockers are fixed.

**Evidence**:

- Current source still has generic updown plan/clock leakage, missing V3
  decision-event and BacktestEngine parity surfaces, and unowned V3 defaults.

**Alternatives considered**:

- Combine audit and strict CI promotion: rejected because it would mix detection
  with blocker retirement and make the PR unreviewable.
