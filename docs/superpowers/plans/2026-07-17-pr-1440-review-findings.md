# PR #1440 Review Findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the confirmed PR #1440 adversarial-review findings without changing the selected NautilusTrader pin or blocking existing-risk exits.

**Architecture:** Provider bindings remain the only provider-specific capability dispatch. The binary-oracle strategy carries the derived signal capability into shared submit admission, which rejects entry intent with typed evidence while leaving pricing and risk-reducing exits intact. Source-fence evidence independently checks additive stale prose and the Polymarket fixture against the exact pinned NautilusTrader checkout.

**Tech Stack:** Rust, NautilusTrader Rust APIs, Python 3.12, pytest/unittest-style fence tests, `just` governed verification, GitHub Actions exact-head verification.

## Global Constraints

- Keep the change inside issue #1383's first executable slice; do not merge, deploy, trade, or close the issue.
- Do not introduce a second immutable NautilusTrader SHA authority or change the selected revision.
- Do not add a strategy-local submit gate or clear reference pricing; shared submit admission owns the entry rejection.
- Use evidence-driven implementation. Add focused regression evidence with each behavior change, but red-first sequencing is optional.
- Do not run local compile-heavy Rust commands. Local evidence is formatting and Python/static fences; Rust behavior evidence runs on the governed exact-head remote path.
- Use `apply_patch` for source edits and preserve unrelated user changes.

---

### Task 1: Make provider capability dispatch explicit and correct Binance Spot JSON

**Files:**
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/bolt_v3_providers/binance.rs`
- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_realized_volatility_runtime.rs`

- [ ] Add a `NewRiskMarketDataAvailabilityLoader` function type and a required `new_risk_market_data_available` field to `ProviderBinding`.
- [ ] Add one explicit neutral evaluator for providers whose market-data capability is unaffected, assign it to every non-Binance binding, and assign `binance::new_risk_market_data_available` to the Binance binding.
- [ ] Replace the venue-name conditional in `bolt_v3_providers::new_risk_market_data_available` with binding lookup and dispatch. Return an error for an unknown provider instead of inheriting `Ok(true)`.
- [ ] Change the Binance evaluator so any configured Spot data client is unavailable when `NAUTILUS_SOURCE_CAPABILITIES.binance_spot_sbe_new_risk_quorum` is false, regardless of `spot_market_data_mode`; keep clients without data config and non-Spot products available.
- [ ] Add focused provider tests covering Binance Spot SBE unavailable, Binance Spot JSON unavailable, non-Spot available, neutral bindings available, and unknown binding rejection.
- [ ] Evidence: `rg -n "new_risk_market_data_available" src/bolt_v3_providers src/bolt_v3_strategy_registration.rs tests/bolt_v3_strategy_registration.rs tests/bolt_v3_realized_volatility_runtime.rs` shows every dispatch path is binding-owned and every expected case has coverage.

### Task 2: Enforce the unavailable capability in shared submit admission

**Files:**
- Modify: `src/bolt_v3_decision_evidence.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `tests/bolt_v3_submit_admission.rs`
- Modify: all Rust request/claim fixtures found by `rg -l "BoltV3SubmitAdmissionRequest(Input)? \\{" --glob '*.rs'`

- [ ] Add an entry-capability field to `BoltV3SubmitAdmissionRequestInput`, `BoltV3SubmitAdmissionRequest`, and `BoltV3BasketSubmitSlotClaim`. Require callers to state it explicitly so new request paths cannot silently default open.
- [ ] Add `RejectedProviderCapabilityUnavailable` to `BoltV3AdmissionOutcome` and a matching typed `ProviderCapabilityUnavailable` variant to `BoltV3SubmitAdmissionError`; update display text, evidence serialization coverage, outcome keys, routing/error conversion, basket outcome conversion, and every exhaustive match.
- [ ] In `BoltV3SubmitAdmissionState::evaluate`, reject only `BoltV3SubmitIntentKind::Entry` when the capability field is false. Perform the check after lifecycle eligibility and before loss/capital reservation so the rejection cannot consume admission capacity.
- [ ] Set the field to true in generic shared execution paths that have no provider capability dependency, and explicitly propagate it through all direct request/test fixtures. Do not infer availability inside admission.
- [ ] Add focused admission tests proving an otherwise-valid entry returns the typed rejection, records the rejection outcome, consumes no capacity, and an otherwise-valid risk-reducing exit remains admissible with the same false capability field.
- [ ] Evidence: `rg -n "RejectedProviderCapabilityUnavailable|ProviderCapabilityUnavailable|new_risk_provider" src tests` accounts for the request field, every outcome conversion, evidence serialization, and paired entry/exit tests.

### Task 3: Carry the production signal capability into admission and replace the tautological strategy proof

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs` only if a reusable production-shaped helper is needed

- [ ] Pass `self.config.signal_new_risk_available` into `BoltV3SubmitAdmissionRequestInput` in `submit_admission_request_from_order`; do not modify `observe_reference_current_price`, selected pricing spot, or exit pricing.
- [ ] Replace the existing unavailable-Binance submit-permit test that retains only the blocked RV source, forces the selected spot to `None`, and omits reference data.
- [ ] Build a production-shaped regression using the normal three-source RV surface, a valid Chainlink reference price, ready non-Binance RV quorum, and `signal_new_risk_available = false`. Drive an otherwise-valid entry through shared submission and assert no submit permit plus the typed provider-capability evidence.
- [ ] Add or adapt an exit-path regression using the same false signal capability to prove a risk-reducing exit reaches shared admission and is not rejected for provider capability.
- [ ] Evidence: the replacement test contains no `sources.retain(...)` and no forced `set_selected_pricing_spot(None)` antecedent; it explicitly delivers reference and non-Binance RV inputs before attempting entry.

### Task 4: Key RV transport validation on the explicit unavailable-source set

**Files:**
- Modify: `src/bolt_v3_realized_volatility_runtime.rs`
- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`

- [ ] Expose a narrow read-only runtime query that reports whether a configured `(surface_id, source_id)` is in `new_risk_capability_unavailable_sources`.
- [ ] Remove `subscription_requests()` as the exemption authority in `validate_realized_volatility_node_transport_membership`. Require every configured enabled source to have registered transport unless the explicit runtime query marks that exact source capability-unavailable.
- [ ] Add tests proving an explicitly unavailable source receives the startup exception while an available source with a missing derived subscription/transport still produces the loud registration error.
- [ ] Evidence: `rg -n "subscription_requests|new_risk_capability_unavailable" src/bolt_v3_strategy_registration.rs src/bolt_v3_realized_volatility_runtime.rs tests/bolt_v3_strategy_registration.rs` shows the validator no longer derives its exception from the artifact it validates.

### Task 5: Reject additive stale capability prose across governed current surfaces

**Files:**
- Modify: `scripts/verify_bolt_v3_boundary_evidence.py`
- Modify: `scripts/test_verify_bolt_v3_boundary_evidence.py`
- Inspect and modify only if stale text exists: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md`
- Inspect: `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`

- [ ] Define the bounded stale positive Binance schema/receive-clock claim forms and obsolete fork-authority form as verifier constants.
- [ ] Keep the exact count of two truthful current-status claims and additionally require zero stale positive forms in the status map.
- [ ] Scan the current NautilusTrader capability section of the runtime-contract document for the same forbidden current claims without rejecting clearly historical evidence outside that governed section.
- [ ] Replace the misleading replacement-only mutation test with additive mutations that append stale positive text while retaining both truthful claims. Add one mutation for the governed runtime-contract section.
- [ ] Evidence: run `python3 scripts/test_verify_bolt_v3_boundary_evidence.py` and `python3 scripts/verify_bolt_v3_boundary_evidence.py`; both mutations must be rejected and the head tree must pass.

### Task 6: Mechanically prove Polymarket fixture bytes against the exact pinned checkout

**Files:**
- Modify: `scripts/verify_runtime_capture_yaml.py`
- Modify: `scripts/test_verify_runtime_capture_yaml.py`
- Modify: `tests/config_parsing.rs`
- Inspect: `tests/fixtures/nt_polymarket_query_post_order_params_8160730c.txt`

- [ ] Generalize the pinned-checkout locator already used for NautilusTrader API capture so it resolves the exact full revision checkout directory, not merely a short-revision path assumption.
- [ ] Parse the fixture's `Revision`, `Path`, `Full source SHA-256`, and declared extracted ranges. Require the revision to match the root Cargo pin and confine the source path beneath the resolved checkout.
- [ ] Hash the complete pinned upstream `query.rs`, compare it with the declared SHA-256, reconstruct each inclusive declared line range with original newlines, and compare those bytes exactly with the fixture body after the metadata separator.
- [ ] Invoke this check from the post-`cargo fetch --locked` runtime-capture verifier, keeping static source-fence free of an undeclared checkout prerequisite.
- [ ] Add isolated verifier tests using a temporary synthetic checkout for: valid fixture, wrong revision, wrong digest, invalid/out-of-bounds range, path escape, and one-byte fixture-body mutation.
- [ ] Keep the Rust configuration test focused on the compiled-pin revision and consumer-facing field shape; remove the tautological hardcoded digest-presence assertion. If the pinned crate exposes `PostOrderParams` publicly, add a direct serialization assertion against the real upstream type rather than another text replica.
- [ ] Evidence: run `python3 scripts/test_verify_runtime_capture_yaml.py`; then, with the existing pinned checkout, run `python3 scripts/verify_runtime_capture_yaml.py` and confirm the exact source digest/range comparison passes.

### Task 7: Format, run governed local evidence, publish exact head, and obtain remote Rust proof

**Files:**
- Modify only formatter output from the preceding tasks.

- [ ] Run `cargo fmt` through the already-approved governed command, then `just fmt-check`.
- [ ] Run the focused Python verifier suites from Tasks 5 and 6 and `just source-fence-static`.
- [ ] Run `git diff --check`, inspect `git status --short`, and review the complete diff for unrelated issue work, hardcoded runtime values, alternate paths, or accidental fixture/lockfile changes.
- [ ] Commit the coherent fix set with an issue-scoped message and publish it with `just sandbox-safe-push`; verify the remote branch head equals local `HEAD`.
- [ ] Run `just rust-probe suggest` only to document the smallest useful Rust target. Prefer the repository's exact-head `just verify-remote` gate for completion evidence because the change spans shared admission, strategy integration, provider dispatch, and source fences.
- [ ] Inspect PR #1440 exact-head checks and adjudicate any failure. Do not request external review until the branch is clean, pushed, local findings are resolved, and the required exact-head evidence is green under the current pre-cutover policy.
- [ ] Conduct the required internal adversarial review of the final diff, address any substantive finding, then request native review from the login resolving to node ID `U_kgDOEZMFhA`. Do not merge.
