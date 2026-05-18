# Research: CI Slow Test Post-Stop Delay

## Decision: Use NT builder post-stop delay configuration in tests only

**Rationale**: Baseline evidence shows `contract_happy_path_polymarket` spends 10.04s in NT shutdown and logs `Awaiting residual events (10s)`. NT exposes `LiveNodeBuilder::with_delay_post_stop_secs`, and existing tests elsewhere already use zero for test-local builders.

**Alternatives considered**:

- Move slow tests out of PR CI: rejected because #357 forbids gate weakening without replacement evidence and approval.
- Change production defaults: rejected because #357 is test runtime cost, not production runtime semantics.
- Edit every site inline: rejected because it repeats a test-local policy across 31 builder sites.

## Decision: Centralize plain test node construction in `tests/support/mod.rs`

**Rationale**: The helper makes the zero-delay test policy explicit and keeps future slow-test additions from reintroducing NT's default delay by copy-paste.

**Alternatives considered**:

- Per-file helpers: rejected because three copies create drift.
- Source-code verifier for test literals: rejected as extra verifier surface for a narrow test helper.
