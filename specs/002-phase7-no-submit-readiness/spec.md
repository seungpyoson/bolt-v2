# Feature Specification: Phase 7 No-submit Readiness

**Feature Branch**: `017-bolt-v3-phase7-no-submit-readiness-fresh`
**Created**: 2026-05-14
**Status**: Draft
**Input**: User description: "Phase 7: no-submit live readiness evidence path from current main. Use real SSM-resolved credentials only, connect/read required venue and reference readiness, place zero orders, produce redacted readiness report, fail closed on missing or invalid SSM, venue auth, geo block, wrong market or instrument, stale data, or missing Chainlink/reference readiness. No environment fallback, no AWS CLI subprocess, no hidden dual readiness path, no live submit. PRs #318/#319/#320 are reference-only forensic input."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Produce Local No-submit Readiness Evidence (Priority: P1)

As the bolt-v3 operator, I can run a local no-submit readiness path against controlled test doubles and receive a redacted report whose shape is accepted by the existing live-canary gate, without any submit, cancel, replace, amend, subscribe, or runner-loop behavior.

**Why this priority**: This is the minimum safe artifact contract before any real SSM or venue connectivity is attempted. It proves Phase 7 stays a thin readiness producer and does not create a second trading path.

**Independent Test**: A local test can build the production-shaped bolt-v3 runtime with fake secret resolution and mock NT clients, run only controlled connect and disconnect, write a redacted report, and feed that report to the live-canary gate.

**Acceptance Scenarios**:

1. **Given** a valid bolt-v3 root TOML fixture with `[live_canary]` report path and fake SSM resolution, **When** local no-submit readiness runs, **Then** the report contains satisfied controlled-connect and controlled-disconnect stages and the live-canary gate accepts it.
2. **Given** no-submit readiness source code, **When** source fences inspect the module and ignored operator harness, **Then** the inspected source contains no submit, cancel, replace, amend, subscribe, or runner-loop calls.
3. **Given** a connect failure from a mock NT client, **When** local no-submit readiness runs, **Then** the report fails closed, records the failed connect stage, runs disconnect cleanup when applicable, and still writes only redacted details.

---

### User Story 2 - Gate Real No-submit Readiness Behind Explicit Operator Approval (Priority: P2)

As the operator, I can run an ignored real-readiness harness only after explicit approval, using the same SSM-only secret boundary and existing bolt-v3 build path, and receive a redacted report without printing or committing secret values.

**Why this priority**: Real credentials and venues are required for live-readiness evidence, but they are side-effect-bearing enough to require explicit operator control and a default-off test surface.

**Independent Test**: The default test suite proves the real harness is ignored, requires an approval id matching config, rejects missing approval before secret resolution, and writes only to the configured report path when explicitly run.

**Acceptance Scenarios**:

1. **Given** the default local test command, **When** the operator harness is present, **Then** it is ignored and performs no real SSM or venue calls.
2. **Given** a missing or whitespace approval id, **When** real no-submit readiness is requested, **Then** the request fails before building the runtime or resolving secrets.
3. **Given** approval id mismatch with `[live_canary].approval_id`, **When** real no-submit readiness is requested, **Then** the request fails before building the runtime or resolving secrets.
4. **Given** explicit approval, exact head, root TOML checksum, and real SSM/venue configuration, **When** the ignored harness is run, **Then** it uses SSM through Rust AWS SDK only, performs controlled connect/readiness and disconnect, writes a redacted report, and performs zero order placement.

---

### User Story 3 - Preserve Phase 8 Safety Boundary (Priority: P3)

As the operator, I can use the Phase 7 report as a prerequisite for Phase 8 planning, while the system still blocks tiny-capital live action until strategy-input safety, no-submit evidence, exact approval, and live-canary gate checks are complete.

**Why this priority**: Phase 7 must unblock evidence gathering without silently authorizing Phase 8 live capital.

**Independent Test**: Plan and tasks show Phase 8 remains out of scope and blocked unless a real approved no-submit report exists and a separate strategy-input safety audit approves live action.

**Acceptance Scenarios**:

1. **Given** only local Phase 7 tests have run, **When** Phase 8 readiness is evaluated, **Then** the outcome is "not ready for live order" because no approved real no-submit report exists.
2. **Given** a redacted no-submit report exists, **When** Phase 8 planning begins, **Then** it must still require strategy-input safety review for `eth_chainlink_taker`, Chainlink feed path, exchange references, market selection, volatility, kurtosis, theta, fee and slippage model, caps, and edge economics.

### Edge Cases

- Missing `[live_canary]` block or report path.
- Report path parent directory does not exist.
- Oversized report or malformed report JSON.
- Any readiness stage missing, skipped, failed, or stale.
- Secret resolver setup failure before any venue client is built.
- SSM path exists but value cannot be resolved or parsed.
- Venue authentication failure, geo block, wrong market, wrong instrument, stale Chainlink or exchange reference data, or missing required reference venue.
- Disconnect fails after connect fails.
- Redacted report or debug output contains a resolved credential value.
- `config/live.local.toml` is legacy render input and not a bolt-v3 root TOML.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST add one Phase 7 no-submit readiness path that reuses the existing bolt-v3 config, secret resolution, live-node build, client registration, and controlled connect/disconnect boundaries.
- **FR-002**: System MUST keep no-submit readiness separate from startup/build readiness; startup readiness remains build-only and must not be relabeled as authenticated no-submit readiness.
- **FR-003**: System MUST produce a redacted no-submit readiness report containing explicit stages, statuses, and operator-safe details.
- **FR-004**: System MUST make the report schema compatible with the existing live-canary gate report validator.
- **FR-005**: System MUST fail closed unless every required no-submit readiness stage is satisfied.
- **FR-006**: System MUST perform zero order placement and contain no submit, cancel, replace, amend, runner-loop, or subscribe call in the no-submit readiness module or operator harness.
- **FR-007**: System MUST resolve credentials only through `SsmResolverSession` and the Rust AWS SDK SSM path already used by bolt-v3.
- **FR-008**: System MUST reject missing, whitespace, or mismatched operator approval id before secret resolution, client construction, or venue connection.
- **FR-009**: System MUST keep real SSM and venue readiness in an ignored operator harness that never runs in the default test suite.
- **FR-010**: System MUST write readiness output only to the configured `[live_canary].no_submit_readiness_report_path`.
- **FR-011**: System MUST redact resolved secret values and avoid printing raw SSM values, API keys, private keys, passphrases, or bearer-like token material.
- **FR-012**: System MUST preserve NautilusTrader ownership of adapter behavior, connection dispatch, cache, lifecycle, order state, reconciliation, and venue wire behavior.
- **FR-013**: System MUST NOT expose raw mutable `LiveNode` access as a general public escape hatch to satisfy Phase 7.
- **FR-014**: System MUST classify PR #318/#319/#320 content as reference-only and must not port stale `BoltV3BuiltLiveNode` or `node_mut` design into current main.
- **FR-015**: System MUST keep Phase 8 live order and soak execution out of scope unless user explicitly approves exact head and command in a later runtime turn.
- **FR-016**: System MUST record exact command, head SHA, config checksum, report path, and result for any explicitly approved real no-submit operator run without exposing secrets.

### Key Entities

- **NoSubmitReadinessReport**: Redacted report consumed by the live-canary gate. Contains stage records, approval/config identity, and operator-safe failure details.
- **NoSubmitReadinessStage**: One readiness observation such as approval validation, secret resolution, live-node build, controlled connect, reference readiness, controlled disconnect, and report write.
- **NoSubmitReadinessStatus**: Stage status. Only satisfied stages may pass the live-canary gate; failed or skipped stages fail closed.
- **OperatorApproval**: Explicit approval id supplied by operator and matched against config before side-effect-bearing readiness.
- **ReadinessRunEvidence**: Non-secret record of exact head SHA, root TOML checksum, report path, command, and exit status for approved real no-submit runs.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Local Phase 7 tests prove no-submit report schema compatibility with the live-canary gate.
- **SC-002**: Source-fence tests prove no-submit readiness code contains zero submit, cancel, replace, amend, subscribe, or runner-loop calls.
- **SC-003**: Local tests prove missing and mismatched operator approval fail before secret resolution.
- **SC-004**: Local tests prove resolved secret values are absent from report debug and serialized JSON output.
- **SC-005**: Default test run reports the real operator harness as ignored and performs no real SSM or venue calls.
- **SC-006**: An explicitly approved real operator run can produce a redacted report from real SSM and venue controlled connect/disconnect that the live-canary gate accepts.
- **SC-007**: Phase 8 remains blocked unless SC-006 evidence exists and the separate strategy-input safety audit approves live action.

## Assumptions

- Current source of truth is `main == origin/main == d6f55774c32b71a242dcf78b8292a7f9e537afab`.
- PRs #318, #319, and #320 are closed stale/superseded and may be read only as forensic input.
- Phase 6 submit admission on main is accepted and must be preserved.
- `config/live.local.toml` is legacy render input; approved real no-submit readiness must use a bolt-v3 root TOML with `[live_canary]`.
- Phase 7 implementation may add a narrow current-main-safe internal runner boundary, but must not reintroduce stale stacked-branch wrappers.
- Real no-submit readiness is not live capital and still requires explicit operator approval because it touches real SSM and venue connectivity.
