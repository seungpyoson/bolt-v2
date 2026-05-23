# Research: Developer-Tool Storage Hygiene

## Decision: Keep #375 as Developer-Tool Storage, Not Rust Verification

**Rationale**: Issue #375 names Codex logs/sessions, Factory droid log, and rustup toolchains. `specs/014-disk-pressure-governance/spec.md:83-87` separates #374 cargo invocation hardening, #375 developer-tool enumeration, and #376 runtime/local CI/cargo registry inventory. PR #436 states #375 remains separate.

**Alternatives considered**:
- Extend #374 verifier/parser logic: rejected because #375 does not require shell/wrapper parsing and the user explicitly forbids spur-of-the-moment verifier/parser architecture work.
- Fold cargo registry/git into #375: rejected because #376 owns those surfaces.

## Decision: Treat Codex SQLite Files As Report-Only Initially

**Rationale**: Local measurement shows `~/.codex/logs_2.sqlite*` exceeds 4 GiB, but OpenAI Codex config reference documents `history.max_bytes`, `history.persistence`, and `log_dir`, not SQLite cleanup semantics. Deleting or rotating sqlite db/WAL files without a tool contract risks corrupting tool state.

**Alternatives considered**:
- Delete sqlite WAL or db files by size: rejected as unsafe and heuristic.
- Ignore sqlite files because the issue body did not name them: rejected because the Phase 1 comment explicitly asks for additional Codex paths.

## Decision: Use Config-Driven Policy And Scratch Fixtures

**Rationale**: Repo rules require runtime values from TOML, and #375 cleanup must be deterministic. Tests can create scratch Codex/Factory/rustup trees and verify candidate/protected/report-only classification without touching real home data.

**Alternatives considered**:
- Hard-code user home paths or retention values in tests/code: rejected by repo rules and portability.
- Test against the operator's actual `~/.codex` and `~/.rustup`: rejected because verification must not delete or expose real data.

## Decision: Require Dry-Run Before Apply

**Rationale**: Existing disk-pressure governance contract requires status/dry-run before apply and never removing pinned or active Rust toolchains. #375 also carries transcript and log-loss risks.

**Alternatives considered**:
- Apply-only cleanup: rejected because it is too destructive.
- Docs-only instructions: rejected unless operator denies command approval, because #375 asks for bounded behavior, not just an inventory.

## Decision: Protect Active, Default, Project-Pinned, And Exact-Retained Toolchains

**Rationale**: The repository-root `rust-toolchain.toml` pins `1.95.0`, `Cargo.toml` requires Rust `1.95.0`, and local `rustup toolchain list` reports `1.95.0-aarch64-apple-darwin` active and `1.94.1-aarch64-apple-darwin` default. Removing active/default/pinned toolchains can break local work. Rustup age and directory mtime are not reliable cleanup predicates, so removal eligibility must come only from exact configured installed toolchain names after protection is applied.

**Alternatives considered**:
- Keep only the project pin: rejected because default/active may support adjacent current work.
- Remove by stale age or directory mtime: rejected because it is heuristic and can remove a toolchain that is still operationally relevant.
- Remove stable by hardcoded name: rejected because retention/removal must be TOML policy-driven exact-name configuration, not code hardcoding.

## Decision: Pause Before New Operator-Facing Command Semantics

**Rationale**: The user explicitly requires no new command semantics unless approved. A cleanup helper with status/dry-run/apply may be necessary to satisfy #375, but implementation must not start that command surface without explicit operator approval.

**Alternatives considered**:
- Add a new command immediately because #375 implies cleanup: rejected by the user's global rule.
- Avoid any code by shipping only docs: retained only as fallback if operator approval is denied.
