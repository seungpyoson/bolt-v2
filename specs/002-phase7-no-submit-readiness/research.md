# Phase 7 Research

## Decision: Start from current main, not stale stacked PRs

**Decision**: Use `main == origin/main == d6f55774c32b71a242dcf78b8292a7f9e537afab` as base. Treat PR #318/#319/#320 as forensic input only.

**Rationale**: PR #319 references stale `BoltV3BuiltLiveNode` and `node_mut` design. Current main uses opaque `BoltV3LiveNodeRuntime` and Phase 6 submit admission. Porting stale runtime wrappers would regress accepted Phase 6 boundaries.

**Alternatives considered**: Rebase #319 or merge #318/#319/#320. Rejected because user forbids stale branch continuation and the stale branch tree conflicts with current main.

## Decision: No-submit readiness is separate from startup readiness

**Decision**: Keep `src/bolt_v3_readiness.rs` as build/startup readiness only. Add a new Phase 7 no-submit readiness module.

**Rationale**: Startup readiness intentionally does not connect clients. Phase 7 requires authenticated SSM and venue controlled connect/readiness/disconnect evidence. Relabeling build-only readiness would create false live-readiness claims.

**Alternatives considered**: Extend startup readiness directly. Rejected because it would weaken existing source fences and blur build-only vs authenticated-connect evidence.

## Decision: Reuse current live-node build and controlled-connect boundaries

**Decision**: Build through current bolt-v3 live-node path, then run a narrow controlled-connect/readiness/disconnect sequence without runner entry.

**Rationale**: This keeps NT ownership intact and avoids a duplicate readiness architecture. `connect_bolt_v3_clients` and `disconnect_bolt_v3_clients` already centralize bounded NT client operations.

**Alternatives considered**: Add a separate mock venue world or direct client connection stack. Rejected as dual path and not NT-first.

## Decision: Do not expose raw mutable LiveNode as a public API

**Decision**: If implementation needs runtime internals, add a current-main-safe helper near `BoltV3LiveNodeRuntime` that executes the controlled readiness sequence without exposing general `node_mut`.

**Rationale**: Phase 6 intentionally made `BoltV3LiveNodeRuntime` opaque. A broad mutable escape hatch would weaken submit admission and runtime-capture boundaries.

**Alternatives considered**: Restore stale `BoltV3BuiltLiveNode::node_mut`. Rejected because it is stale branch design and conflicts with current main.

## Decision: Report schema is shared with live-canary gate

**Decision**: Add shared constants only for report keys/status strings consumed by both producer and gate.

**Rationale**: The producer and gate must agree on report shape. Shared constants remove a real duplication without adding a new framework.

**Alternatives considered**: Keep private string literals in both modules. Rejected because schema drift would be easy and Phase 8 depends on exact compatibility.

## Decision: Real SSM/venue readiness stays ignored and approval-gated

**Decision**: Add an ignored operator harness. Default tests prove it is ignored and rejects missing/mismatched approval before secret resolution.

**Rationale**: Real SSM and venue connectivity are side-effect-bearing and may expose auth/geo/market failures. They must not run by default or without operator approval.

**Alternatives considered**: Run real no-submit readiness in normal CI. Rejected due secret access, network dependence, and approval requirements.

## Decision: Phase 8 remains blocked

**Decision**: Phase 7 does not authorize tiny live order or soak. Phase 8 requires real no-submit report plus separate strategy-input safety audit.

**Rationale**: Current evidence found Chainlink feed/environment uncertainty and `config/live.local.toml` is legacy render input. Live action requires exact current config, feed path, strategy math, economics, approval, and command.

**Alternatives considered**: Treat local Phase 7 tests as enough for Phase 8 implementation readiness. Rejected because local tests are not live readiness.
