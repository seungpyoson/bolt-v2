# CI Workflow Hygiene Data Model

## WorkflowTopologyContract

- **required_jobs**: exact job ids that must exist in `.github/workflows/ci.yml`.
- **gate_needs**: required lanes listed in `gate.needs`.
- **gate_result_checks**: `needs.<job>.result` expressions that must appear in the gate script.
- **job_needs**: direct needs required for individual jobs.
- **build_required_expression**: detector output expression that must gate the build job.

Validation rules:

- Missing required jobs fail closed.
- Missing gate needs or gate result checks fail closed.
- `build` must remain detector-output-gated.
- `test` must remain after `source-fence`.

## WorkflowHygieneVerifier

- **input_path**: workflow file path.
- **parsed_jobs**: minimal job map extracted from top-level `jobs:`.
- **errors**: actionable messages for missing or invalid invariants.

Validation rules:

- Uses standard-library parsing only.
- Does not require generic YAML completeness; only this repo's workflow contract.
- Handles inline and block `needs` syntax.
- Handles job ids beyond the old awk `[A-Za-z0-9_-]+` shape.

## SetupTargetDirOptIn

- **input_name**: `include-managed-target-dir`.
- **default**: `false`.
- **true_jobs**: jobs that use `steps.setup.outputs.managed_target_dir`.
- **false_jobs**: jobs that do not use `steps.setup.outputs.managed_target_dir`.

Validation rules:

- Jobs using managed target cache must set `include-managed-target-dir: "true"`.
- Jobs not using managed target cache must not set it.
- The setup action must not resolve target dir unless the input is true.

## DeployDefenseNeeds

- **deploy_job**: `deploy`.
- **direct_needs**: `gate`, `build`, `detector`, `fmt-check`, `deny`, `clippy`, `source-fence`, `test`.

Validation rules:

- Deploy cannot be wired only through transitive gate/build edges.
- Deploy keeps gate as an aggregate dependency and build as the artifact dependency.

## DetectorSerializationDecision

- **removed_edge**: `fmt-check -> detector`.
- **kept_edges**: `build -> detector`, `source-fence -> detector`, `test -> detector`, `test -> source-fence`.

Validation rules:

- `fmt-check` must not list detector in `needs`.
- `build` still consumes detector output.
- Gate still requires detector success.
