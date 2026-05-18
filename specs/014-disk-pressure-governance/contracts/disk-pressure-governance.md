# Contract: Disk Pressure Governance

## Issue-To-PR Mapping

| Issue | Role | Planned PR mapping |
|---|---|---|
| #123 | Epic/governance | This planning PR only; does not close child implementation |
| #48 | Investigation history | No new implementation PR; forward work maps to #374 or #286 |
| #70 | Closed investigation | No PR unless new evidence reopens the symptom |
| #124 | Managed-cache symptom | No direct implementation PR; forward work maps to #286 |
| #125 | Claude task-output incident anchor | bolt-v2-side anchor only; implementation belongs to `seungpyoson/claude-config#597` |
| #286 | Managed Rust cache retention | One implementation PR for status/prune/retention policy |
| #374 | Wrapper hardening | One implementation PR after Phase 1 cargo-path enumeration is pinned |
| #375 | Developer-tool hygiene | One implementation PR after Phase 1 developer-tool enumeration is pinned |
| #376 | Uncovered surface inventory | One investigation/doc PR; follow-up implementation issue created from inventory |
| #377 | Unknown-class detection | One implementation PR after Phase 1 detection-surface enumeration is pinned |

## Local Rust Verification Contract

- Broad verification belongs to exact-head GitHub CI after a draft/open PR exists.
- Use `just test <args>` only for managed targeted local tests with an explicit reason.
- Use `just clippy`, `just build`, `just check-aarch64`, or explicit rust-verification owner commands only when local evidence is necessary and disk preflight passes.
- Do not use raw `cargo test`, `command cargo`, no-mistakes raw `cargo`, or agent shell cargo commands until #374 proves routing for that launcher.
- Full local test suite is allowed only after disk preflight, routing proof, and a recorded reason CI cannot provide the needed signal.
- S3 is allowed for immutable deploy artifacts/evidence only, not active Cargo target directories.

## Verified Cargo Routing Evidence

Snapshot scope: operator `spson`, local machine, 2026-05-18.

- `just` resolves bolt-v2 Rust target output to `/Users/spson/.cache/rust-verification/bolt-v2/target`.
- Checked bolt-v2 worktrees resolve to the same managed target path.
- no-mistakes v1.18.3 had no `CARGO_TARGET_DIR` in the daemon environment during the 2026-05-18 check.
- `.no-mistakes.yaml` currently configures raw `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`.
- A recorded no-mistakes bolt-v2 run wrote into `/Users/spson/.no-mistakes/worktrees/70a6a97b9d39/01KRQ799H1N51HC3PH5ZNKMWN9/target/...` and failed with `No space left on device`.

## Cleanup Safety Contract

- Status/dry-run before apply.
- Refuse apply when matching active writer/build processes are detected unless #286 defines a reviewed emergency override.
- Never unlink live Claude `.output` files as a reclaim mechanism.
- Never delete whole managed target cache by default.
- Never remove pinned or active Rust toolchains.
- Never print secrets, raw approval IDs, private keys, or credential values.

## Review Gate Contract

For every implementation PR in this epic:

1. Red test/verifier observed.
2. Minimal implementation observed green.
3. `git diff --check` passes.
4. Relevant targeted local checks pass through managed entrypoints.
5. Branch is clean and pushed.
6. Exact-head CI is green.
7. no-mistakes approves or findings are resolved without creating unmanaged Cargo targets.
8. Claude, Gemini, DeepSeek, GLM, and Kimi review/adversarial review are unanimous approve or findings are resolved.
9. PR body names issue scope and residuals.
10. No merge without explicit operator approval.
