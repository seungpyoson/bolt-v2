# Data Model: Decompose Disk-Governance Verifiers

## Characterization Case

Represents one current-behavior assertion used to prevent parser drift.

| Field | Meaning | Validation |
|---|---|---|
| `name` | Stable case identifier | Unique within the characterization file |
| `input` | Command string, token list, or workflow snippet | Must avoid secrets and machine-local paths unless synthetic |
| `surface` | Runtime verifier, static verifier, or shared parser | Must map to an in-scope verifier surface |
| `expected` | Current accepted classification | Must be derived from current main before extraction |
| `reason` | Why the case matters | Must cite the helper family or risk it protects |

## Shared Command Understanding Path

Represents the shared parser/scanner module used by both verifier clients.

| Field | Meaning | Validation |
|---|---|---|
| `module_path` | Proposed source path | `scripts/command_understanding.py` |
| `helper_family` | Extracted behavior group | Existing behavior only; no new semantics |
| `clients` | Verifier scripts importing the helper | Must include both relevant verifier surfaces when a duplicated family is extracted |
| `compatibility_guard` | Tests proving behavior preservation | Must include characterization/parity tests and existing suite coverage |

## Verifier Surface

Represents a current script or test file affected by #454.

| Field | Meaning | Validation |
|---|---|---|
| `path` | Repository path | Must be in `scripts/` for this issue |
| `role` | Runtime enforcement, static workflow hygiene, or test coverage | Must be one of the issue-defined roles |
| `line_count_baseline` | Current line count at branch start | Recorded in `evidence.md` |
| `change_kind` | Import shared helper, remove duplicate helper, add characterization, or no change | Must remain mechanical and reviewable |

## Evidence Map Entry

Represents a source-backed finding or verification result.

| Field | Meaning | Validation |
|---|---|---|
| `category` | Current behavior, latent risk, verification, review, or remaining risk | Must separate facts from recommendations |
| `source` | Command, file path, line, issue, PR, or commit | Must be exact enough to reproduce |
| `finding` | What was learned | Must not overclaim readiness |
| `status` | Planned, verified, deferred, or blocked | Deferred items must name why they are out of scope |
