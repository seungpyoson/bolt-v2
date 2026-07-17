# Polymarket Execution Neg-Risk Behavioral Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the pinned Polymarket execution adapter denies limit, market, and batch orders before signing or HTTP submission when boolean `neg_risk` metadata is unavailable.

**Architecture:** Extend the existing NT `exec_client.rs` integration harness, which already captures `ExecutionEvent` values and HTTP post counters. Keep adapter production code unchanged, publish the test-only NT commit, then repin both bolt workspaces and refresh registered boundary evidence.

**Tech Stack:** Rust, Tokio, rstest, Axum test server, Cargo lockfiles, Python source-fence verifiers, GitHub Actions.

## Global Constraints

- Tests exercise the real `PolymarketExecutionClient` command path.
- Missing and non-boolean `neg_risk` must emit `OrderDenied` and produce zero order HTTP posts.
- Explicit `false` and `true` remain accepted and distinguishable.
- No bolt-owned execution path, local Cargo patch, or alternate dependency source.
- Bolt local verification remains non-compile; Rust proof comes from targeted NT tests and exact-head remote bolt CI.

---

### Task 1: Add NT execution behavioral regressions

**Files:**
- Modify: `crates/adapters/polymarket/tests/exec_client.rs`

**Interfaces:**
- Consumes: `create_test_execution_client`, `TestServerState`, `make_limit_order`, `make_market_order`, `make_submit_cmd`, `make_submit_order_list_cmd`, `assert_order_event`, and `order_event_reason`.
- Produces: integration tests proving denial events and zero `/order` or `/orders` posts.

- [x] **Step 1: Create an isolated NT clone at production revision `e7af3dce0c7656862c33acb962aff5ae738eecb6` and a named review branch**

```bash
git clone https://github.com/seungpyoson/nautilus_trader.git /private/tmp/nautilus-neg-risk-tests
git -C /private/tmp/nautilus-neg-risk-tests switch -c codex/polymarket-neg-risk-execution-tests e7af3dce0c7656862c33acb962aff5ae738eecb6
```

- [x] **Step 2: Add explicit-valid and invalid instrument fixture helpers**

Change the existing cache helper so ordinary positive tests insert `info["neg_risk"] = Bool(false)`. Refactor the current constructor body into a sibling helper accepting `Option<Value>` so tests can install absent, string, `false`, or `true` metadata without duplicating `BinaryOption` construction. The existing constructor arguments stay byte-for-byte unchanged; replace only its current `info: None` argument with `info: Some(info)`:

```rust
fn add_instrument_to_cache_with_neg_risk(
    cache: &Rc<RefCell<Cache>>,
    instrument_id: InstrumentId,
    neg_risk: Option<Value>,
) {
    let mut info = nautilus_core::Params::new();
    if let Some(value) = neg_risk {
        info.insert("neg_risk".to_string(), value);
    }
    let instrument = BinaryOption::new(
        instrument_id,
        raw_symbol,
        AssetClass::Alternative,
        Currency::pUSD(),
        UnixNanos::default(),
        UnixNanos::default(),
        4,
        size_precision,
        Price::from("0.0001"),
        size_increment,
        None, None, None, None, None, None, None, None, None, None,
        None, None, None, None,
        Some(info),
        UnixNanos::default(),
        UnixNanos::default(),
    );
    cache.borrow_mut()
        .add_instrument(InstrumentAny::BinaryOption(instrument))
        .unwrap();
}
```

- [x] **Step 3: Add parameterized limit and market denial tests**

For `None` and `Some(Value::String("false".into()))`, install the instrument, start the client, submit a valid limit or market order, and assert:

```rust
let denied = assert_order_event(recv_execution_event(&mut rx).await, "Denied");
assert!(order_event_reason(&denied).contains("Missing required neg_risk metadata"));
assert_eq!(*state.order_post_count.lock().await, 0);
assert_eq!(*state.batch_order_post_count.lock().await, 0);
```

- [x] **Step 4: Add all-invalid and mixed batch tests**

The all-invalid batch asserts one denial per invalid order and zero batch posts. The mixed batch uses one invalid instrument and two valid instruments carrying explicit `false` and `true`; it asserts the invalid order is denied, the valid requests reach the mock server, and the captured request bodies preserve both boolean values.

- [x] **Step 5: Verify RED against the unsafe parent**

Apply only the test diff to a temporary worktree at `b25a99cc`, then run:

```bash
cargo test -p nautilus-polymarket --test exec_client neg_risk -- --nocapture
```

Expected: the new invalid-metadata submission tests fail because the parent fabricates `false` or otherwise reaches the HTTP server instead of emitting the required denial.

- [x] **Step 6: Verify GREEN at `a192a89f`**

Run the same targeted command on the review branch. Expected: all new tests pass and existing `exec_client` tests compile with explicit fixture metadata.

- [x] **Step 7: Commit and publish the NT test-only change**

```bash
git add crates/adapters/polymarket/tests/exec_client.rs
git commit -m "test(polymarket): prove neg-risk execution denial"
git push origin HEAD:codex/polymarket-neg-risk-execution-tests
```

Record the immutable commit SHA for Task 2.

Recorded revision: `a192a89f7a24e435cfba7a45b6dcd6de14622967`. The targeted six-case
regression and the complete 82-test `exec_client` integration suite pass at
that revision. Applying the same tests to the unsafe parent `b25a99cc` fails
all six cases because invalid metadata reaches `OrderSubmitted`.

---

### Task 2: Repin bolt and refresh boundary evidence

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/backtesting-vertical-slice/Cargo.toml`
- Modify: `crates/backtesting-vertical-slice/Cargo.lock`
- Modify: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md`
- Modify: `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md`
- Rename/Modify: `tests/fixtures/nt_polymarket_neg_risk_contract_<sha>.txt`
- Rename/Modify: `tests/fixtures/nt_polymarket_query_post_order_params_<sha>.txt`
- Modify: `scripts/verify_bolt_v3_boundary_evidence.py`
- Modify: `scripts/test_verify_bolt_v3_boundary_evidence.py`
- Modify: `docs/superpowers/specs/2026-07-16-end-to-end-contract-closure-design.md`
- Modify: `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`
- Modify: `docs/bolt-v3/research/naming/nt-owned-name-audit.yaml`
- Modify: `tests/config_parsing.rs`

**Interfaces:**
- Consumes: immutable NT commit SHA from Task 1.
- Produces: one consistent NT revision across manifests, lockfiles, docs, fixtures, and boundary verifiers.

- [x] **Step 1: Replace the old pin on every governed surface**

Use `rg -n 'e7af3dce0c7656862c33acb962aff5ae738eecb6|e7af3dce'` to census the predecessor pin. Replace active pin references in manifests, lockfiles, documentation, verifier constants, and fixture bodies with `a192a89f7a24e435cfba7a45b6dcd6de14622967`. Rename SHA-bearing fixtures with `a192a89f` and update every filename reference to that exact basename. Historical red/green evidence in this plan keeps the predecessor SHA explicitly labeled as such.

- [x] **Step 2: Refresh source hashes**

Compute SHA-256 for the pinned `http/parse.rs`, `execution/lifecycle.rs`, and `execution/orders.rs`. Update only the registered fixture values; the test-only commit should leave all three production hashes unchanged.

- [x] **Step 3: Refresh both lockfiles through the governed dependency workflow**

Update only `source = git+...#<sha>` and the corresponding package identities required by the pin. Confirm no unrelated package version drift.

- [x] **Step 4: Run local non-compile verification**

```bash
python3 scripts/test_verify_bolt_v3_boundary_evidence.py
python3 scripts/verify_bolt_v3_boundary_evidence.py
python3 scripts/test_verify_bolt_v3_evidence_novelty.py
python3 scripts/verify_bolt_v3_evidence_novelty.py
just fmt-check
just deny
just source-fence-static
just ci-lint-workflow
git diff --check
```

Expected: every command succeeds.

- [x] **Step 5: Commit and publish bolt**

```bash
git add Cargo.toml Cargo.lock crates/backtesting-vertical-slice/Cargo.toml crates/backtesting-vertical-slice/Cargo.lock docs scripts tests/fixtures
git commit -m "test: pin neg-risk execution regressions"
just sandbox-safe-push
```

- [x] **Step 6: Verify exact-head remote evidence**

Run `just verify-remote`. Required root and Backtester gates must complete successfully at the exact pushed SHA. Record the SHA and workflow run IDs; do not merge or claim native approval.
