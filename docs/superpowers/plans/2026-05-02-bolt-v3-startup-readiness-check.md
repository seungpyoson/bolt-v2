# bolt-v3 Startup Readiness/Check Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:test-driven-development` for the implementation and `superpowers:verification-before-completion` before claiming the branch is complete. Steps use checkbox (`- [ ]`) syntax for tracking.

**Issue:** #294

**Goal:** Add a narrow Bolt-v3 startup readiness/check contract that reports explicit startup facts from the existing config, forbidden-env, SSM secret-resolution, adapter-mapping, client-registration, and LiveNode-build boundaries before any trading path is entered.

**Architecture:** Build a library-level readiness/check module first. Do not add a CLI or launch entrypoint in this slice. The check should reuse the current Bolt-v3 startup stages and produce a fact report; it must not derive market-identity plans, connect clients implicitly, inspect NT caches, start the runner loop, register strategies, construct orders, or submit orders.

**Non-goals:**

- Do not generalize the updown-shaped `MarketIdentityPlan`; that remains #290.
- Do not derive or consume market-identity plans in this slice.
- Do not perform EC2 lifecycle cleanup; that remains #292.
- Do not add a Bolt-v3 binary, `just check`, strategy runtime, allocation loop, reconciliation loop, or trading launch path.
- Do not introduce aggregate booleans or names such as `ready`, `can_trade`, `tradable`, or `entry_ready`.

---

## Current Source Boundaries

- `src/bolt_v3_live_node.rs::build_bolt_v3_live_node_with_summary` already executes the no-connect startup build order: forbidden env check, injected SSM secret resolution, no-identity adapter mapping, client registration, and `LiveNodeBuilder::build`.
- `src/bolt_v3_live_node.rs::connect_bolt_v3_clients` is explicit, opt-in, bounded, and documented as dispatch plus connected check only. It does not prove NT cache or instrument readiness.
- `src/bolt_v3_adapters.rs::map_bolt_v3_adapters` intentionally uses an empty `MarketIdentityPlan`. Target/family compatibility is already covered by `map_bolt_v3_adapters_with_market_identity` tests and stays outside this readiness slice until #290.
- `src/bolt_v3_client_registration.rs::register_bolt_v3_clients` returns `BoltV3RegistrationSummary`, which is the right fact source for registered data/execution client kinds.
- `src/bolt_v3_secrets.rs::resolve_bolt_v3_secrets_with` and `check_no_forbidden_credential_env_vars_with` are already test-injectable and should be reused directly.

---

## Planned File Structure

### New source files

- `src/bolt_v3_readiness.rs`
  Library-level startup check report types and test-injectable orchestration.

### New tests

- `tests/bolt_v3_readiness.rs`
  End-to-end fixture tests for the readiness/check report and representative failure stages.

### Modified files

- `src/lib.rs`
  Export `bolt_v3_readiness`.

- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md`
  Add a short startup-check contract section stating what the check proves and does not prove.

- `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
  Classify any new production diagnostic literals introduced by `src/bolt_v3_readiness.rs`.

---

## Data Model

The report should name facts, not collapse them into a launch decision.

```rust
pub struct BoltV3StartupCheckReport {
    pub facts: Vec<BoltV3StartupCheckFact>,
}

pub struct BoltV3StartupCheckFact {
    pub stage: BoltV3StartupCheckStage,
    pub subject: BoltV3StartupCheckSubject,
    pub status: BoltV3StartupCheckStatus,
    pub detail: String,
}

pub enum BoltV3StartupCheckStage {
    ForbiddenCredentialEnv,
    SecretResolution,
    AdapterMapping,
    LiveNodeBuilder,
    ClientRegistration,
    LiveNodeBuild,
}

pub enum BoltV3StartupCheckStatus {
    Satisfied,
    Failed,
    Skipped,
}

pub enum BoltV3StartupCheckSubject {
    Root,
    Venue(String),
    BlockedByStage(BoltV3StartupCheckStage),
}
```

Implementation note: do not add `is_ready`, `ready`, `can_trade`, `tradable`, or `entry_ready` fields or methods. If callers need to decide how to render failures, they can inspect the facts and statuses.

---

## Task 1: Pin the report contract with failing tests

**Files:**

- Create: `tests/bolt_v3_readiness.rs`

- [ ] At the top of `tests/bolt_v3_readiness.rs`, declare `mod support;` so the test can reuse `tests/support/mod.rs`, mirroring the existing Bolt-v3 integration tests.

- [ ] Add `startup_check_reports_success_facts_without_connecting`.
  - Load `tests/fixtures/bolt_v3/root.toml`.
  - Call `run_bolt_v3_startup_check_with(&loaded, |_| false, support::fake_bolt_v3_resolver)`.
  - Assert facts exist for:
    - `ForbiddenCredentialEnv`
    - `SecretResolution`
    - `AdapterMapping`
    - `LiveNodeBuilder`
    - `ClientRegistration`
    - `LiveNodeBuild`
  - Assert the registration fact names each venue and whether data/execution clients were registered.
  - Do not assert on NT connection state; this slice does not connect.

- [ ] Add `startup_check_reports_forbidden_env_failure_and_skips_downstream`.
  - Use an env predicate that marks a provider-owned forbidden credential env var as present.
  - Assert a `Failed` fact at `ForbiddenCredentialEnv`.
  - Assert exactly these downstream stages are `Skipped`, not silently absent and not marked satisfied: `SecretResolution`, `AdapterMapping`, `LiveNodeBuilder`, `ClientRegistration`, `LiveNodeBuild`.

- [ ] Add `startup_check_reports_secret_resolution_failure_without_secret_value`.
  - Use a resolver that returns an error for one fixture SSM path.
  - Assert `SecretResolution` is `Failed`.
  - Assert the failing fact includes venue/field/path context from `BoltV3SecretError`.
  - Assert exactly these downstream stages are `Skipped`: `AdapterMapping`, `LiveNodeBuilder`, `ClientRegistration`, `LiveNodeBuild`.

- [ ] Add `startup_check_reports_adapter_mapping_failure`.
  - Mirror `tests/bolt_v3_adapter_mapping.rs::adapter_mapper_rejects_subscribe_new_markets_true_if_validation_was_bypassed`: mutate `polymarket_main` so `data.subscribe_new_markets = true` after loading the canonical fixture.
  - Use the same TOML mutation pattern as the existing test:
    `loaded.root.venues.get_mut("polymarket_main").unwrap().data.as_mut().unwrap().as_table_mut().unwrap().insert("subscribe_new_markets".into(), toml::Value::Boolean(true));`
  - Assert `AdapterMapping` is `Failed`.
  - Assert exactly these downstream stages are `Skipped`: `LiveNodeBuilder`, `ClientRegistration`, `LiveNodeBuild`.
  - Assert no fake secret values returned by `support::fake_bolt_v3_resolver` appear in any serialized fact. This redaction assertion belongs on this later-stage failure because secret resolution has already succeeded.

- [ ] Add `startup_check_reports_livenode_builder_failure_and_skips_downstream` if a deterministic fixture can trigger `make_bolt_v3_live_node_builder` failure.
  - Prefer a minimal loaded-config mutation such as an invalid `trader_id` only if it fails in `make_bolt_v3_live_node_builder` itself and not earlier validation-like stages.
  - Assert `LiveNodeBuilder` is `Failed`.
  - Assert exactly these downstream stages are `Skipped`: `ClientRegistration`, `LiveNodeBuild`.
  - If no reliable fixture exists, state that explicitly in the test file and keep the implementation branch focused on the failure modes that can be triggered without test-only production hooks.

- [ ] Add `startup_check_reports_registration_failure_and_skips_build` if a deterministic fixture can trigger `BoltV3ClientRegistrationError` without adding test-only production API.
  - If no practical fixture exists, state that explicitly in the test file and rely on `tests/bolt_v3_client_registration.rs` for direct registration-error coverage while this readiness test suite exercises the skip-chain machinery through earlier-stage failures.

- [ ] Add `startup_check_source_does_not_expose_launch_booleans`.
  - Use `include_str!("../src/bolt_v3_readiness.rs")`.
  - Assert no identifier token matches `\b(entry_ready|can_trade|tradable|ready)\b`; do not use a broad substring check that would trip on `readiness` or unrelated prose.
  - This is a regression fence for the explicit-facts contract.

- [ ] Add `startup_check_source_remains_no_trade`.
  - Use `include_str!("../src/bolt_v3_readiness.rs")`.
  - Treat this as a code-token fence, not a prose ban. Either strip comments and string literals before scanning, or use identifier-boundary matching and keep documentation matches from failing the test.
  - Assert no code-token occurrence of `connect_bolt_v3_clients`, `disconnect_bolt_v3_clients`, `subscribe_*`, `submit_order`, `runner`, or `run_trader`.
  - Keep forbidden tokens in the test file, not in `src/bolt_v3_readiness.rs`.

Run the tests and confirm they fail before implementation:

```bash
cargo test --test bolt_v3_readiness -- --nocapture
```

---

## Task 2: Add the readiness module and report types

**Files:**

- Create: `src/bolt_v3_readiness.rs`
- Modify: `src/lib.rs`

- [ ] Add `pub mod bolt_v3_readiness;` to `src/lib.rs`.
- [ ] Define the report, fact, stage, status, and subject types.
- [ ] Implement small report helpers that append facts but do not expose launch-decision booleans.
- [ ] Implement `Display` only where needed for diagnostics; every new production literal must be audited by `verify_bolt_v3_runtime_literals`.
- [ ] Keep `src/bolt_v3_readiness.rs` provider-neutral and market-family-neutral. It must route provider/family details through existing neutral surfaces and runtime venue-key strings, never through concrete imports such as `bolt_v3_providers::{binance, polymarket}` or `bolt_v3_market_families::*`. The provider-leak verifier's `discovered_core_files` glob auto-discovers `src/bolt_v3_readiness.rs`; any concrete provider or market-family import should fail CI.

Expected public function shape:

```rust
pub fn run_bolt_v3_startup_check_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> BoltV3StartupCheckReport
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display;
```

Do not add a production SSM-session variant in the first pass unless the tests and docs need it. The production `build_bolt_v3_live_node` path already owns the real `SsmResolverSession`; this issue is the report contract.

---

## Task 3: Reuse the existing build stages without duplicating them

**Files:**

- Modify: `src/bolt_v3_readiness.rs`

- [ ] Do not wrap builder creation, client registration, and final `builder.build()` in one readiness helper. The report needs a fact boundary at each observed call.
- [ ] Keep `build_bolt_v3_live_node`, `build_bolt_v3_live_node_with`, and `build_bolt_v3_live_node_with_summary` behavior unchanged. Do not refactor `src/bolt_v3_live_node.rs` unless compiler visibility forces it; `make_bolt_v3_live_node_builder` and `register_bolt_v3_clients` are already public.
- [ ] In `run_bolt_v3_startup_check_with`, execute stages in this order:
  1. `check_no_forbidden_credential_env_vars_with`
  2. `resolve_bolt_v3_secrets_with`
  3. `map_bolt_v3_adapters`
  4. `make_bolt_v3_live_node_builder`
  5. `register_bolt_v3_clients`
  6. `builder.build`
- [ ] After each successful stage, append a `Satisfied` fact with a concrete subject.
- [ ] On first failure, append a `Failed` fact and append `Skipped` facts for downstream stages with a detail that names the blocked stage.
- [ ] If `make_bolt_v3_live_node_builder` fails, emit `LiveNodeBuilder: Failed`, then mark `ClientRegistration` and `LiveNodeBuild` as skipped.
- [ ] Accept that `make_bolt_v3_live_node_builder` currently returns `anyhow::Result<LiveNodeBuilder>`. For this slice, render that builder-construction error as a `LiveNodeBuilder` fact detail string and keep a typed builder-error refactor out of scope unless implementation proves the untyped detail is unusable.
- [ ] If `register_bolt_v3_clients` fails, emit `ClientRegistration: Failed`, preserve any error detail without secret values, and mark `LiveNodeBuild` as skipped.
- [ ] If `builder.build` fails, emit `ClientRegistration: Satisfied` with the `BoltV3RegistrationSummary` already returned by `register_bolt_v3_clients`, then emit `LiveNodeBuild: Failed`.
- [ ] Drop the built `LiveNode` immediately after the build fact. Do not call `connect_bolt_v3_clients`, `disconnect_bolt_v3_clients`, any `subscribe_*` API, any runner API, or any order API.

---

## Task 4: Documentation and literal audit

**Files:**

- Modify: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md`
- Modify: `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`

- [ ] Add a concise section near the existing controlled-connect notes:
  - The startup check proves config/secret/adapter/client-registration/build facts.
  - The startup check does not prove strategy registration, runner-loop safety, trading readiness, market selection, order submission, connectedness, NT cache contents, or instrument availability.
  - Controlled connect remains explicit and separate.
- [ ] Run the runtime-literal verifier.
- [ ] Classify any new production strings introduced in `src/bolt_v3_readiness.rs`.

---

## Task 5: Verification

Run the focused checks first:

```bash
cargo test --test bolt_v3_readiness -- --nocapture
cargo test --test bolt_v3_client_registration -- --nocapture
cargo test --test bolt_v3_adapter_mapping -- --nocapture
python3 scripts/test_verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_runtime_literals.py
```

Then run the repo gate used by CI:

```bash
just fmt-check
```

Before requesting review, confirm:

- [ ] `git status --short` is clean.
- [ ] The branch has one scoped commit for #294.
- [ ] The PR description says this is the startup readiness/check slice only.
- [ ] The PR does not claim to close #236, #290, or #292.
- [ ] The exact PR head has green CI before external review is requested.

---

## Review Prompt

Use this after the implementation branch is pushed and CI is green:

```text
Adversarially review PR <number> at exact head <sha>.

Scope: issue #294 only, Bolt-v3 startup readiness/check contract. This PR must report explicit startup facts and must not implement strategy runtime, trading launch, #290 MarketIdentityPlan generalization, EC2 lifecycle cleanup, or a Bolt-v3 CLI.

Review questions:
1. Does the report expose explicit facts by stage/subject, or did it smuggle in aggregate launch booleans like ready/can_trade/tradable/entry_ready?
2. Does the implementation reuse existing Bolt-v3 startup boundaries instead of duplicating config, SSM, adapter, registration, or LiveNode-build logic?
3. Does it preserve distinct facts for LiveNode builder creation, client registration, and final LiveNode build instead of bundling them behind one helper?
4. Does it remain no-trade: no connect unless explicitly named and tested, no runner loop, no strategy actor activation, no market selection, no order construction, no order submission?
5. Does it avoid pulling on #290 by deriving/consuming MarketIdentityPlan or importing concrete market-family modules into the new core readiness file?
6. Are failure facts specific enough for operators without leaking secret values?
7. Are the tests representative for success, forbidden env failure, secret resolution failure, adapter mapping failure, registration/build failure where practical, skipped downstream stages, no-trade source tokens, and no launch-booleans?
8. Do provider-leak and runtime-literal verifiers still cover the new file?

Findings should include severity, file/symbol, what is wrong, why it matters architecturally, recommended correction, and whether it blocks merge.
```
