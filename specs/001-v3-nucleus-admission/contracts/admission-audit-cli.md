# Contract: Admission Audit CLI

## Command

```bash
python3 scripts/verify_bolt_v3_nucleus_admission.py
```

Repository recipe:

```bash
just verify-bolt-v3-nucleus-admission
```

## Options

- `--strict`: exit nonzero when blockers or scan-universe failures are present.
- `--repo-root <path>`: optional repository root override for self-tests and
  fixtures. Defaults to the detected repository root.

No option may require credentials, AWS access, network access, live venue
access, or production runtime configuration.

## Exit Status

- Default mode:
  - `0`: audit ran and printed admitted or blocked status.
  - nonzero: audit crashed or could not execute.
- Strict mode:
  - `0`: audit ran, scan universe was proven, and no blockers were found.
  - `1`: blockers were found or scan universe could not be proven.
  - `2`: command-line usage error.

## Output Requirements

The command prints a deterministic text report containing:

- audit mode;
- admission verdict;
- scan universe summary and file count;
- blockers grouped by blocker class;
- evidence records with path, line when available, short excerpt or absence
  proof, and retirement condition;
- warning records, if any.

The output must not print secrets or full credential values.

## Non-Goals

- No live order execution.
- No provider client construction.
- No AWS or SSM reads.
- No mutation of repository files.
- No required CI wiring in this feature.
