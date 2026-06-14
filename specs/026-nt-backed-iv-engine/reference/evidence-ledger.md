# IV Engine Evidence Ledger

**Feature**: `specs/026-nt-backed-iv-engine/`
**Basis**: `origin/main` at `c1b1f7b49414008a11af11da24ebc49762debf54`, branch head `f994ae15198502aee9227aea5e813d12b8d5bf92` before Phase 1 edits
**Purpose**: Record current-main evidence before implementing the NT-backed IV engine.

## Current Main Search Evidence

- `rg -n "bolt_v3_iv" src tests specs/026-nt-backed-iv-engine`: no current source or test implementation exists outside the design packet.
- `rg --files src`: existing crate modules are flat `src/bolt_v3_*` files plus nested provider/strategy modules.
- `src/lib.rs:1-44`: crate exports existing `bolt_v3_*` modules; no `bolt_v3_iv` export exists before T006.
- `src/bolt_v3_config.rs:452`: root TOML loading entrypoint is `load_bolt_v3_config`.
- `src/bolt_v3_config.rs:59`, `src/bolt_v3_config.rs:83`, and related config types use `#[serde(deny_unknown_fields)]`.
- `src/bolt_v3_live_node.rs:57-58`: existing live-node assembly uses NT `LiveNodeBuilder` and `LiveNodeConfig`.
- `src/bolt_v3_live_node.rs:1877-1889`: existing live-node path registers data/execution clients and strategies.
- `src/bolt_v3_strategy_registration.rs:96-131`: current strategy registration is through an injected generic binding boundary.
- `justfile:163`: repository `source-fence` recipe exists, but it has no IV-specific fence before this feature.
- `Cargo.toml:25-46`: direct NT dependencies are pinned to one explicit NautilusTrader git revision.
- `Cargo.lock:4463-5165`: locked NT packages resolve to the same NautilusTrader git revision.

## Requirement Evidence Status

| Requirements | Current-main evidence | Status before implementation |
|---|---|---|
| `FR-001`, `FR-002`, `FR-042`, `FR-047`, `FR-054` | Cargo pin evidence exists in `Cargo.toml`/`Cargo.lock`; no IV capability resolver, ledger generator, seed scan, whole-checkout sweep, or classification fixture exists. | Missing |
| `FR-003`, `FR-004`, `FR-005`, `FR-006`, `FR-024`, `FR-041` | Live-node client/strategy registration exists in `src/bolt_v3_live_node.rs`; no IV subscription planner or IV source lifecycle module exists. | Missing |
| `FR-007`, `FR-008`, `FR-009`, `FR-010`, `FR-011`, `FR-012`, `FR-013`, `FR-018`, `FR-019`, `FR-022`, `FR-025`, `FR-038`, `FR-044`, `FR-045` | Current source has no IV raw-event, indexed product, provenance, audit reader, retention, smile, surface, aggregate greeks, or custom IV evidence model. Existing `src/bolt_v3_decision_evidence.rs` is strategy evidence, not an IV store. | Missing |
| `FR-014`, `FR-015`, `FR-028`, `FR-029`, `FR-031`, `FR-043`, `FR-046` | Current strategy registration is generic, and `just source-fence` exists; no IV query handle, strategy authorization, live-node IV binding, or IV-specific source-fence case exists. | Missing |
| `FR-016`, `FR-017`, `FR-026`, `FR-040`, `FR-048` | Cargo pins NT, but current source has no IV helper policy, derived input set, helper invocation wrapper, or typed derived-IV rejection matrix. | Missing |
| `FR-020`, `FR-021`, `FR-032`, `FR-033`, `FR-034`, `FR-035`, `FR-036`, `FR-037`, `FR-039`, `FR-049`, `FR-050`, `FR-051`, `FR-052`, `FR-053` | Existing root/strategy config uses TOML and deny-unknown-fields patterns; no IV profile schema, selector union, selector authorization, audit policy, projection/interpolation/fallback/quorum policy, numeric bounds, or schema-version validation exists. | Missing |
| `FR-027` | No IV core logic exists yet. Existing hardcode/fence risk is tracked for IV-specific source-fence tasks before runtime behavior lands. | Missing for IV |
| `FR-030` | The design packet explicitly records FV/RV as out of scope. No code change is required for this documentation requirement. | Implemented in spec docs |

## Success-Criteria Evidence Status

| Criteria | Current-main evidence | Status before implementation |
|---|---|---|
| `SC-001`, `SC-015`, `SC-018` | No IV capability test target exists. | Missing |
| `SC-002`, `SC-009`, `SC-013`, `SC-023`, `SC-024` | TOML parsing patterns exist in `src/bolt_v3_config.rs` and `tests/config_parsing.rs`; no IV profile fixture or IV schema tests exist. | Missing |
| `SC-003`, `SC-004`, `SC-008`, `SC-011`, `SC-014`, `SC-020`, `SC-021` | No IV ingest/store/provenance/audit tests exist. | Missing |
| `SC-005`, `SC-017`, `SC-019` | No IV derivation/helper-policy tests exist. | Missing |
| `SC-006`, `SC-010`, `SC-016`, `SC-022` | Generic strategy registration and source-fence infrastructure exist; no IV query-handle, authorization, live integration, or IV source-fence tests exist. | Missing |
| `SC-007` | No IV core logic exists, so the IV hardcode fence must be added before runtime behavior can be accepted. | Missing |

## Prior Work And Scope Evidence

- Closed unmerged PR `#608` is reference-only and cannot be treated as accepted implementation.
- Open issues reviewed for overlap: `#158`, `#488`, `#493`.
- No open issue or PR is fully ported by this feature as of the current overlap search.
- FV, RV, market-maker behavior, broad sidecar collectors, and venue-specific collection are out of scope for this feature.
