# Data Model: Bolt-v3 Nucleus Admission Audit

## AdmissionAuditRun

Represents one execution of the audit.

Fields:

- `mode`: `report` or `strict`.
- `repo_root`: repository root inspected.
- `scan_universe`: collection of scanned path groups.
- `files_scanned`: count of UTF-8 files inspected.
- `files_skipped`: files skipped with reasons.
- `blockers`: ordered list of `AdmissionBlocker`.
- `warnings`: non-blocking observations.
- `exit_status`: expected process status for the selected mode.

Validation rules:

- `mode` must be explicit.
- `scan_universe` must include source, tests, fixtures, scripts, and V3 docs
  where present.
- Strict mode exits nonzero if any blocker exists or if scan universe proof
  fails.

## ScanUniverse

Represents the files the audit claims to inspect.

Fields:

- `roots`: configured root paths.
- `include_patterns`: file or path patterns included.
- `exclude_patterns`: generated/vendor/cache paths excluded.
- `matched_files`: concrete file list after filtering.
- `utf8_file_count`: files decoded and inspected.

Validation rules:

- Missing expected V3 roots must produce a blocker or warning, not a silent pass.
- Non-UTF-8 skips must be reported.
- Generated directories such as target caches and Python bytecode caches must not
  define admission state.

## NucleusInvariant

Represents a constitution-backed rule.

Fields:

- `id`: stable kebab-case identifier.
- `title`: human-readable name.
- `source`: constitution section or repo rule.
- `admission_requirement`: exact condition required for admission.

Validation rules:

- Every blocker maps to one invariant.
- Invariants must be stable enough for review comments and future issues.

## AdmissionBlocker

Represents one reason Bolt-v3 is not admitted.

Fields:

- `id`: stable blocker id.
- `class`: blocker class.
- `severity`: `blocker` or `warning`.
- `invariant_id`: linked `NucleusInvariant`.
- `evidence`: one or more `EvidenceRecord`.
- `retirement_condition`: concrete condition required to remove the blocker.

Validation rules:

- Blockers must be deterministic in order and content.
- Each blocker must include at least one evidence record.
- Absence blockers must explain what was searched and where.

## EvidenceRecord

Represents the proof behind a blocker.

Fields:

- `path`: repository-relative path.
- `line`: line number when available.
- `excerpt`: short matching text when available.
- `absence_query`: search pattern and searched roots for absence proof.
- `explanation`: why the evidence matters.

Validation rules:

- Either `excerpt` or `absence_query` must be present.
- Excerpts must be short and safe to print.
- Secret-looking values must never be printed.

## Waiver

Represents an explicit temporary exception.

Fields:

- `blocker_id`: blocker being waived.
- `path`: affected path.
- `excerpt`: exact short evidence being waived.
- `rationale`: why the exception is safe.
- `retirement_issue`: issue number or URL.

Validation rules:

- All fields are required.
- Waivers cannot apply to live order behavior, credentials, or production secret
  paths.
- Invalid waivers are blockers in strict mode.

## State Transitions

```text
not_scanned -> scanned_with_blockers -> report_only_known_blocked
not_scanned -> scan_universe_unproven -> blocked
scanned_with_blockers -> blockers_retired -> admitted
admitted -> strict_gate_promoted -> required_ci_gate
```

The current feature stops at `report_only_known_blocked`; promotion to required
CI is a separate follow-up.
