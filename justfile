set shell := ["bash", "-euo", "pipefail", "-c"]

# bolt-v2 build commands — single source of truth.
# CI and local both call these recipes. No raw cargo build/check commands in workflow YAML.

nextest_version := "0.9.132"
deny_version := "0.19.0"
zigbuild_version := "0.22.1"
zig_version := "0.15.2"

target := "aarch64-unknown-linux-gnu"
worktree_root := env_var('HOME') + "/worktrees/bolt-v2"
# Tracked live profile selected by the operator. This is an opaque profile ID;
# there is no venue/market/strategy default.
live_profile := env_var_or_default('BOLT_LIVE_PROFILE', '')
# Generated, gitignored runtime config the binary actually runs.
live_runtime := "config/live.toml"
repo_root := justfile_directory()
rust_verification_owner := repo_root + "/scripts/rust_verification.py"

[private]
check-workspace:
    #!/usr/bin/env bash
    project_root="$(git rev-parse --show-toplevel 2>/dev/null || printf '%s\n' '{{justfile_directory()}}')"
    dir="$(dirname "$project_root")"

    while true; do
        candidate="$dir/Cargo.toml"
        if [ -f "$candidate" ] && grep -q '^\[workspace\]' "$candidate"; then
            echo "ERROR: Foreign Cargo workspace detected at $candidate"
            echo "This checkout sits under an unrelated Cargo workspace."
            echo "Fix: recreate with 'just worktree <branch-name>' under {{worktree_root}}"
            exit 1
        fi

        if [ "$dir" = "/" ]; then
            break
        fi

        parent="$(dirname "$dir")"
        if [ "$parent" = "$dir" ]; then
            break
        fi
        dir="$parent"
    done

[private]
require-rust-verification-owner:
    python3 "{{rust_verification_owner}}" validate-policy --repo "{{repo_root}}" >/dev/null

[private]
require-local-verification-gate:
    #!/usr/bin/env bash
    if [ "${BOLT_LOCAL_VERIFICATION_GATE:-}" != "1" ]; then
        echo "ERROR: run the public local verification recipe so scripts/local_verification_gate.py owns the lane"
        exit 2
    fi

[private]
require-live-profile:
    #!/usr/bin/env bash
    if [ -z "${BOLT_LIVE_PROFILE:-}" ]; then
        echo "ERROR: set BOLT_LIVE_PROFILE to an opaque profile ID"
        exit 2
    fi

verify-bolt-v3-runtime-literals: check-workspace
    python3 scripts/test_verify_bolt_v3_runtime_literals.py
    python3 scripts/verify_bolt_v3_runtime_literals.py

verify-bolt-v3-provider-leaks: check-workspace
    python3 scripts/test_verify_bolt_v3_provider_leaks.py
    python3 scripts/verify_bolt_v3_provider_leaks.py

verify-bolt-v3-status-map-current: check-workspace
    python3 scripts/test_verify_bolt_v3_status_map_current.py
    python3 scripts/verify_bolt_v3_status_map_current.py

verify-bolt-v3-schema-current: check-workspace
    python3 scripts/test_verify_bolt_v3_schema_current.py
    python3 scripts/verify_bolt_v3_schema_current.py

verify-bolt-v3-core-boundary: check-workspace
    python3 scripts/test_verify_bolt_v3_core_boundary.py
    python3 scripts/verify_bolt_v3_core_boundary.py

verify-bolt-v3-naming: check-workspace
    python3 scripts/test_verify_bolt_v3_naming.py
    python3 scripts/verify_bolt_v3_naming.py

verify-bolt-v3-pure-rust-runtime: check-workspace
    python3 scripts/test_verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/verify_bolt_v3_pure_rust_runtime.py

verify-ra-single-engine-import-boundary: check-workspace
    python3 scripts/test_verify_ra_single_engine_import_boundary.py
    python3 scripts/verify_ra_single_engine_import_boundary.py

verify-ra-notebook-read-only-boundary: check-workspace
    python3 scripts/test_verify_ra_notebook_read_only_boundary.py
    python3 scripts/verify_ra_notebook_read_only_boundary.py

verify-ra-point-in-time-leakage: check-workspace
    python3 scripts/test_verify_ra_point_in_time_leakage.py
    python3 scripts/verify_ra_point_in_time_leakage.py

verify-ra-thin-reader-helper: check-workspace
    python3 scripts/test_verify_ra_thin_reader_helper.py
    python3 scripts/verify_ra_thin_reader_helper.py

verify-ra-bte-phase-prerequisite: check-workspace
    python3 scripts/test_verify_ra_bte_phase_prerequisite.py
    python3 scripts/verify_ra_bte_phase_prerequisite.py

verify-ra-gate0-catalog-persistence: check-workspace
    python3 scripts/test_verify_ra_gate0_catalog_persistence.py
    python3 scripts/verify_ra_gate0_catalog_persistence.py

verify-ra-leadlag-catalog-lift: check-workspace
    python3 scripts/test_verify_ra_leadlag_catalog_lift.py
    python3 scripts/verify_ra_leadlag_catalog_lift.py

verify-ra-sweep-orchestration: check-workspace
    python3 scripts/test_verify_ra_sweep_orchestration.py
    python3 scripts/verify_ra_sweep_orchestration.py

verify-ra-cost-realism: check-workspace
    python3 scripts/test_verify_ra_cost_realism.py
    python3 scripts/verify_ra_cost_realism.py

verify-ra-domain-metrics: check-workspace
    python3 scripts/test_verify_ra_domain_metrics.py
    python3 scripts/verify_ra_domain_metrics.py

verify-ra-findings-promotion: check-workspace
    python3 scripts/test_verify_ra_findings_promotion.py
    python3 scripts/verify_ra_findings_promotion.py

verify-ra-artifact-index-commit: check-workspace
    python3 scripts/test_verify_ra_artifact_index_commit.py
    python3 scripts/verify_ra_artifact_index_commit.py

verify-ra-run-pointer-index: check-workspace
    python3 scripts/test_verify_ra_run_pointer_index.py
    python3 scripts/verify_ra_run_pointer_index.py

verify-ra-bi-surface-and-feature-joins: check-workspace
    python3 scripts/test_verify_ra_bi_surface_and_feature_joins.py
    python3 scripts/verify_ra_bi_surface_and_feature_joins.py

verify-dashboard-customer-jobs: check-workspace
    python3 scripts/test_verify_dashboard_customer_jobs.py
    python3 scripts/verify_dashboard_customer_jobs.py

verify-dashboard-field-source-matrix: check-workspace
    python3 scripts/test_verify_dashboard_field_source_matrix.py
    python3 scripts/verify_dashboard_field_source_matrix.py

verify-dashboard-read-only-contract: check-workspace
    python3 scripts/test_verify_dashboard_read_only_contract.py
    python3 scripts/verify_dashboard_read_only_contract.py

verify-023-status-legend-registry: check-workspace
    python3 scripts/test_verify_023_status_legend_registry.py
    python3 scripts/verify_023_status_legend_registry.py

verify-bte-022-pmxt-durable-source: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_durable_source.py
    python3 scripts/verify_bte_022_pmxt_durable_source.py

verify-bte-022-pmxt-storage-proof: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_storage_proof.py
    python3 scripts/verify_bte_022_pmxt_storage_proof.py

verify-bte-022-pmxt-coverage-ledger: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_coverage_ledger.py
    python3 scripts/verify_bte_022_pmxt_coverage_ledger.py

verify-bte-022-pmxt-dynamic-tick-size: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py
    python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py

verify-bte-022-binary-option-bar-catalog: check-workspace
    python3 scripts/test_verify_bte_022_binary_option_bar_catalog.py
    python3 scripts/verify_bte_022_binary_option_bar_catalog.py

verify-bte-022-pmxt-broad-backfill-efficiency: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_broad_backfill_efficiency.py
    python3 scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py

verify-bolt-v3-legacy-default-fence: check-workspace
    python3 scripts/test_verify_bolt_v3_legacy_default_fence.py
    python3 scripts/verify_bolt_v3_legacy_default_fence.py

verify-bolt-v3-strategy-policy-fence: check-workspace
    python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/verify_bolt_v3_strategy_policy_fence.py

verify-bolt-v3-no-exit-market-command: check-workspace
    python3 scripts/test_verify_bolt_v3_no_exit_market_command.py
    python3 scripts/verify_bolt_v3_no_exit_market_command.py

verify-bolt-v3-usable-mu-sole-mint: check-workspace
    python3 scripts/test_verify_bolt_v3_usable_mu_sole_mint.py
    python3 scripts/verify_bolt_v3_usable_mu_sole_mint.py

verify-bolt-v3-no-venue-name-branch: check-workspace
    python3 scripts/test_verify_bolt_v3_no_venue_name_branch.py
    python3 scripts/verify_bolt_v3_no_venue_name_branch.py

verify-bolt-v3-requote-construction: check-workspace
    python3 scripts/test_verify_bolt_v3_requote_construction.py
    python3 scripts/verify_bolt_v3_requote_construction.py

verify-bolt-v3-market-family-coupling: check-workspace
    python3 scripts/test_verify_bolt_v3_market_family_coupling.py
    python3 scripts/verify_bolt_v3_market_family_coupling.py

verify-bolt-v3-dependency-direction: check-workspace
    python3 scripts/test_verify_bolt_v3_dependency_direction.py
    python3 scripts/verify_bolt_v3_dependency_direction.py

# Enforces "allowlist may only shrink" against the protected mainline: fails if
# the in-tree dependency allowlist is not a subset of the one on origin/main.
# No-op on the PR that first introduces the fence; active on every PR after merge.
verify-bolt-v3-dependency-shrink-only: check-workspace
    git fetch -q origin main 2>/dev/null
    python3 scripts/verify_bolt_v3_dependency_direction.py --check-shrink-only-vs-main

test-verify-runtime-capture-yaml: check-workspace
    python3 scripts/test_verify_runtime_capture_yaml.py

verify-runtime-capture-yaml: test-verify-runtime-capture-yaml
    python3 scripts/verify_runtime_capture_yaml.py

fmt-check: check-workspace require-rust-verification-owner
    python3 scripts/local_verification_gate.py fmt-check -- just fmt-check-inner

[private]
fmt-check-inner: require-local-verification-gate check-workspace require-rust-verification-owner verify-bolt-v3-runtime-literals verify-bolt-v3-provider-leaks
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fmt --check

fmt: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fmt

deny: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- deny check bans

deny-advisories: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- deny check advisories

[private]
managed-clippy: check-workspace
    if [ "${BOLT_MANAGED_JUST:-}" != "1" ]; then echo "ERROR: managed-clippy must run through scripts/rust_verification.py run"; exit 2; fi
    cargo clippy --locked -- -D warnings

[private]
managed-test *args: check-workspace
    if [ "${BOLT_MANAGED_JUST:-}" != "1" ]; then echo "ERROR: managed-test must run through scripts/rust_verification.py run"; exit 2; fi
    cargo nextest run --locked {{args}}

[private]
managed-build: check-workspace
    if [ "${BOLT_MANAGED_JUST:-}" != "1" ]; then echo "ERROR: managed-build must run through scripts/rust_verification.py run"; exit 2; fi
    cargo zigbuild --release --target {{target}} --locked

clippy: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}" clippy

test *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}" test {{args}}

test-archive archive *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- nextest archive --locked --archive-file "{{archive}}" {{args}}

test-archive-run archive extract_root *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- nextest run --archive-file "{{archive}}" --extract-to "{{extract_root}}" --extract-overwrite --workspace-remap "{{repo_root}}" {{args}}

build: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}" build

# backtesting-vertical-slice crate (separate workspace at crates/backtesting-vertical-slice/).
# Routed through the SAME managed wrapper as bolt-v2; --repo selects the crate's policy +
# justfile + its own `backtesting-vertical-slice` cache namespace. Used by .github/workflows/
# backtester-ci.yml and for local dev. bte-build is native (aarch64-apple-darwin), local-only.
# bte-fmt-check is the fast fail-early gate (no compile) that CI runs before clippy/test.
bte-fmt-check: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- fmt --check

bte-clippy: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}/crates/backtesting-vertical-slice" clippy

bte-test *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}/crates/backtesting-vertical-slice" test {{args}}

bte-test-archive archive *args: check-workspace require-rust-verification-owner
    archive_path="{{archive}}"; \
      case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac; \
      mkdir -p "$(dirname "$archive_path")"; \
      python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- nextest archive --locked --archive-file "$archive_path" {{args}}

bte-test-archive-run archive extract_root *args: check-workspace require-rust-verification-owner
    archive_path="{{archive}}"; \
      case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac; \
      python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- nextest run --archive-file "$archive_path" --extract-to "{{extract_root}}" --extract-overwrite --workspace-remap "{{repo_root}}/crates/backtesting-vertical-slice" {{args}}

bte-build: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}/crates/backtesting-vertical-slice" build

check-aarch64: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- check --target {{target}} --locked

verify-remote: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" verify-remote --repo "{{repo_root}}"

rust-probe *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" rust-probe --repo "{{repo_root}}" {{args}}

# Print failed-job diagnostics for the matching exact-head full-CI run; not a pass/fail gate.
ci-logs: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" ci-logs --repo "{{repo_root}}"

ci-runner-minutes *args:
    python3 scripts/ubicloud_runner_minutes.py {{args}}

ci-storage-audit *args: check-workspace
    python3 scripts/ci_storage_audit.py {{args}}

source-fence-static: check-workspace require-rust-verification-owner
    python3 scripts/local_verification_gate.py source-fence-static -- just source-fence-static-inner

[private]
source-fence-static-inner: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 scripts/test_verify_bolt_v3_runtime_literals.py
    python3 scripts/verify_bolt_v3_runtime_literals.py
    python3 scripts/test_verify_bolt_v3_provider_leaks.py
    python3 scripts/verify_bolt_v3_provider_leaks.py
    python3 scripts/test_verify_bolt_v3_core_boundary.py
    python3 scripts/verify_bolt_v3_core_boundary.py
    python3 scripts/test_verify_bolt_v3_naming.py
    python3 scripts/verify_bolt_v3_naming.py
    python3 scripts/test_verify_bolt_v3_dependency_direction.py
    python3 scripts/verify_bolt_v3_dependency_direction.py
    python3 scripts/test_verify_bolt_v3_status_map_current.py
    python3 scripts/verify_bolt_v3_status_map_current.py
    python3 scripts/test_verify_bolt_v3_schema_current.py
    python3 scripts/verify_bolt_v3_schema_current.py
    python3 scripts/test_migrate_bolt_v3_decision_evidence_v13_to_v14.py
    python3 scripts/test_migrate_bolt_v3_capital_admission_config.py
    python3 scripts/test_verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/test_verify_ra_single_engine_import_boundary.py
    python3 scripts/verify_ra_single_engine_import_boundary.py
    python3 scripts/test_verify_ra_notebook_read_only_boundary.py
    python3 scripts/verify_ra_notebook_read_only_boundary.py
    python3 scripts/test_verify_ra_point_in_time_leakage.py
    python3 scripts/verify_ra_point_in_time_leakage.py
    python3 scripts/test_verify_ra_thin_reader_helper.py
    python3 scripts/verify_ra_thin_reader_helper.py
    python3 scripts/test_verify_ra_gate0_catalog_persistence.py
    python3 scripts/verify_ra_gate0_catalog_persistence.py
    python3 scripts/test_verify_ra_bte_phase_prerequisite.py
    python3 scripts/verify_ra_bte_phase_prerequisite.py
    python3 scripts/test_verify_ra_leadlag_catalog_lift.py
    python3 scripts/verify_ra_leadlag_catalog_lift.py
    python3 scripts/test_verify_ra_sweep_orchestration.py
    python3 scripts/verify_ra_sweep_orchestration.py
    python3 scripts/test_verify_ra_cost_realism.py
    python3 scripts/verify_ra_cost_realism.py
    python3 scripts/test_verify_ra_domain_metrics.py
    python3 scripts/verify_ra_domain_metrics.py
    python3 scripts/test_verify_ra_findings_promotion.py
    python3 scripts/verify_ra_findings_promotion.py
    python3 scripts/test_verify_ra_artifact_index_commit.py
    python3 scripts/verify_ra_artifact_index_commit.py
    python3 scripts/test_verify_ra_run_pointer_index.py
    python3 scripts/verify_ra_run_pointer_index.py
    python3 scripts/test_verify_ra_bi_surface_and_feature_joins.py
    python3 scripts/verify_ra_bi_surface_and_feature_joins.py
    python3 scripts/test_verify_023_status_legend_registry.py
    python3 scripts/verify_023_status_legend_registry.py
    python3 scripts/test_verify_bte_022_pmxt_durable_source.py
    python3 scripts/verify_bte_022_pmxt_durable_source.py
    python3 scripts/test_verify_bte_022_pmxt_storage_proof.py
    python3 scripts/verify_bte_022_pmxt_storage_proof.py
    python3 scripts/test_verify_bte_022_pmxt_coverage_ledger.py
    python3 scripts/verify_bte_022_pmxt_coverage_ledger.py
    python3 scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py
    python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py
    python3 scripts/test_verify_bte_022_binary_option_bar_catalog.py
    python3 scripts/verify_bte_022_binary_option_bar_catalog.py
    python3 scripts/test_verify_bte_022_pmxt_broad_backfill_efficiency.py
    python3 scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py
    python3 scripts/test_verify_bte_test_topology.py
    python3 scripts/verify_bte_test_topology.py
    python3 scripts/test_verify_dashboard_customer_jobs.py
    python3 scripts/verify_dashboard_customer_jobs.py
    python3 scripts/test_verify_dashboard_field_source_matrix.py
    python3 scripts/verify_dashboard_field_source_matrix.py
    python3 scripts/test_verify_dashboard_read_only_contract.py
    python3 scripts/verify_dashboard_read_only_contract.py
    python3 scripts/test_verify_bolt_v3_legacy_default_fence.py
    python3 scripts/verify_bolt_v3_legacy_default_fence.py
    python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/test_verify_outcome_group_nt_reuse.py
    python3 scripts/verify_outcome_group_nt_reuse.py
    python3 scripts/test_verify_bolt_v3_no_exit_market_command.py
    python3 scripts/verify_bolt_v3_no_exit_market_command.py
    python3 scripts/test_verify_bolt_v3_usable_mu_sole_mint.py
    python3 scripts/verify_bolt_v3_usable_mu_sole_mint.py
    python3 scripts/test_verify_bolt_v3_no_venue_name_branch.py
    python3 scripts/verify_bolt_v3_no_venue_name_branch.py
    python3 scripts/test_verify_bolt_v3_requote_construction.py
    python3 scripts/verify_bolt_v3_requote_construction.py
    python3 scripts/test_verify_bolt_v3_market_family_coupling.py
    python3 scripts/verify_bolt_v3_market_family_coupling.py
    python3 scripts/test_verify_runtime_capture_yaml.py
    python3 scripts/test_local_verification_gate.py
    python3 scripts/test_lane_governor.py
    python3 scripts/test_verify_lane_governance.py
    python3 scripts/verify_lane_governance.py
    python3 scripts/test_verify_install_unit_generated.py
    python3 scripts/verify_install_unit_generated.py

source-fence: source-fence-static
    git fetch -q origin main 2>/dev/null
    python3 scripts/verify_bolt_v3_dependency_direction.py --check-shrink-only-vs-main
    # Fresh CI runners need the pinned NT checkout before source-capture checks.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fetch --locked
    python3 scripts/verify_runtime_capture_yaml.py
    # #342 owns these canonical source-fence checks. Until #332 changes full
    # nextest ownership, `test` intentionally still duplicates them under `gate`.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- test --locked --test wiring_registration -- bolt_v3_controlled_connect:: bolt_v3_production_entrypoint:: --nocapture
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- test --locked --test iv -- bolt_v3_iv_source_fence:: --nocapture

# Cargo shim guard tests (pytest-based, unlike the self-running script tests)
cargo-shim-tests:
    python3 -m pytest scripts/test_cargo_shim.py -q

# Render the systemd unit from deploy/install-layout.env + the .in template. The
# committed deploy/systemd/bolt-v2.service is a GENERATED artifact — edit the template
# or layout and regenerate; never hand-edit the unit. Drift is caught by source-fence.
generate-unit:
    python3 scripts/render_install_unit.py > deploy/systemd/bolt-v2.service

# Generate the runtime config by composing the operator-selected tracked profile
# overlay onto config/root.toml. The single, fail-closed path from a reviewed
# profile ID to a deployable runtime config; operators never hand-edit the
# runtime config (issue #768).
live-generate: check-workspace require-live-profile require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- ops generate-live-config --profile "{{live_profile}}" --config-root config

# Prove a deployed runtime config regenerates from the tracked profile and still
# loads against this exact binary (byte parity + independent schema load).
live-verify: check-workspace require-live-profile require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- ops verify-live-config --profile "{{live_profile}}" --config-root config

# Canonical repo-local operator lane for bolt-v2 from this checkout.
live: live-generate
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- ops launch --profile "{{live_profile}}" --config-root config

ci-lint-workflow: check-workspace require-rust-verification-owner
    python3 scripts/local_verification_gate.py ci-lint-workflow -- just ci-lint-workflow-inner

[private]
ci-lint-workflow-inner: require-local-verification-gate check-workspace require-rust-verification-owner
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    workflow_files=()
    action_files=()
    github_script_files=()

    [ -f .github/workflows/ci.yml ] && workflow_files+=(.github/workflows/ci.yml)
    [ -f .github/workflows/advisory.yml ] && workflow_files+=(.github/workflows/advisory.yml)
    [ -f .github/actions/setup-environment/action.yml ] && action_files+=(.github/actions/setup-environment/action.yml)
    github_script_files=(.github/scripts/*.sh)

    github_automation_files=("${workflow_files[@]}" "${action_files[@]}" "${github_script_files[@]}")
    repo_governance_files=()
    [ -f .no-mistakes.yaml ] && repo_governance_files+=(.no-mistakes.yaml)
    rust_invocation_files=(justfile "${repo_governance_files[@]}" scripts/*.sh tests/*.sh "${github_automation_files[@]}")

    if [ "${#github_automation_files[@]}" -eq 0 ]; then
        echo "No workflow or action files found — skipping"
    fi

    failed=0
    pattern='(^|[^[:alnum:]_])cargo[[:space:]]+(audit|bench|build|check|clean|clippy|deny|doc|fetch|fmt|install|nextest|run|rustc|test|version|zigbuild)([^[:alnum:]_]|$)'
    bypass_pattern='(^|[^[:alnum:]_./-])(command[[:space:]]+cargo|~\/\.cargo\/bin\/cargo|\/[^[:space:]]*\/\.cargo\/bin\/cargo)([^[:alnum:]_./-]|$)'
    just_target='{{target}}'
    managed_build_profile='release'
    policy_json="$(python3 "{{rust_verification_owner}}" validate-policy --repo "{{repo_root}}")"
    toml_target="$(printf '%s\n' "$policy_json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["build_target"])')"
    toml_profile="$(printf '%s\n' "$policy_json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["build_profile"])')"
    if ! python3 scripts/test_verify_ci_workflow_hygiene.py; then
        failed=1
    fi
    if ! python3 scripts/test_ci_test_manifest.py; then
        failed=1
    fi
    if ! python3 scripts/test_cancel_obsolete_dispatch_runs.py; then
        failed=1
    fi
    if ! python3 scripts/test_run_rust_probe.py; then
        failed=1
    fi
    if ! python3 scripts/test_rust_probe_wrapper.py; then
        failed=1
    fi
    if ! python3 scripts/test_ci_provenance.py; then
        failed=1
    fi
    if ! python3 scripts/test_merge_readiness.py; then
        failed=1
    fi
    if ! python3 scripts/test_coverage_enforcer.py; then
        failed=1
    fi
    if ! python3 scripts/test_nextest_fingerprint.py; then
        failed=1
    fi
    if ! python3 scripts/test_root_bin_sidecars.py; then
        failed=1
    fi
    if ! python3 scripts/test_ci_storage_audit.py; then
        failed=1
    fi
    if ! python3 scripts/test_find_same_sha_main_evidence.py; then
        failed=1
    fi
    if ! python3 scripts/test_ubicloud_runner_minutes.py; then
        failed=1
    fi
    if ! python3 scripts/test_verify_ci_path_filters.py; then
        failed=1
    fi
    if ! python3 scripts/test_rust_verification.py; then
        failed=1
    fi
    if ! python3 scripts/test_verify_remote.py; then
        failed=1
    fi
    if ! python3 scripts/test_command_understanding.py; then
        failed=1
    fi
    if ! python3 scripts/test_rust_verification_decoupling.py; then
        failed=1
    fi
    if ! python3 scripts/test_rust_verification_cache_retention.py; then
        failed=1
    fi
    if ! python3 scripts/verify_ci_path_filters.py; then
        failed=1
    fi
    if ! python3 scripts/verify_ci_workflow_hygiene.py; then
        failed=1
    fi

    for f in "${github_automation_files[@]}"; do
        if grep -En "$pattern" "$f"; then
            echo "ERROR: Raw cargo commands found in $f"
            failed=1
        fi
    done

    for f in "${rust_invocation_files[@]}"; do
        if grep -En "$bypass_pattern" "$f"; then
            echo "ERROR: Rust wrapper bypass found in $f"
            failed=1
        fi
    done

    if [ "$toml_target" != "$just_target" ]; then
        echo "ERROR: justfile target ($just_target) does not match ci/rust-verification.toml build target ($toml_target)"
        failed=1
    fi

    if [ "$toml_profile" != "$managed_build_profile" ]; then
        echo "ERROR: managed-build profile ($managed_build_profile) does not match ci/rust-verification.toml build profile ($toml_profile)"
        failed=1
    fi

    if [ "$failed" -ne 0 ]; then
        echo "All tracked automation must avoid raw cargo workflow commands, explicit Rust-wrapper bypasses, and justfile/TOML build drift."
        exit 1
    fi

    if [ "${#github_automation_files[@]}" -eq 0 ]; then
        echo "OK: No workflow or action files found; automation-specific checks skipped"
    else
        echo "OK: No raw cargo workflow commands or explicit Rust-wrapper bypasses found"
    fi

worktree branch:
    #!/usr/bin/env bash
    set -euo pipefail
    dest="{{worktree_root}}/{{branch}}"
    mkdir -p "$(dirname "$dest")"

    if git show-ref --verify --quiet "refs/heads/{{branch}}"; then
        git worktree add "$dest" "{{branch}}"
    elif git show-ref --verify --quiet "refs/remotes/origin/{{branch}}"; then
        git worktree add --track -b "{{branch}}" "$dest" "origin/{{branch}}"
    elif git ls-remote --exit-code --heads origin "refs/heads/{{branch}}" >/dev/null 2>&1; then
        git fetch origin "refs/heads/{{branch}}:refs/remotes/origin/{{branch}}"
        git worktree add --track -b "{{branch}}" "$dest" "origin/{{branch}}"
    else
        git worktree add "$dest" -b "{{branch}}"
    fi

    echo "Created worktree at $dest"

worktree-remove branch:
    #!/usr/bin/env bash
    dest="{{worktree_root}}/{{branch}}"
    git worktree remove "$dest"
    git worktree prune
    echo "Removed worktree at $dest"

# clean-merged: auto-cleanup of merged branches and worktrees.
# See docs/ops/clean-merged-design.md. Default = dry-run; pass --apply to execute.
clean-merged *args:
    python3 scripts/clean_merged_artifacts.py {{args}}

# clean-merged: print install/heartbeat/quarantine/gh health.
clean-merged-doctor:
    python3 scripts/clean_merged_artifacts.py --doctor

# clean-merged: one-time bulk reclaim of the worktree backlog.
# Prints a dry-run first; pass --apply to actually archive+remove.
clean-merged-backlog *args:
    python3 scripts/clean_merged_artifacts.py --include-worktrees {{args}}

# clean-merged: prune quarantine archives and backup refs older than DAYS (default 30).
clean-merged-purge days='30':
    python3 scripts/clean_merged_artifacts.py --purge-quarantine {{days}}
    python3 scripts/clean_merged_artifacts.py --prune-backups {{days}}

setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Setting git hooks path..."
    git config core.hooksPath .githooks
    # Ensure managed hooks are executable (git warns + skips otherwise).
    chmod +x .githooks/post-merge .githooks/post-checkout .githooks/post-rewrite 2>/dev/null || true

    echo "Enabling remote.origin.prune (auto-prune deleted upstreams on fetch)..."
    git config remote.origin.prune true

    echo "Adding {{target}} target..."
    rustup target add {{target}}

    if command -v cargo-nextest >/dev/null 2>&1 && cargo-nextest --version | grep -Eq "^cargo-nextest {{nextest_version}}([[:space:]]|$)"; then
        echo "cargo-nextest {{nextest_version}} already installed"
    else
        echo "ERROR: cargo-nextest {{nextest_version}} is required as a prebuilt tool"
        exit 2
    fi

    if command -v cargo-deny >/dev/null 2>&1 && cargo-deny --version | grep -Eq "^cargo-deny {{deny_version}}([[:space:]]|$)"; then
        echo "cargo-deny {{deny_version}} already installed"
    else
        echo "ERROR: cargo-deny {{deny_version}} is required as a prebuilt tool"
        exit 2
    fi

    if command -v cargo-zigbuild >/dev/null 2>&1 && cargo-zigbuild --version | grep -Eq "^cargo-zigbuild {{zigbuild_version}}([[:space:]]|$)"; then
        echo "cargo-zigbuild {{zigbuild_version}} already installed"
    else
        echo "ERROR: cargo-zigbuild {{zigbuild_version}} is required as a prebuilt tool"
        exit 2
    fi

    if ! command -v zig >/dev/null 2>&1; then
        echo "ERROR: Zig {{zig_version}} is required for just build"
        echo "Install it locally with 'brew install zig'"
        exit 1
    fi

    if [ "$(zig version)" != "{{zig_version}}" ]; then
        echo "ERROR: Zig {{zig_version}} is required for just build"
        echo "Found Zig $(zig version)"
        exit 1
    fi

    echo "Zig {{zig_version}} already installed"

    echo "Setup complete."

# Create the CI runner debug SSH key in 1Password and publish SSH_PUBLIC_KEY to GitHub.
ci-debug-ssh-bootstrap:
    python3 scripts/sync_ci_debug_ssh_secret.py bootstrap

# Publish the CI runner debug SSH public key from 1Password to GitHub Actions.
ci-debug-ssh-sync:
    python3 scripts/sync_ci_debug_ssh_secret.py sync
