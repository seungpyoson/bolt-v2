# Issue #1340 Optional Hardening Implementation Plan

> **Historical reference only.** This plan is superseded by current `AGENTS.md`; its unchecked
> steps and commands are non-operational and must not be executed.

**Goal:** Remove remaining BVS provenance hardcodes and make the execution validator reproduce unrestricted NT market-book walking with discriminating guards.

**Architecture:** BVS's existing embedded dependency proof remains the single NT revision owner. The execution contract continues to delegate economics to NT, but supplies the same unrestricted side-specific price bound used by NT's market matching engine instead of deriving a limit from observed output.

**Tech Stack:** Rust, NautilusTrader exact `Price`/`Quantity`/`Money` types, BVS embedded dependency proof, `anyhow`, nextest through remote-first verification.

## Global Constraints

- No production strategy, production configuration, sizing, submission, matching-engine, or CI workflow changes.
- Valid deterministic partial market fills remain accepted.
- No dynamic fee or rebate implementation; #843 item 10 remains authoritative.
- Rust compilation and tests are remote-first; local work uses formatting, text checks, and source fences only.

---

### Task 1: Pin market-only validation and unrestricted book walking

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`

**Interfaces:**
- Consumes: `ExecutionContractTrace`, NT `OrderBook::simulate_fills`, `Price::{max,min}`, `fixed::FIXED_PRECISION`.
- Produces: market-only validation whose simulated fills cover all executable levels and whose settlement closes the quantity actually filled.

- [ ] Add `rejects_non_market_entry_at_market_only_guard` by changing both order and position copies to `OrderType::Limit` and asserting the market-only error.
- [ ] Add a multi-level book mutation whose observed fills omit a deeper executable level; assert rejection by the deterministic-book guard. This is RED against the observed-last-fill price bound.
- [ ] Replace the observed-last-fill bound with `Price::max(FIXED_PRECISION)` for buys and `Price::min(FIXED_PRECISION)` for sells, matching NT's `determine_market_price_and_volume` implementation.
- [ ] Sum typed entry-fill quantities with exact checked `Quantity` arithmetic and require the terminal fill to close that observed total, not the requested quantity.
- [ ] Replace the former reversed-position mutation with an acceptance test for a deterministic partial fill whose terminal settlement closes exactly the filled quantity.
- [ ] Remove the redundant `Position::is_closed()` assertion: exact entry-sequence equality plus one opposite-side terminal fill for the exact filled quantity already proves closure, so the assertion cannot carry an independent mutation.
- [ ] Format with `cargo fmt` in the BVS workspace and run `just bte-fmt-check`.

### Task 2: Make BVS dependency proof the only recorded revision owner

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Modify: `crates/backtesting-vertical-slice/src/run_manifest.rs`
- Modify: `crates/backtesting-vertical-slice/src/result_contract.rs`
- Modify: BVS integration-test files containing `resolved_nt_version` literals.

**Interfaces:**
- Consumes: `nt_dependency_proof_from_embedded_manifests() -> Result<NtDependencyProof, NtDependencyProofError>`.
- Produces: manifest/test revisions derived from `NtDependencyProof.nautilus_revision`, with `lock_sources_all_resolve_to_revision == true` required before recording.

- [ ] Add a small BVS-owned helper that returns the embedded proof only after rejecting `lock_sources_all_resolve_to_revision == false`; add a synthetic lock-skew test proving rejection.
- [ ] Change issue #789 and maker-smoke manifests to consume the checked BVS proof owner.
- [ ] Replace every full stale BVS `resolved_nt_version` literal with the checked owner; for raw TOML fixtures, construct the string with `format!` and the owned revision.
- [ ] Update result-contract expectations to compare against the shared owner rather than another literal.
- [ ] Verify `rg` finds no full stale revision literal under `crates/backtesting-vertical-slice`.
- [ ] Format with `cargo fmt` in the BVS workspace and run `just bte-fmt-check`.

### Task 3: Clarify provenance semantics and verify the slice

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`
- Modify: PR description if publication evidence changes.

**Interfaces:**
- Consumes: canonical resolved configuration bytes and recorded SHA-256.
- Produces: documentation that calls this an integrity/agreement check, not a frozen configuration golden.

- [ ] Update the validator/module documentation and relevant test name/message to describe canonical-byte/hash agreement and applied-override sensitivity accurately.
- [ ] Run `just fmt-check`, `just bte-fmt-check`, and `just source-fence-static`; require exit 0.
- [ ] Run `git diff --check` and static searches for stale revision literals, `exit_price`, and historical-test skip collisions.
- [ ] Commit and publish one coherent draft head with `git push`.
- [ ] Report explicitly that Rust tests remain unexecuted unless exact-head remote evidence is obtained; do not request review, mark ready, run full CI, merge, queue, or close #1340.
