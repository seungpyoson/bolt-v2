# Research: Managed Rust Cache Retention

## Decision: Work in bolt-v2, not claude-config

Rationale: PR #398 moved Rust verification ownership into `bolt-v2/scripts/rust_verification.py`; PR #794 made claude-config hooks discover repo-local owners. `#286` now belongs in bolt-v2.

Alternatives considered:
- Continue old claude-config verifier branch: rejected, superseded by #398/#794.
- New external verifier repo: not needed for immediate `#286`; repo-local owner now exists.

## Decision: Do not run local Cargo for this planning/test slice

Rationale: The cache-retention behavior can be tested with fake target trees and process-detector seams. Real Cargo would consume disk and is not needed to validate status/prune behavior.

Alternatives considered:
- Run `just test`: rejected for this planning slice because it is expensive and unrelated.
- Rely only on CI: insufficient for local disk status/prune behavior.

## Decision: Dry-run first

Rationale: The cache is useful hot state. Blind deletion trades disk pressure for cold rebuild cost and violates #286.

Alternatives considered:
- Delete `target/debug`: rejected as one-off cleanup, not policy.
- Delete whole managed root: rejected by issue acceptance.

## Decision: Keep #374 separate

Rationale: Repo-local `target/` and `/private/tmp/bolt-v2-*` bundles are real current consumers, but #374 owns shell bypass, temp bundles, and worktree lifecycle. Mixing them into #286 would make one PR too broad.
