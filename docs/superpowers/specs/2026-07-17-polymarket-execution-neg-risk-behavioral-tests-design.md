# Polymarket Execution Neg-Risk Behavioral Test Design

## Purpose

Close PR #1429's remaining verification gap by proving that the pinned
NautilusTrader Polymarket execution adapter denies orders before signing or
network submission when `neg_risk` metadata is missing or has the wrong type.
This is a test-only adapter change followed by a bolt pin update; production
behavior must remain unchanged.

## Scope

The pinned NT fork will add behavioral regressions for all three execution
entrypoints affected by the existing fail-closed implementation:

- single limit order;
- single market order;
- batch limit orders, including a mixed batch where invalid orders are denied
  while independently valid orders retain their explicit `neg_risk` value.

Each invalid case will cover absent metadata and a non-boolean value. Explicit
`false` and `true` remain accepted and distinguishable.

## Test Boundary

Tests will exercise the real `PolymarketExecutionClient` command path with the
adapter's existing test infrastructure. The observable contract is:

1. the instrument reaches the execution lookup without a valid boolean
   `neg_risk` entry;
2. the submit command emits `OrderDenied` with the missing-metadata reason;
3. the order builder, signer, and HTTP submitter receive zero calls for the
   denied order;
4. batch processing does not let one denied order suppress unrelated valid
   orders.

Mocks or counters are permitted only at the signer/network boundary. Assertions
must be on adapter-visible events and call counts, not on private implementation
ordering.

## Repository Integration

After the NT tests pass, bolt will pin the resulting immutable NT revision in
both workspaces, regenerate both lockfiles, update the registered boundary
fixture hashes and pin documentation, and retain the existing source-fence
checks. No alternate dependency path or local patch override will be added.

## Evidence

- Red evidence: applying the tests to unsafe parent `b25a99cc` produces six
  failures because missing and wrong-type metadata emits `OrderSubmitted`.
- Green evidence: all six targeted cases and the complete 82-test execution
  integration suite pass at test-only NT revision
  `a192a89f7a24e435cfba7a45b6dcd6de14622967`.
- Bolt evidence: boundary verifier/self-tests, formatting, dependency policy,
  source-fence-static, diff checks, and exact-head remote root and Backtester CI.

## Non-Goals

- Changing Polymarket signing or order-submission behavior.
- Adding a bolt-owned execution implementation.
- Broad NT execution refactoring unrelated to the behavioral test seam.
- Granting merge, deploy, readiness, or trading authority.
