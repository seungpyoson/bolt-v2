# Trustworthy Delivery Process

## Purpose

Use this process to turn a concrete ask such as "ship this issue" into a deliverable that is trustworthy by construction. The goal is not optimism. The goal is to make wrong actions impossible, and to make ambiguous states fail closed.

## Roles

- `Planner`: defines scope, state space, invariants, alternatives, and execution contract.
- `Implementer`: changes code and tests, but only against the approved contract.
- `Adversaries`: search for ways the plan or code can produce a wrong action.
- `Verifier`: independently checks claims, tests, and review findings before any completion claim or PR.

One role may play multiple parts across time, but the checks must remain independent. The same reasoning pass that proposed a change must not be the only pass that approves it.

## Non-Negotiable Rules

1. Work in an isolated copy whenever the task touches production behavior.
2. Do not start production edits until the state space and invariants are written down.
3. Every finding must end in exactly one terminal state: `FIXED` or `DISPROVEN`.
4. If a required fact is not derivable from local code, configs, tests, or authoritative task text, emit an `EXTERNAL-INPUT` block and pause.
5. Every success claim requires fresh evidence from the exact verification command.
6. Wrong actions are forbidden. If certainty is missing at runtime, halt or reject under a fail-closed default.

## Phase 1: Universe Enumeration

Create a state map for the exact seam being changed.

For each reachable state, record:

- entry condition
- input source
- intended outcome
- safe failure mode
- code location expected to own the decision

If a state cannot be assigned to an owning mechanism, the task is not ready for implementation.

## Phase 2: Invariant Ledger

Translate the state map into invariants that must always hold.

Each invariant must name:

- the canonical data representation
- the allow/deny rule
- the fail-closed behavior when the rule cannot be proven
- the verification artifact that will prove it later

## Phase 3: Design Alternatives

Produce at least two viable designs and one recommendation.

For each alternative, state:

- boundary of change
- new failure modes introduced
- why it preserves or weakens trustworthiness
- migration and testing cost

Reject any alternative that keeps ambiguous states or relies on raw strings when structure is available.

## Phase 4: Adversarial Review

Adversaries try to break the recommended design before code exists.

Required attack classes:

- malformed input
- ambiguous metadata
- stale assumptions or hidden literals
- casing or formatting drift
- unsupported families or assets
- bypass of validation gates

Every attack becomes a finding with an owner and a proof obligation. Findings remain open until later marked `FIXED` or `DISPROVEN`.

## Phase 5: Implementation Contract

Freeze the exact scope before code:

- files allowed to change
- interfaces allowed to change
- tests that must be added first
- runtime halts or rejections that are required
- behaviors explicitly out of scope

The Implementer may not widen scope without returning to the Planner.

## Phase 6: Walkthrough Simulation

Run the design on representative scenarios before writing production code.

Include:

- happy path
- required mismatch rejection
- malformed-input halt or rejection
- at least one new-family case that should succeed without code literals

If the walkthrough cannot explain who decides, when, and how the system fails closed, the contract is incomplete.

## Phase 7: Red-Green-Refactor Execution

1. Write one failing test for one contract rule.
2. Run it and confirm the failure is the expected one.
3. Write the minimum production change to make that test pass.
4. Run the focused tests again.
5. Refactor only while the tests stay green.
6. Repeat until every contract rule is covered.

No production code is allowed before the corresponding failing test exists.

## Phase 8: Integrated Verification

Before any completion claim or PR:

- run the focused tests for the changed seam
- run broader regression tests or build commands required by the repo
- re-check the invariant ledger against the actual code
- update the state map with the final owning mechanism for each state

The final verification artifact must name exact mechanisms in the format:

- `file`
- `line`
- `condition`
- `timing or execution point`

If a state still lacks an owning mechanism, the deliverable is not trustworthy.

## Phase 9: PR and Automated Review Loop

Open the PR only after verification evidence exists. The PR body must include:

- scope statement
- state-map summary
- verification commands and results
- explicit remaining ask for external review

Treat automated review comments as findings. Each one must be resolved to `FIXED` or `DISPROVEN`, with evidence.

## Phase 10: External Adversarial Review Gate

Ask for at least one reviewer outside the Planner/Implementer family. Provide:

- issue statement
- changed files
- invariants
- known risky seams
- verification commands
- exact questions to attack

Do not merge until that review is complete or the human owner waives it explicitly.

## Required Artifacts

Every execution of this process should leave behind:

- issue snapshot
- universe enumeration
- invariant ledger
- design alternatives
- adversarial findings log
- implementation contract
- walkthrough simulation
- code and tests
- PR text
- review-resolution log
- session-state handoff

## `EXTERNAL-INPUT` Template

```text
EXTERNAL-INPUT <ID>
Need:
Why local evidence is insufficient:
Blocking decision:
Safe paused state:
```

## Finding Template

```text
Finding:
Attack:
Expected failure mode:
Owner:
Evidence required:
Terminal status: FIXED | DISPROVEN
```
