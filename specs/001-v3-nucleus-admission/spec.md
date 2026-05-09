# Feature Specification: Bolt-v3 Nucleus Admission Audit

**Feature Branch**: `001-v3-nucleus-admission`
**Created**: 2026-05-09
**Status**: Draft
**Input**: User description: "Bolt-v3 nucleus admission audit: report-only verifier proving generic NT-first contract readiness before any concrete provider, market-family, strategy, or live behavior continues."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - See Current Nucleus Blockers (Priority: P1)

A maintainer can run one command and see whether Bolt-v3 is admitted to continue
behavior work, with every blocker tied to concrete evidence and an invariant.

**Why this priority**: This is the immediate guardrail against repeating the
known failure mode: concrete fixture or provider work becoming architecture.

**Independent Test**: Run the audit on current `main`. It exits successfully in
default mode and reports all current admission blockers without requiring any
provider credentials, live venue access, AWS access, or network access.

**Acceptance Scenarios**:

1. **Given** current Bolt-v3 source has generic boundaries carrying concrete
   market-family concepts, **When** the maintainer runs the audit in default
   mode, **Then** the report lists each blocker with evidence and a retirement
   path.
2. **Given** current Bolt-v3 source lacks a decision-event contract,
   conformance harness, or BacktestEngine/live parity boundary, **When** the
   maintainer runs the audit in default mode, **Then** these absences are
   reported as admission blockers.
3. **Given** current fixtures include concrete provider, family, strategy,
   market, symbol, or feed values, **When** the maintainer runs the audit,
   **Then** the report distinguishes fenced fixture evidence from forbidden
   generic-core leakage.

---

### User Story 2 - Fail Strictly When Admission Is Blocked (Priority: P2)

A maintainer or CI job can run the same audit in strict mode and receive a
nonzero exit status whenever admission blockers remain.

**Why this priority**: The report-only audit is useful immediately, but the same
logic must become a required gate after the existing blockers are retired.

**Independent Test**: Run the audit in strict mode on current `main`. It exits
nonzero and prints the same blocker inventory as default mode.

**Acceptance Scenarios**:

1. **Given** at least one admission blocker exists, **When** strict mode is
   used, **Then** the audit exits nonzero.
2. **Given** no admission blockers exist, **When** strict mode is used, **Then**
   the audit exits successfully.
3. **Given** an existing narrower verifier allowlist accepts a leak, **When**
   strict mode is used, **Then** the admission audit still fails if the leak
   violates the nucleus contract.

---

### User Story 3 - Prove The Audit Cannot Be Fooled By A Narrow Scan (Priority: P3)

A reviewer can inspect the audit tests and know the verifier covers its intended
scan universe, positive failing fixtures, and waiver format.

**Why this priority**: Prior verifier confidence was too high when the scan
universe and bypass cases were not proven.

**Independent Test**: Run the audit self-tests. They include passing fixtures,
failing fixtures, scan-universe assertions, and waiver validation.

**Acceptance Scenarios**:

1. **Given** a fixture with a concrete provider or market-family name in generic
   core, **When** self-tests run, **Then** the audit catches it.
2. **Given** a fixture with the same concrete name inside an allowed fixture or
   provider-owned binding, **When** self-tests run, **Then** the audit permits
   it only when the allowed context is explicit.
3. **Given** a waiver without path, excerpt, blocker id, or retirement issue,
   **When** self-tests run, **Then** the waiver is rejected.

### Edge Cases

- If Bolt-v3 modules are moved or renamed, the audit must report a scan-universe
  failure instead of silently passing.
- If no Bolt-v3 source exists, the audit must report that the nucleus cannot be
  admitted because the expected contract surface is absent.
- If non-UTF-8 files exist in the repository, the audit must skip them with a
  reported reason rather than crashing or hiding the scan count.
- If a concrete name appears only in documentation or historical evidence, the
  audit must not classify it as runtime-core leakage.
- If a report-only run finds blockers, it must still exit successfully so the
  initial PR can merge without weakening required CI.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The audit MUST report whether Bolt-v3 is admitted, blocked, or
  unscannable.
- **FR-002**: The audit MUST classify blockers by invariant, including generic
  contract leakage, missing contract surfaces, unowned runtime defaults,
  unfenced concrete fixtures, narrow verifier bypasses, and missing parity
  gates.
- **FR-003**: The audit MUST include evidence for every blocker: path, excerpt
  or absence proof, blocker id, severity, and required retirement condition.
- **FR-004**: Default mode MUST exit successfully while still reporting
  blockers.
- **FR-005**: Strict mode MUST exit nonzero when blockers are present or when
  the scan universe cannot be proven.
- **FR-006**: The audit MUST prove that existing narrow verifier allowlists do
  not suppress nucleus admission blockers.
- **FR-007**: The audit MUST require every waiver to include path, excerpt,
  blocker id, rationale, and retirement issue.
- **FR-008**: The audit MUST require an explicit policy boundary between
  concrete values allowed in fixtures/catalog/provider-owned bindings and
  concrete values forbidden in generic core.
- **FR-009**: The feature MUST add self-tests covering passing fixtures,
  failing fixtures, scan-universe failures, strict-mode status, default-mode
  status, and invalid waivers.
- **FR-010**: The feature MUST NOT add or change live trading behavior,
  provider behavior, market-family behavior, strategy behavior, production
  secret handling, deployment behavior, or required CI gates.
- **FR-011**: The feature MUST expose a repository command that runs the audit
  in report-only mode.
- **FR-012**: The feature MUST document the follow-up condition for promoting
  strict mode into required CI after blockers are retired.

### Key Entities *(include if feature involves data)*

- **Admission Audit Run**: A single execution, including mode, scan universe,
  files scanned, blockers found, warnings, and exit status.
- **Nucleus Invariant**: A non-negotiable rule from the constitution that the
  Bolt-v3 nucleus must satisfy before behavior work continues.
- **Admission Blocker**: A specific violation or absence that prevents nucleus
  admission until retired.
- **Evidence Record**: The path, excerpt or absence proof, and explanation that
  supports a blocker.
- **Waiver**: An explicit, temporary exception with blocker id, path, excerpt,
  rationale, and retirement issue.
- **Promotion Condition**: The required state before the report-only audit can
  become a strict required CI gate.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On current `main`, default mode reports the known admission
  blockers and exits successfully.
- **SC-002**: On current `main`, strict mode reports the same blockers and exits
  nonzero.
- **SC-003**: Self-tests include at least one positive failing fixture for each
  blocker class implemented in this feature.
- **SC-004**: Self-tests include at least one allowed-context fixture proving
  concrete names remain permitted in explicitly fenced fixture or binding
  contexts.
- **SC-005**: The audit output names the scan universe and file count so a
  reviewer can see what was and was not inspected.
- **SC-006**: The feature introduces no production runtime behavior changes.

## Assumptions

- This feature is the first forward milestone for Bolt-v3 recovery.
- This feature creates a report-only admission audit, not the final strict CI
  gate.
- Existing Bolt-v3 code and fixtures are evidence for the audit, not a foundation
  to extend with provider-specific behavior.
- Promotion to required CI is a separate follow-up after the admission blockers
  are retired.
- No provider credentials, AWS access, live venue access, or external network
  calls are needed for this feature.
