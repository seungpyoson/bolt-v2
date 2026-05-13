# Implementation Plan: Phase 7 No-submit Readiness

**Branch**: `017-bolt-v3-phase7-no-submit-readiness-fresh` | **Date**: 2026-05-14 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/002-phase7-no-submit-readiness/spec.md`

## Summary

Add a fresh Phase 7 no-submit readiness evidence path from current main. The slice reuses bolt-v3 config, SSM-only secret resolution, live-node build, client registration, and NT-owned lifecycle/cache surfaces. It writes a redacted readiness report accepted by the existing live-canary gate only after reference data required by the loaded strategies is visible through NT-owned cache evidence. It proves no submit, cancel, replace, amend, runner-loop, or live-capital action can occur in Phase 7 default flow.

## Technical Context

**Language/Version**: Rust, current repository toolchain
**Primary Dependencies**: NautilusTrader Rust API, `aws-sdk-ssm`, existing bolt-v3 config/secret/live-node modules, `serde`, `serde_json`, `tempfile` for tests
**Storage**: Config-selected JSON report file only; no tracked secret artifact
**Testing**: `cargo test`, source-fence tests, `cargo fmt --check`, `git diff --check`, no-mistakes, external review gates
**Target Platform**: bolt-v3 Rust binary on operator host
**Project Type**: Rust binary/library with integration tests
**Performance Goals**: Bounded readiness run by existing configured NT timeouts; no unbounded loops; report size bounded by `[live_canary]` byte cap
**Constraints**: No hardcodes for runtime values, no dual readiness path, SSM-only secrets, pure Rust, no Python runtime layer, zero order placement, no live capital
**Scale/Scope**: One Phase 7 slice; local tests plus ignored operator harness; no Phase 8 live order

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **NT-First Thin Layer**: PASS. Plan uses existing NT start/stop lifecycle and cache evidence for authenticated readiness and does not implement adapter, cache, order, fill, cancel, or reconciliation behavior.
- **Generic Core, Concrete Edges**: PASS. Plan keeps readiness generic over loaded config and existing registration surfaces. Provider-specific checks stay in config/adapter/reference client surfaces.
- **Single Path And Config-Controlled Runtime**: PASS. Plan uses bolt-v3 root TOML and `[live_canary]` report path. Credentials use SSM through Rust SDK only.
- **Test-First Safety Gates**: PASS. Tasks require one failing behavior/source-fence test before implementation.
- **Evidence Before Claims**: PASS. Local proof and real operator proof are separated. Phase 8 remains blocked without real no-submit report plus strategy-input safety audit.
- **Minimal Slice Discipline**: PASS. One branch covers Phase 7 readiness only. Phase 8/9 remain separate.

## Current Main Evidence

- Current authoritative main: `d6f55774c32b71a242dcf78b8292a7f9e537afab`.
- Production entrypoint builds and runs through bolt-v3 live node: `src/main.rs`.
- `BoltV3LiveNodeRuntime` is current runtime carrier; stale `BoltV3BuiltLiveNode` from PR #319 must not return.
- Existing startup readiness is build-only: `src/bolt_v3_readiness.rs`.
- Existing controlled NT boundaries include `connect_bolt_v3_clients` and `disconnect_bolt_v3_clients` in `src/bolt_v3_live_node.rs`, but current implementation evidence shows connect success alone is not reference readiness.
- NT `LiveNode::start()` connects data clients, flushes instrument events into NT cache, connects execution clients, performs startup reconciliation, and starts strategy shells without entering `LiveNode::run()`.
- Existing live-canary gate validates no-submit readiness report shape in `src/bolt_v3_live_canary_gate.rs`.
- Existing Phase 6 submit admission must be preserved in `src/bolt_v3_submit_admission.rs`.

## Phase 0 Research Summary

Detailed decisions are in [research.md](research.md).

- Use a new `src/bolt_v3_no_submit_readiness.rs` module for report model, redaction, and sequencing.
- Use a small shared schema module only if it removes duplicated report-key literals between producer and gate.
- Do not expose general `node_mut`; prefer current-main-safe helpers inside `src/bolt_v3_live_node.rs` that run bounded NT start/stop and inspect required reference instruments through NT cache against the opaque runtime.
- Keep real SSM/venue readiness behind an ignored operator test requiring explicit approval inputs.
- Do not update `config/live.local.toml`; it is legacy render input.

## Implementation Discovery

Initial Phase 7 implementation proved controlled connect/disconnect can produce redacted reports, but it also proved that connect success cannot honestly satisfy `reference_readiness`: bolt-v3 currently has no no-run reference snapshot/read proof, and the strategy consumes `ReferenceSnapshot` from msgbus only after strategy shell subscription. The revised path must therefore use NT `LiveNode::start()`/`stop()` without `run()` so NT owns data-client connection, data-event flush into cache, execution-client connection, and cleanup. The readiness stage passes only when every `[reference_data.*]` instrument required by every loaded strategy is present in NT cache after controlled start.

The implementation must bound cache inspection with the existing configured live-node timeout instead of a one-shot check. It must always call and record `LiveNode::stop()` after any start attempt, including reference-cache failure or partial startup failure. Strategy `on_start()` behavior is allowed only as an NT-owned startup side effect; Phase 7 source fences must not be misread as runtime-subscription fences, and submit admission remains unarmed during the readiness window.

## Phase 1 Design Summary

Design details are in [data-model.md](data-model.md), [contracts/no-submit-readiness.md](contracts/no-submit-readiness.md), and [quickstart.md](quickstart.md).

## Project Structure

### Documentation (this feature)

```text
specs/002-phase7-no-submit-readiness/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── no-submit-readiness.md
├── checklists/
│   ├── requirements.md
│   └── phase7-requirements.md
└── tasks.md
```

### Source Code (repository root)

```text
src/
├── bolt_v3_live_canary_gate.rs
├── bolt_v3_live_node.rs
├── bolt_v3_no_submit_readiness.rs
├── bolt_v3_no_submit_readiness_schema.rs
└── lib.rs

tests/
├── bolt_v3_no_submit_readiness.rs
├── bolt_v3_no_submit_readiness_operator.rs
└── support/
```

**Structure Decision**: Single Rust crate. Phase 7 adds a narrow readiness producer and tests; it does not add a second runtime, strategy, adapter, or provider framework.

## Complexity Tracking

No constitution violations.
