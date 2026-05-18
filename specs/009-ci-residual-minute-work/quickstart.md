# Quickstart: #344 Residual Minute-Consumption Work

Run from the feature worktree.

```bash
python3 -B scripts/test_verify_ci_path_filters.py
python3 -B scripts/verify_ci_path_filters.py
python3 -B scripts/test_verify_ci_workflow_hygiene.py
python3 -B scripts/verify_ci_workflow_hygiene.py
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml"); YAML.load_file(".github/workflows/ci-docs-pass-stub.yml"); puts "OK"'
just ci-lint-workflow
git diff --check
```

Inspect required #344 docs:

```bash
rg -n "AGENTS.md|workflow|src/|rust-verification|Cargo.lock|mixed|pass-stub" docs/ci/paths-ignore-behavior.md
rg -n "active|reference-only|dead-merged-prunable" docs/ci/branch-hygiene-2026-05-15.md
```

Blocked evidence still needed after stack lands:

- docs-only throwaway PR run evidence
- post-#332/#195/#205 monthly Actions minute rebaseline
