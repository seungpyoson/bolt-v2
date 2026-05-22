# Feature Specification: Developer-Tool Storage Hygiene

**Feature Branch**: `codex/375-developer-tool-storage-hygiene`
**Created**: 2026-05-23
**Status**: Draft
**Input**: Issue #375: developer-tool storage hygiene for Codex logs/sessions, Factory droid log, and rustup toolchains, with Phase 1 enumeration before implementation.

## User Scenarios & Testing

### User Story 1 - Enumerate Developer-Tool Writers (Priority: P1)

As the operator, I can inspect a source-backed inventory of developer tools that write to disk during bolt-v2 work, including exact paths, growth shape, native retention support, and ownership.

**Why this priority**: #375 explicitly blocks implementation until a Phase 1 enumeration is present and reviewed for gaps and overlaps.

**Independent Test**: Review `specs/024-developer-tool-storage-hygiene/evidence.md` and verify every in-scope and adjacent developer-tool category has path, growth shape, native rotation/retention status, and owner classification.

**Acceptance Scenarios**:

1. **Given** a developer-tool path is listed in #375, **When** the evidence map is reviewed, **Then** it identifies the exact path family, current measurement, growth shape, native policy support, and #375 ownership.
2. **Given** a measured path is adjacent but not #375-owned, **When** the evidence map is reviewed, **Then** it names the owning issue or classifies the path as out of repo or report-only.
3. **Given** a reviewer asks whether NautilusTrader owns this behavior, **When** the NT evidence is inspected, **Then** the answer is source-backed and does not infer NT responsibility for developer-tool storage.

---

### User Story 2 - Define Deterministic Cleanup Policy (Priority: P1)

As the operator, I can see exactly which #375 path families are eligible for cleanup, which are protected, and which are report-only because safe deletion semantics are not proven.

**Why this priority**: Blind deletion can lose debugging context, transcripts, or toolchains; the issue asks for deterministic policy over heuristics.

**Independent Test**: Given a synthetic storage tree, the planned policy classifies Codex log rotation, Codex session TTL, Factory log rotation, rustup toolchain retention, and report-only Codex data surfaces without touching protected or report-only path families.

**Acceptance Scenarios**:

1. **Given** `codex-tui.log` exceeds the configured cap, **When** policy is evaluated, **Then** the file is eligible for deterministic rotation with a bounded retained count.
2. **Given** Codex session JSONL files are older than the configured TTL, **When** policy is evaluated, **Then** they are listed as candidates in dry-run output before any apply behavior.
3. **Given** a rustup toolchain is active, default, or matches the project pin, **When** policy is evaluated, **Then** it is protected under every mode.
4. **Given** Codex SQLite db/WAL files are large, **When** policy is evaluated, **Then** they are measured and reported but not deleted unless a safe native contract is proven.
5. **Given** Codex `history.jsonl` exists, **When** policy is evaluated, **Then** it is measured as a report-only native-config surface and is not selected for deletion by #375 cleanup policy.
6. **Given** Codex archived sessions exist, **When** policy is evaluated, **Then** they are measured as report-only session archives and are not selected for deletion by #375 cleanup policy.

---

### User Story 3 - Preflight Before Heavy Work (Priority: P2)

As the operator, I can run a lightweight preflight that reports #375 storage pressure before expensive local work continues.

**Why this priority**: #375 is symptom-facing; the operator needs warning before developer-tool storage silently returns the machine to disk-pressure conditions.

**Independent Test**: Given synthetic measurements over configured warning/error thresholds, preflight returns a deterministic status that separates cleanup candidates, report-only large surfaces, and out-of-scope machine caches.

**Acceptance Scenarios**:

1. **Given** total #375-owned storage exceeds the warning threshold, **When** preflight runs, **Then** it reports per-family size, owner, and next action without deleting files.
2. **Given** available disk falls below the configured error threshold, **When** preflight runs, **Then** it fails closed before heavy local verification is recommended.
3. **Given** only out-of-repo browser or package-manager caches are large, **When** preflight runs, **Then** it reports them as adjacent context without claiming #375 cleanup ownership.

---

### User Story 4 - Verify Policy Without Touching Real Home Data (Priority: P2)

As a reviewer, I can validate #375 cleanup and preflight behavior against scratch fixtures instead of the operator's real home directory.

**Why this priority**: The repo must prove dry-run/apply safety without deleting local logs, sessions, credentials, or toolchains.

**Independent Test**: Tests build scratch Codex, Factory, and rustup-like directories, run policy classification against those directories, and assert candidate/protected/report-only results.

**Acceptance Scenarios**:

1. **Given** scratch Codex logs, sessions, sqlite files, history, archived sessions, Factory logs, and rustup toolchains, **When** tests run, **Then** only configured cleanup candidates are selected.
2. **Given** scratch rustup includes pinned, active, default, and stale toolchains, **When** tests run, **Then** pinned, active, and default toolchains are protected.
3. **Given** a policy file is malformed or incomplete, **When** tests run, **Then** behavior fails closed with a specific validation error.
4. **Given** a policy validates during dry-run but becomes malformed or incomplete before apply, **When** apply begins, **Then** apply revalidates policy, aborts before mutation, and reports the validation error.
5. **Given** a cleanup candidate changes or disappears after dry-run, **When** apply begins, **Then** apply re-scans immediately before mutation and aborts rather than applying stale candidate data.
6. **Given** a configured active writer process is detected for a mutable Codex or Factory surface, **When** apply begins, **Then** apply refuses before mutation and reports the active-writer reason.

## Edge Cases

- Codex SQLite db/WAL files are large but do not have a documented cleanup contract: measure and report only.
- Codex `history.jsonl` is large but has documented native history settings: report the file and native-config guidance, but do not delete it under #375 cleanup policy.
- Codex archived sessions are present: measure and report them, but do not delete archived transcripts without a separate proven session-archive contract.
- Factory executable is absent but the log path exists: keep the path in the inventory and apply file-policy only if configured explicitly.
- Codex sessions newer than TTL, missing mtimes, unreadable files, or symlinks appear: preserve them unless deterministic policy proves they are safe candidates.
- A mutable Codex or Factory surface has a configured active writer process: refuse apply rather than mutating a live writer.
- A rustup toolchain is both stale and active/default/pinned: protected status wins.
- General machine caches such as npm, Homebrew, Xcode, browser profiles, and IDE caches are large: report adjacency without deleting them under #375.
- Any new operator-facing cleanup command or command semantics are needed: stop and obtain explicit operator approval before implementation.

## Requirements

### Functional Requirements

- **FR-001**: The repo MUST include a Phase 1 developer-tool storage enumeration for #375 with tool category, exact path family, measured current size when locally present, growth shape, native rotation/retention status, and ownership.
- **FR-002**: The enumeration MUST distinguish current behavior, latent risk, and future enablement requirements.
- **FR-003**: The implementation MUST keep #375 separate from #286 managed Rust cache retention, #374 verifier/wrapper cleanup/preflight, #376 runtime/local CI/cargo registry inventory, and out-of-repo machine caches.
- **FR-004**: Cleanup policy MUST be deterministic and config-driven; no cleanup candidate may be selected by substring-only heuristics.
- **FR-005**: Cleanup policy MUST support dry-run output before apply behavior.
- **FR-006**: Cleanup policy MUST protect active, default, and project-pinned rustup toolchains under every mode.
- **FR-007**: Cleanup policy MUST treat Codex SQLite db/WAL files, Codex `history.jsonl`, and Codex archived sessions as report-only unless a safe native cleanup contract is proven.
- **FR-008**: Preflight MUST report per-family sizes, ownership, cleanup eligibility, protected items, report-only items, and out-of-scope adjacent storage.
- **FR-009**: Tests MUST use scratch directories and synthetic toolchain/session/log fixtures instead of mutating the operator's real home directory.
- **FR-010**: The PR MUST NOT change NautilusTrader runtime behavior, Bolt live trading behavior, or #374 verifier/parser architecture unless source evidence proves it is required for #375.
- **FR-011**: The PR MUST NOT add new shell parser cases, wrapper families, command prediction, or raw Cargo command semantics.
- **FR-012**: If satisfying #375 requires a new operator-facing command or changed command semantics, implementation MUST pause until the operator explicitly approves that command surface.
- **FR-013**: The final PR MUST record targeted tests, relevant Rust verification, source-fence/schema/runtime-literal checks if touched, ai-slop cleanup, no-mistakes exact-head result, GitHub exact-head CI, and external review outcomes.
- **FR-014**: Apply behavior, if approved, MUST revalidate policy, re-scan the filesystem immediately before mutation, and refuse mutable Codex and Factory actions when configured active writer processes are detected.
- **FR-015**: Active-writer detection MUST use configured exact process names and process snapshots; it MUST NOT add shell parser cases, wrapper-family semantics, or command prediction.

### Key Entities

- **StorageSurface**: A measured path family with category, exact path, growth shape, owner, native policy, current size, and cleanup eligibility.
- **CleanupPolicy**: Configured limits and retention rules for #375-owned surfaces.
- **ProcessSnapshot**: A read-only list of observed process names used to decide active-writer refusal.
- **ProtectedItem**: A path or toolchain that cleanup must never remove in the current mode.
- **CleanupCandidate**: A deterministic dry-run/apply action selected from scratch or real measurements.
- **ActiveWriterRefusal**: A pre-apply refusal caused by a configured active process match for a mutable Codex or Factory surface.
- **PreflightReport**: A read-only status payload that summarizes disk pressure and recommended next action.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A reviewer can trace every #375-owned path family from issue evidence to repo evidence and policy ownership.
- **SC-002**: Synthetic dry-run tests list stale Codex sessions, oversized Codex/Factory logs, and stale unprotected rustup toolchains without modifying scratch files.
- **SC-003**: Synthetic apply tests modify only configured cleanup candidates, preserve protected/report-only paths, re-scan before mutation, and refuse configured active writer processes.
- **SC-004**: Preflight tests fail closed when configured disk or #375-owned storage thresholds are breached.
- **SC-005**: The PR changes exactly the #375 artifact/code surface and does not implement #454 or broader verifier decomposition work.

## Assumptions

- The target operator environment is macOS, matching the measured paths and issue body.
- The repo may provide policy, verification, and installable native configuration artifacts, but should not mutate the operator's real home directory during tests.
- OpenAI Codex config support for `history.max_bytes`, `history.persistence`, and `tui.log_dir` covers history storage guidance and log directory placement, but it does not by itself provide `codex-tui.log` rotation or `sessions/**/*.jsonl` TTL.
- The active project Rust pin is `1.95.0` until `rust-toolchain.toml` changes.
