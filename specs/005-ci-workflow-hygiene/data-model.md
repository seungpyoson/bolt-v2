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

## PrebuiltToolInstallContract

- **install_action_jobs**: `deny`, `advisories`, and `test-shards`.
- **install_action_tool_specs**: `cargo-deny@${{ steps.setup.outputs.deny_version }}` and `cargo-nextest@${{ steps.setup.outputs.nextest_version }}`.
- **install_action_pin**: `taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c`.
- **install_action_fallback**: `none`.
- **manual_archive_job**: `build`.
- **manual_archive_tool**: `cargo-zigbuild`.
- **manual_archive_version_output**: `steps.setup.outputs.zigbuild_version`.
- **manual_archive_sha256_output**: `steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256`.

Validation rules:

- CI and advisory workflows must not source-build guarded tools with `cargo install`, including flag-reordered, toolchain-prefixed, or `crate@version` forms.
- `deny`, `advisories`, and `test-shards` must use the pinned install action with `fallback: none`.
- `build` must download the pinned `cargo-zigbuild` release archive and compare its hash to the in-repo setup output, not to a checksum downloaded from the same release origin.
- The setup action must export the `cargo-zigbuild` archive SHA256 from the justfile under `include-build-values`.

## DetectorSerializationDecision

- **removed_edge**: `fmt-check -> detector`.
- **kept_edges**: `build -> detector`, `source-fence -> detector`, `test -> detector`, `test -> source-fence`.

Validation rules:

- `fmt-check` must not list detector in `needs`.
- `build` still consumes detector output.
- Gate still requires detector success.
