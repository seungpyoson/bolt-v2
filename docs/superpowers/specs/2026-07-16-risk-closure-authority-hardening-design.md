# Risk-Closure Authority Hardening Design

## Goal

Close the confirmed review findings in PR #1430 without expanding beyond the #1382 risk-closure workspace tracer bullet.

## Authority ownership

`RiskClosureWorkspaceAuthority` remains a cloneable handle to one shared authority, but production code receives no public constructor in this slice. Construction stays private and test-only while production activation and application resource-ledger ownership remain deferred to #1382. This prevents callers from creating independent ten-slot authorities that multiply the configured capacity and memory bound.

The unused configuration accessors are removed. In-module tests read private configuration fields directly.

The runtime storage-replacement API is removed. The configured buffers are allocated once with the authority and released only when the authority is dropped. This prevents a replacement operation from temporarily holding both the old and new 160 MiB allocations.

## Closure incarnation binding

Every authority owns an opaque, non-serializable instance identity. Every committed reservation also retains an opaque closure-generation value derived from the authority's non-repeating internal lease sequence.

The retained slot, recovery lease, authoritative durable terminal transition, and `TerminalReleasePermit` all carry the same authority identity and closure generation. `release_terminal` validates authority identity, closure identity, and closure generation while holding the metadata lock. A permit from another authority or an earlier reuse of the same `ClosureIdentity` is rejected.

Every failure returns `TerminalReleaseFailure` containing the original active lease and permit. Successful release consumes both. Permit construction remains private and test-only until #1382 provides the authoritative durable-transition integration; no boolean or public constructor is introduced.

## Configuration fence

The dedicated verifier reads `arena_bytes` and `slot_bytes` from the sole TOML authority. It rejects direct literals and supported constant expressions equal to either configured value outside generated Rust. Capacity remains derived from arena geometry rather than value-fenced because its small numeric value is not unique.

The TOML census scans repository TOML files rather than only `config/`, excluding repository metadata, worktrees, build outputs, and test fixtures explicitly created outside the production tree. The private generated configuration type and constant remain accessible only inside the owner module.

The generator's fail-closed cases move into the already governed `test_verify_risk_closure_workspace_authority.py` suite, and the orphan generator test file is removed. This keeps one automatic discovery route.

## Compiler-negative coverage

The governed nextest member module continues to invoke `rustc` for negative snippets. This is a compiler test executed by the governed remote Rust lane, not local agent verification. It will additionally prove that a permit cannot be reused after terminal release and that reservation and lease values cannot be constructed through private fields where stable diagnostics permit.

The harness will continue using the lane's `rustc` from `PATH`. Cargo does not guarantee a compile-time `RUSTC` value for `env!`, so the suggested replacement is not portable. Exact-head remote execution remains the toolchain proof.

## Evidence

- Unit tests prove one slot callback cannot block another, panic isolation, atomic duplicate rejection, recoverable release failures, cross-authority permit rejection, stale-generation permit rejection, and successful matching release.
- One governed Rust test allocates the actual generated ten-slot configuration and verifies the real reserved byte count and capacity boundary.
- Compiler-negative tests prove clone, forgery, and post-consumption reuse failures.
- Python tests prove repository-wide TOML census, both configured byte values, generated drift, schema closure, integral geometry, and disabled activation.
- Local evidence uses formatting, Python verifier tests, dependency policy, CI lint, and `source-fence-static`. Rust proof comes from exact-head remote CI.

## Deliberate boundary

This hardening does not add production permit issuance, Capsule persistence, the full resource ledger, production activation, deployment, merge authority, or trading authority. Those remain tracked by #1382 or their owning slices.
