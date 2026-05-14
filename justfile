set shell := ["bash", "-euo", "pipefail", "-c"]

# bolt-v2 build commands — single source of truth.
# CI and local both call these recipes. No raw cargo build/check commands in workflow YAML.

nextest_version := "0.9.132"
deny_version := "0.19.0"
zigbuild_version := "0.22.1"
zig_version := "0.15.2"

target := "aarch64-unknown-linux-gnu"
worktree_root := env_var('HOME') + "/worktrees/bolt-v2"
live_input := "config/live.local.toml"
live_input_example := "config/live.local.example.toml"
live_config := "config/live.toml"
repo_root := justfile_directory()
rust_verification_owner := env_var('HOME') + "/.claude/lib/rust_verification.py"
rust_verification_source_repo := "seungpyoson/claude-config"
rust_verification_source_sha := "50a8b4fb40d5ec4a83de2fa545083355970a7c78"
rust_verification_require_script := "scripts/require_rust_verification_owner.sh"
rust_verification_ci_install_script := "scripts/install_ci_rust_verification_owner.sh"

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
    RUST_VERIFICATION_SOURCE_REPO="{{rust_verification_source_repo}}" RUST_VERIFICATION_SOURCE_SHA="{{rust_verification_source_sha}}" bash "{{rust_verification_require_script}}" "{{rust_verification_owner}}"

verify-bolt-v3-runtime-literals: check-workspace
    python3 scripts/test_verify_bolt_v3_runtime_literals.py
    python3 scripts/verify_bolt_v3_runtime_literals.py

verify-bolt-v3-provider-leaks: check-workspace
    python3 scripts/test_verify_bolt_v3_provider_leaks.py
    python3 scripts/verify_bolt_v3_provider_leaks.py

verify-bolt-v3-core-boundary: check-workspace
    python3 scripts/test_verify_bolt_v3_core_boundary.py
    python3 scripts/verify_bolt_v3_core_boundary.py

verify-bolt-v3-naming: check-workspace
    python3 scripts/test_verify_bolt_v3_naming.py
    python3 scripts/verify_bolt_v3_naming.py

verify-bolt-v3-status-map-current: check-workspace
    python3 scripts/test_verify_bolt_v3_status_map_current.py
    python3 scripts/verify_bolt_v3_status_map_current.py

verify-bolt-v3-pure-rust-runtime: check-workspace
    python3 scripts/test_verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/verify_bolt_v3_pure_rust_runtime.py

fmt-check: check-workspace require-rust-verification-owner verify-bolt-v3-runtime-literals verify-bolt-v3-provider-leaks verify-bolt-v3-core-boundary verify-bolt-v3-naming verify-bolt-v3-status-map-current verify-bolt-v3-pure-rust-runtime
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fmt --check

fmt: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fmt

deny: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- deny check bans

deny-advisories: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- deny check advisories

[private]
managed-clippy: check-workspace
    cargo clippy --locked -- -D warnings

[private]
managed-test: check-workspace
    cargo nextest run --locked

[private]
managed-build: check-workspace
    cargo zigbuild --release --target {{target}} --locked

clippy: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}" clippy

test: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}" test

build: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" run --repo "{{repo_root}}" build

check-aarch64: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- check --target {{target}} --locked

source-fence: check-workspace require-rust-verification-owner
    python3 scripts/test_verify_bolt_v3_runtime_literals.py
    python3 scripts/verify_bolt_v3_runtime_literals.py
    python3 scripts/test_verify_bolt_v3_provider_leaks.py
    python3 scripts/verify_bolt_v3_provider_leaks.py
    python3 scripts/test_verify_bolt_v3_core_boundary.py
    python3 scripts/verify_bolt_v3_core_boundary.py
    python3 scripts/test_verify_bolt_v3_naming.py
    python3 scripts/verify_bolt_v3_naming.py
    python3 scripts/test_verify_bolt_v3_status_map_current.py
    python3 scripts/verify_bolt_v3_status_map_current.py
    python3 scripts/test_verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/verify_bolt_v3_pure_rust_runtime.py
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- test --test bolt_v3_controlled_connect live_node_module_only_runs_nt_after_live_canary_gate -- --nocapture
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- test --test bolt_v3_production_entrypoint -- --nocapture

live-generate: check-workspace require-rust-verification-owner
    #!/usr/bin/env bash
    # Generate the runtime artifact from the human-edited local source of truth.
    if [ ! -f "{{live_input}}" ]; then
        echo "Missing {{live_input}}"
        echo "Create it from {{live_input_example}}, then rerun."
        exit 1
    fi

    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --quiet --bin render_live_config -- --input {{live_input}} --output {{live_config}}

# Canonical repo-local operator lane for bolt-v2 from this checkout.
live: live-generate
    # Run with the generated runtime config artifact.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- run --config {{live_config}}

# Optional diagnostics for the live operator config.
live-check: live-generate
    # Validate secret-config completeness only; do not resolve secrets.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- secrets check --config {{live_config}}

live-resolve: live-generate
    # Perform actual secret resolution against the generated runtime config.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- secrets resolve --config {{live_config}}

ci-lint-workflow:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    workflow_files=()
    action_files=()

    [ -f .github/workflows/ci.yml ] && workflow_files+=(.github/workflows/ci.yml)
    [ -f .github/workflows/advisory.yml ] && workflow_files+=(.github/workflows/advisory.yml)
    [ -f .github/actions/setup-environment/action.yml ] && action_files+=(.github/actions/setup-environment/action.yml)

    github_automation_files=("${workflow_files[@]}" "${action_files[@]}")
    rust_invocation_files=(justfile scripts/*.sh tests/*.sh "${github_automation_files[@]}")

    if [ "${#github_automation_files[@]}" -eq 0 ]; then
        echo "No workflow or action files found — skipping"
    fi

    failed=0
    pattern='(^|[^[:alnum:]_])cargo[[:space:]]+(fmt|clippy|test|nextest|zigbuild|deny|audit|build|check)([^[:alnum:]_]|$)'
    bypass_pattern='(^|[^[:alnum:]_./-])(command[[:space:]]+cargo|~\/\.cargo\/bin\/cargo|\/[^[:space:]]*\/\.cargo\/bin\/cargo)([^[:alnum:]_./-]|$)'
    just_lane_pattern='(^|[^[:alnum:]_./-])just[[:space:]]+(fmt-check|deny|deny-advisories|clippy|test|build|check-aarch64|source-fence)([^[:alnum:]_]|$)'
    setup_action_literal='uses: ./.github/actions/setup-environment'
    setup_lint_literal='lint-workflow-contract:'
    setup_lint_true_literal='lint-workflow-contract: "true"'
    setup_token_literal='claude-config-read-token:'
    setup_token_source_literal='secrets.CLAUDE_CONFIG_READ_TOKEN'
    setup_just_version_literal='just-version:'
    setup_just_version_source_literal='env.JUST_VERSION'
    setup_deny_version_literal='include-deny-version: "true"'
    setup_nextest_version_literal='include-nextest-version: "true"'
    setup_build_values_literal='include-build-values: "true"'
    setup_rustfmt_literal='toolchain-components: rustfmt'
    setup_clippy_literal='toolchain-components: clippy'
    setup_default_target_literal='use-default-target: "true"'
    managed_binary_path_literal='binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'
    deny_output_literal='steps.setup.outputs.deny_version'
    nextest_output_literal='steps.setup.outputs.nextest_version'
    zig_version_output_literal='steps.setup.outputs.zig_version'
    zigbuild_version_output_literal='steps.setup.outputs.zigbuild_version'
    managed_target_dir_output_literal='steps.setup.outputs.managed_target_dir'
    gate_if_always_pattern='always\(\)'
    build_required_output_pattern='needs\.[A-Za-z0-9_-]+\.outputs\.build_required'
    repo_local_artifact_pattern='(^|[^[:alnum:]_./-])target/.*/release/bolt-v2(\.sha256)?([^[:alnum:]_./-]|$)'
    just_target='{{target}}'
    managed_build_profile='release'
    toml_target="$(python3 -c "import pathlib, tomllib; print(tomllib.load(pathlib.Path('.claude/rust-verification.toml').open('rb'))['commands']['build']['target'])")"
    toml_profile="$(python3 -c "import pathlib, tomllib; print(tomllib.load(pathlib.Path('.claude/rust-verification.toml').open('rb'))['commands']['build']['profile'])")"
    action_file='.github/actions/setup-environment/action.yml'
    action_lint_line=0
    action_shared_line=0
    action_owner_line=0
    action_target_dir_line=0
    action_toolchain_line=0
    action_required_literals=(
        "inputs.just-version"
        "inputs.include-deny-version"
        "inputs.include-nextest-version"
        "inputs.include-build-values"
        "inputs.lint-workflow-contract"
        "CLAUDE_CONFIG_READ_TOKEN:"
        "inputs.claude-config-read-token"
        "just ci-lint-workflow"
        "awk -F'\\\"' '/^channel = / {print \$2}' rust-toolchain.toml"
        "just --evaluate deny_version"
        "just --evaluate nextest_version"
        "just --evaluate target"
        "just --evaluate zig_version"
        "just --evaluate zigbuild_version"
        "just --evaluate rust_verification_owner"
        "just --evaluate rust_verification_source_repo"
        "just --evaluate rust_verification_source_sha"
        "just --evaluate rust_verification_ci_install_script"
        'target-dir --repo "$GITHUB_WORKSPACE"'
    )
    action_output_names=(
        "rust_toolchain"
        "deny_version"
        "nextest_version"
        "target"
        "zig_version"
        "zigbuild_version"
        "rust_verification_owner"
        "rust_verification_source_repo"
        "rust_verification_source_sha"
        "rust_verification_ci_install_script"
        "managed_target_dir"
    )

    for f in "${github_automation_files[@]}"; do
        if grep -En "$pattern" "$f"; then
            echo "ERROR: Raw cargo commands found in $f"
            failed=1
        fi
    done

    for f in "${workflow_files[@]}"; do
        while IFS='|' read -r job_name reason; do
            [ -n "$job_name" ] || continue
            case "$reason" in
                setup-action)
                    echo "ERROR: Managed CI setup action missing in $f job '$job_name'"
                    ;;
                setup-token)
                    echo "ERROR: Managed CI setup token wiring missing in $f job '$job_name'"
                    ;;
                setup-lint)
                    echo "ERROR: Managed CI workflow lint wiring missing in $f job '$job_name'"
                    ;;
                setup-lint-true)
                    echo "ERROR: Managed CI workflow lint must be enabled in $f job '$job_name'"
                    ;;
                setup-deny-version)
                    echo "ERROR: Managed CI deny-version wiring missing in $f job '$job_name'"
                    ;;
                setup-nextest-version)
                    echo "ERROR: Managed CI nextest-version wiring missing in $f job '$job_name'"
                    ;;
                setup-build-values)
                    echo "ERROR: Managed CI build-values wiring missing in $f job '$job_name'"
                    ;;
                setup-rustfmt)
                    echo "ERROR: Managed CI rustfmt component wiring missing in $f job '$job_name'"
                    ;;
                setup-clippy)
                    echo "ERROR: Managed CI clippy component wiring missing in $f job '$job_name'"
                    ;;
                setup-default-target)
                    echo "ERROR: Managed CI default target wiring missing in $f job '$job_name'"
                    ;;
                setup-just-version)
                    echo "ERROR: Managed CI just version wiring missing in $f job '$job_name'"
                    ;;
                setup-just-version-source)
                    echo "ERROR: Managed CI just version source must come from env.JUST_VERSION in $f job '$job_name'"
                    ;;
                setup-token-source)
                    echo "ERROR: Managed CI token source must come from secrets.CLAUDE_CONFIG_READ_TOKEN in $f job '$job_name'"
                    ;;
                managed-target-dir)
                    echo "ERROR: Managed CI managed target dir wiring missing in $f job '$job_name'"
                    ;;
                cache-key)
                    echo "ERROR: Managed CI rust-cache key wiring missing in $f job '$job_name'"
                    ;;
                gate-always)
                    echo "ERROR: Managed CI aggregate gate missing always() guard in $f job '$job_name'"
                    ;;
                gate-detector-result)
                    echo "ERROR: Managed CI aggregate gate missing detector result check in $f job '$job_name'"
                    ;;
                gate-fmt-result)
                    echo "ERROR: Managed CI aggregate gate missing fmt-check result check in $f job '$job_name'"
                    ;;
                gate-deny-result)
                    echo "ERROR: Managed CI aggregate gate missing deny result check in $f job '$job_name'"
                    ;;
                gate-clippy-result)
                    echo "ERROR: Managed CI aggregate gate missing clippy result check in $f job '$job_name'"
                    ;;
                gate-test-result)
                    echo "ERROR: Managed CI aggregate gate missing test result check in $f job '$job_name'"
                    ;;
                source-fence-job)
                    echo "ERROR: Managed CI source-fence job missing in $f"
                    ;;
                source-fence-detector-needs)
                    echo "ERROR: Managed CI source-fence job must depend on detector in $f"
                    ;;
                test-source-fence-needs)
                    echo "ERROR: Managed CI test job must depend on source-fence in $f"
                    ;;
                gate-source-fence-needs)
                    echo "ERROR: Managed CI aggregate gate must need source-fence in $f"
                    ;;
                gate-source-fence-result)
                    echo "ERROR: Managed CI aggregate gate missing source-fence result check in $f job '$job_name'"
                    ;;
                gate-build-result)
                    echo "ERROR: Managed CI aggregate gate missing build result check in $f job '$job_name'"
                    ;;
                gate-build-required)
                    echo "ERROR: Managed CI aggregate gate missing build_required handling in $f job '$job_name'"
                    ;;
                build-required-output)
                    echo "ERROR: Managed CI build lane missing detector build_required gating in $f job '$job_name'"
                    ;;
            esac
            failed=1
        done < <(
            awk -v lane_pattern="$just_lane_pattern" \
                -v setup_action_literal="$setup_action_literal" \
                -v setup_lint_literal="$setup_lint_literal" \
                -v setup_lint_true_literal="$setup_lint_true_literal" \
                -v setup_token_literal="$setup_token_literal" \
                -v setup_token_source_literal="$setup_token_source_literal" \
                -v setup_just_version_literal="$setup_just_version_literal" \
                -v setup_just_version_source_literal="$setup_just_version_source_literal" \
                -v setup_deny_version_literal="$setup_deny_version_literal" \
                -v setup_nextest_version_literal="$setup_nextest_version_literal" \
                -v setup_build_values_literal="$setup_build_values_literal" \
                -v setup_rustfmt_literal="$setup_rustfmt_literal" \
                -v setup_clippy_literal="$setup_clippy_literal" \
                -v setup_default_target_literal="$setup_default_target_literal" \
                -v deny_output_literal="$deny_output_literal" \
                -v nextest_output_literal="$nextest_output_literal" \
                -v zig_version_output_literal="$zig_version_output_literal" \
                -v zigbuild_version_output_literal="$zigbuild_version_output_literal" '
                BEGIN {
                    in_jobs = 0
                    current = ""
                    has_lane = 0
                    has_setup_step = 0
                    has_setup_lint = 0
                    has_setup_lint_true = 0
                    has_setup_token = 0
                    has_setup_token_source = 0
                    has_setup_just_version = 0
                    has_setup_just_version_source = 0
                    has_setup_deny_version = 0
                    has_setup_nextest_version = 0
                    has_setup_build_values = 0
                    has_setup_rustfmt = 0
                    has_setup_clippy = 0
                    has_setup_default_target = 0
                    has_deny_output = 0
                    has_nextest_output = 0
                    has_zig_version_output = 0
                    has_zigbuild_version_output = 0
                    step_has_setup = 0
                    step_has_lint = 0
                    step_has_lint_true = 0
                    step_has_token = 0
                    step_has_token_source = 0
                    step_has_just_version = 0
                    step_has_just_version_source = 0
                    step_has_deny_version = 0
                    step_has_nextest_version = 0
                    step_has_build_values = 0
                    step_has_rustfmt = 0
                    step_has_clippy = 0
                    step_has_default_target = 0
                }

                function flush_step() {
                    if (step_has_setup) {
                        has_setup_step = 1
                        if (step_has_lint) {
                            has_setup_lint = 1
                        }
                        if (step_has_lint_true) {
                            has_setup_lint_true = 1
                        }
                        if (step_has_token) {
                            has_setup_token = 1
                        }
                        if (step_has_token_source) {
                            has_setup_token_source = 1
                        }
                        if (step_has_just_version) {
                            has_setup_just_version = 1
                        }
                        if (step_has_just_version_source) {
                            has_setup_just_version_source = 1
                        }
                        if (step_has_deny_version) {
                            has_setup_deny_version = 1
                        }
                        if (step_has_nextest_version) {
                            has_setup_nextest_version = 1
                        }
                        if (step_has_build_values) {
                            has_setup_build_values = 1
                        }
                        if (step_has_rustfmt) {
                            has_setup_rustfmt = 1
                        }
                        if (step_has_clippy) {
                            has_setup_clippy = 1
                        }
                        if (step_has_default_target) {
                            has_setup_default_target = 1
                        }
                    }
                    step_has_setup = 0
                    step_has_lint = 0
                    step_has_lint_true = 0
                    step_has_token = 0
                    step_has_token_source = 0
                    step_has_just_version = 0
                    step_has_just_version_source = 0
                    step_has_deny_version = 0
                    step_has_nextest_version = 0
                    step_has_build_values = 0
                    step_has_rustfmt = 0
                    step_has_clippy = 0
                    step_has_default_target = 0
                }

                function flush_job() {
                    flush_step()
                    if (current == "" || !has_lane) {
                        return
                    }
                    if (!has_setup_step) {
                        print current "|setup-action"
                    }
                    if (current == "fmt-check" && has_setup_step && !has_setup_lint) {
                        print current "|setup-lint"
                    }
                    if (current == "fmt-check" && has_setup_step && !has_setup_lint_true) {
                        print current "|setup-lint-true"
                    }
                    if (has_setup_step && !has_setup_token) {
                        print current "|setup-token"
                    }
                    if (has_setup_step && !has_setup_token_source) {
                        print current "|setup-token-source"
                    }
                    if (has_setup_step && !has_setup_just_version) {
                        print current "|setup-just-version"
                    }
                    if (has_setup_step && !has_setup_just_version_source) {
                        print current "|setup-just-version-source"
                    }
                    if ((current == "deny" || current == "advisories") && has_setup_step && !has_setup_deny_version) {
                        print current "|setup-deny-version"
                    }
                    if (current == "test" && has_setup_step && !has_setup_nextest_version) {
                        print current "|setup-nextest-version"
                    }
                    if (current == "build" && has_setup_step && !has_setup_build_values) {
                        print current "|setup-build-values"
                    }
                    if (current == "fmt-check" && has_setup_step && !has_setup_rustfmt) {
                        print current "|setup-rustfmt"
                    }
                    if (current == "clippy" && has_setup_step && !has_setup_clippy) {
                        print current "|setup-clippy"
                    }
                    if (current == "build" && has_setup_step && !has_setup_default_target) {
                        print current "|setup-default-target"
                    }
                    if ((current == "deny" || current == "advisories") && !has_deny_output) {
                        print current "|setup-deny-version"
                    }
                    if (current == "test" && !has_nextest_output) {
                        print current "|setup-nextest-version"
                    }
                    if (current == "build" && (!has_zig_version_output || !has_zigbuild_version_output)) {
                        print current "|setup-build-values"
                    }
                }

                /^jobs:/ {
                    in_jobs = 1
                    next
                }

                in_jobs && /^[^[:space:]]/ {
                    flush_job()
                    in_jobs = 0
                    current = ""
                    has_lane = 0
                    has_setup_step = 0
                    has_setup_lint = 0
                    has_setup_lint_true = 0
                    has_setup_token = 0
                    has_setup_token_source = 0
                    has_setup_just_version = 0
                    has_setup_just_version_source = 0
                    has_setup_deny_version = 0
                    has_setup_nextest_version = 0
                    has_setup_build_values = 0
                    has_setup_rustfmt = 0
                    has_setup_clippy = 0
                    has_setup_default_target = 0
                    has_deny_output = 0
                    has_nextest_output = 0
                    has_zig_version_output = 0
                    has_zigbuild_version_output = 0
                    step_has_setup = 0
                    step_has_lint = 0
                    step_has_lint_true = 0
                    step_has_token = 0
                    step_has_token_source = 0
                    step_has_just_version = 0
                    step_has_just_version_source = 0
                    step_has_deny_version = 0
                    step_has_nextest_version = 0
                    step_has_build_values = 0
                    step_has_rustfmt = 0
                    step_has_clippy = 0
                    step_has_default_target = 0
                    next
                }

                in_jobs && /^  [A-Za-z0-9_-]+:/ {
                    flush_job()
                    current = $0
                    sub(/^  /, "", current)
                    sub(/:.*/, "", current)
                    has_lane = 0
                    has_setup_step = 0
                    has_setup_lint = 0
                    has_setup_lint_true = 0
                    has_setup_token = 0
                    has_setup_token_source = 0
                    has_setup_just_version = 0
                    has_setup_just_version_source = 0
                    has_setup_deny_version = 0
                    has_setup_nextest_version = 0
                    has_setup_build_values = 0
                    has_setup_rustfmt = 0
                    has_setup_clippy = 0
                    has_setup_default_target = 0
                    has_deny_output = 0
                    has_nextest_output = 0
                    has_zig_version_output = 0
                    has_zigbuild_version_output = 0
                    step_has_setup = 0
                    step_has_lint = 0
                    step_has_lint_true = 0
                    step_has_token = 0
                    step_has_token_source = 0
                    step_has_just_version = 0
                    step_has_just_version_source = 0
                    step_has_deny_version = 0
                    step_has_nextest_version = 0
                    step_has_build_values = 0
                    step_has_rustfmt = 0
                    step_has_clippy = 0
                    step_has_default_target = 0
                    next
                }

                current != "" {
                    if ($0 ~ /^      - /) {
                        flush_step()
                    }
                    if ($0 ~ lane_pattern) {
                        has_lane = 1
                    }
                    if (index($0, setup_action_literal) > 0) {
                        step_has_setup = 1
                    }
                    if (index($0, setup_lint_literal) > 0) {
                        step_has_lint = 1
                    }
                    if (index($0, setup_lint_true_literal) > 0) {
                        step_has_lint_true = 1
                    }
                    if (index($0, setup_token_literal) > 0) {
                        step_has_token = 1
                    }
                    if (index($0, setup_token_source_literal) > 0) {
                        step_has_token_source = 1
                    }
                    if (index($0, setup_just_version_literal) > 0) {
                        step_has_just_version = 1
                    }
                    if (index($0, setup_just_version_source_literal) > 0) {
                        step_has_just_version_source = 1
                    }
                    if (index($0, setup_deny_version_literal) > 0) {
                        step_has_deny_version = 1
                    }
                    if (index($0, setup_nextest_version_literal) > 0) {
                        step_has_nextest_version = 1
                    }
                    if (index($0, setup_build_values_literal) > 0) {
                        step_has_build_values = 1
                    }
                    if (index($0, setup_rustfmt_literal) > 0) {
                        step_has_rustfmt = 1
                    }
                    if (index($0, setup_clippy_literal) > 0) {
                        step_has_clippy = 1
                    }
                    if (index($0, setup_default_target_literal) > 0) {
                        step_has_default_target = 1
                    }
                    if (index($0, deny_output_literal) > 0) {
                        has_deny_output = 1
                    }
                    if (index($0, nextest_output_literal) > 0) {
                        has_nextest_output = 1
                    }
                    if (index($0, zig_version_output_literal) > 0) {
                        has_zig_version_output = 1
                    }
                    if (index($0, zigbuild_version_output_literal) > 0) {
                        has_zigbuild_version_output = 1
                    }
                }

                END {
                    flush_job()
                }
            ' "$f"
        )
    done

    if [ ! -f "$action_file" ]; then
        echo "ERROR: Managed CI setup action missing at $action_file"
        failed=1
    else
        action_lint_line="$(grep -n 'name: Lint workflow contract' "$action_file" | cut -d: -f1 | head -1 || true)"
        action_shared_line="$(grep -n 'name: Read shared values' "$action_file" | cut -d: -f1 | head -1 || true)"
        action_owner_line="$(grep -n 'name: Install managed Rust owner' "$action_file" | cut -d: -f1 | head -1 || true)"
        action_target_dir_line="$(grep -n 'name: Resolve managed target dir' "$action_file" | cut -d: -f1 | head -1 || true)"
        action_toolchain_line="$(grep -n 'name: Setup Rust toolchain' "$action_file" | cut -d: -f1 | head -1 || true)"

        if [ -z "$action_lint_line" ] || [ -z "$action_shared_line" ] || [ -z "$action_owner_line" ] || [ -z "$action_target_dir_line" ] || [ -z "$action_toolchain_line" ]; then
            echo "ERROR: Managed CI setup action missing required ordered steps"
            failed=1
        elif [ "$action_lint_line" -ge "$action_shared_line" ] || [ "$action_shared_line" -ge "$action_owner_line" ] || [ "$action_owner_line" -ge "$action_target_dir_line" ] || [ "$action_target_dir_line" -ge "$action_toolchain_line" ]; then
            echo "ERROR: Managed CI setup action step order drifted"
            failed=1
        fi

        for literal in "${action_required_literals[@]}"; do
            if ! grep -Fq "$literal" "$action_file"; then
                echo "ERROR: Managed CI setup action missing expected literal '$literal'"
                failed=1
            fi
        done

        for output_name in "${action_output_names[@]}"; do
            if ! grep -Eq "^  ${output_name}:" "$action_file"; then
                echo "ERROR: Managed CI setup action missing exported output '$output_name'"
                failed=1
            fi
            if [ "$output_name" = "managed_target_dir" ]; then
                if ! grep -Fq "steps.target_dir.outputs.${output_name}" "$action_file"; then
                    echo "ERROR: Managed CI setup action missing output mapping for '$output_name'"
                    failed=1
                fi
                continue
            fi
            if ! grep -Fq "steps.shared.outputs.${output_name}" "$action_file"; then
                echo "ERROR: Managed CI setup action missing output mapping for '$output_name'"
                failed=1
            fi
        done
    fi

    if [ -f .github/workflows/ci.yml ]; then
        while IFS='|' read -r job_name reason; do
            [ -n "$job_name" ] || continue
            case "$reason" in
                managed-target-dir)
                    echo "ERROR: .github/workflows/ci.yml job '$job_name' must use setup.outputs.managed_target_dir"
                    ;;
                cache-key)
                    echo "ERROR: .github/workflows/ci.yml job '$job_name' must declare an explicit rust-cache key"
                    ;;
                gate-always)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must use always()"
                    ;;
                gate-detector-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate detector result"
                    ;;
                gate-fmt-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate fmt-check result"
                    ;;
                gate-deny-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate deny result"
                    ;;
                gate-clippy-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate clippy result"
                    ;;
                gate-test-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate test result"
                    ;;
                source-fence-job)
                    echo "ERROR: .github/workflows/ci.yml source-fence job missing"
                    ;;
                source-fence-detector-needs)
                    echo "ERROR: .github/workflows/ci.yml source-fence job must depend on detector"
                    ;;
                test-source-fence-needs)
                    echo "ERROR: .github/workflows/ci.yml test job must depend on source-fence"
                    ;;
                gate-source-fence-needs)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must need source-fence"
                    ;;
                gate-source-fence-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate source-fence result"
                    ;;
                gate-build-result)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must validate build result"
                    ;;
                gate-build-required)
                    echo "ERROR: .github/workflows/ci.yml aggregate gate must handle build_required"
                    ;;
                build-required-output)
                    echo "ERROR: .github/workflows/ci.yml build job must gate on detector build_required output"
                    ;;
            esac
            failed=1
        done < <(
            awk -v managed_target_dir_output_literal="$managed_target_dir_output_literal" \
                -v gate_if_always_pattern="$gate_if_always_pattern" \
                -v build_required_output_pattern="$build_required_output_pattern" '
                BEGIN {
                    in_jobs = 0
                    current = ""
                    saw_source_fence = 0
                    has_source_fence_detector_needs = 0
                    has_managed_target_dir = 0
                    has_cache_key = 0
                    has_test_source_fence_needs = 0
                    has_gate_always = 0
                    has_gate_source_fence_needs = 0
                    has_gate_detector_result = 0
                    has_gate_fmt_result = 0
                    has_gate_deny_result = 0
                    has_gate_clippy_result = 0
                    has_gate_test_result = 0
                    has_gate_source_fence_result = 0
                    has_gate_build_result = 0
                    has_gate_build_required = 0
                    has_build_required_output = 0
                    in_needs_block = 0
                }

                function clean_yaml_line(line) {
                    sub(/[[:space:]]+#.*/, "", line)
                    return line
                }

                function mark_source_fence_need() {
                    if (current == "test") {
                        has_test_source_fence_needs = 1
                    }
                    if (current == "gate") {
                        has_gate_source_fence_needs = 1
                    }
                }

                function mark_detector_need() {
                    if (current == "source-fence") {
                        has_source_fence_detector_needs = 1
                    }
                }

                function mark_need(name) {
                    if (name == "detector") {
                        mark_detector_need()
                    }
                    if (name == "source-fence") {
                        mark_source_fence_need()
                    }
                }

                function scan_needs(line, clean) {
                    clean = clean_yaml_line(line)
                    if (clean ~ /^    needs:/) {
                        if (index(clean, "detector") > 0) {
                            mark_need("detector")
                        }
                        if (index(clean, "source-fence") > 0) {
                            mark_need("source-fence")
                        }
                        in_needs_block = (clean ~ /^    needs:[[:space:]]*$/)
                        return
                    }
                    if (in_needs_block && clean ~ /^    [A-Za-z0-9_-]+:/) {
                        in_needs_block = 0
                        return
                    }
                    if (in_needs_block && clean ~ /^[[:space:]]*-[[:space:]]*detector[[:space:]]*$/) {
                        mark_need("detector")
                    }
                    if (in_needs_block && clean ~ /^[[:space:]]*-[[:space:]]*source-fence[[:space:]]*$/) {
                        mark_need("source-fence")
                    }
                }

                function flush_job() {
                    if (current == "clippy" || current == "test" || current == "build" || current == "source-fence") {
                        if (!has_managed_target_dir) {
                            print current "|managed-target-dir"
                        }
                    }
                    if (current == "deny" || current == "test" || current == "clippy" || current == "build" || current == "source-fence") {
                        if (!has_cache_key) {
                            print current "|cache-key"
                        }
                    }
                    if (current == "test" && !has_test_source_fence_needs) {
                        print current "|test-source-fence-needs"
                    }
                    if (current == "source-fence" && !has_source_fence_detector_needs) {
                        print current "|source-fence-detector-needs"
                    }
                    if (current == "gate") {
                        if (!has_gate_always) {
                            print current "|gate-always"
                        }
                        if (!has_gate_source_fence_needs) {
                            print current "|gate-source-fence-needs"
                        }
                        if (!has_gate_detector_result) {
                            print current "|gate-detector-result"
                        }
                        if (!has_gate_fmt_result) {
                            print current "|gate-fmt-result"
                        }
                        if (!has_gate_deny_result) {
                            print current "|gate-deny-result"
                        }
                        if (!has_gate_clippy_result) {
                            print current "|gate-clippy-result"
                        }
                        if (!has_gate_test_result) {
                            print current "|gate-test-result"
                        }
                        if (!has_gate_source_fence_result) {
                            print current "|gate-source-fence-result"
                        }
                        if (!has_gate_build_result) {
                            print current "|gate-build-result"
                        }
                        if (!has_gate_build_required) {
                            print current "|gate-build-required"
                        }
                    }
                    if (current == "build" && !has_build_required_output) {
                        print current "|build-required-output"
                    }
                }

                /^jobs:/ {
                    in_jobs = 1
                    next
                }

                in_jobs && /^[^[:space:]]/ {
                    flush_job()
                    in_jobs = 0
                    current = ""
                    next
                }

                in_jobs && /^  [A-Za-z0-9_-]+:/ {
                    flush_job()
                    current = $0
                    sub(/^  /, "", current)
                    sub(/:.*/, "", current)
                    if (current == "source-fence") {
                        saw_source_fence = 1
                    }
                    has_source_fence_detector_needs = 0
                    has_managed_target_dir = 0
                    has_cache_key = 0
                    has_test_source_fence_needs = 0
                    has_gate_always = 0
                    has_gate_source_fence_needs = 0
                    has_gate_detector_result = 0
                    has_gate_fmt_result = 0
                    has_gate_deny_result = 0
                    has_gate_clippy_result = 0
                    has_gate_test_result = 0
                    has_gate_source_fence_result = 0
                    has_gate_build_result = 0
                    has_gate_build_required = 0
                    has_build_required_output = 0
                    in_needs_block = 0
                    next
                }

                current != "" {
                    if (index($0, managed_target_dir_output_literal) > 0) {
                        has_managed_target_dir = 1
                    }
                    if ($0 ~ /key:[[:space:]]*[[:graph:]]+/) {
                        has_cache_key = 1
                    }
                    scan_needs($0)
                    if (current == "gate") {
                        if ($0 ~ gate_if_always_pattern) {
                            has_gate_always = 1
                        }
                        if (index($0, "needs.detector.result") > 0) {
                            has_gate_detector_result = 1
                        }
                        if (index($0, "needs.fmt-check.result") > 0) {
                            has_gate_fmt_result = 1
                        }
                        if (index($0, "needs.deny.result") > 0) {
                            has_gate_deny_result = 1
                        }
                        if (index($0, "needs.clippy.result") > 0) {
                            has_gate_clippy_result = 1
                        }
                        if (index($0, "needs.test.result") > 0) {
                            has_gate_test_result = 1
                        }
                        if (index($0, "needs.source-fence.result") > 0) {
                            has_gate_source_fence_result = 1
                        }
                        if (index($0, "needs.build.result") > 0) {
                            has_gate_build_result = 1
                        }
                        if (index($0, "needs.detector.outputs.build_required") > 0) {
                            has_gate_build_required = 1
                        }
                    }
                    if (current == "build" && $0 ~ build_required_output_pattern) {
                        has_build_required_output = 1
                    }
                }

                END {
                    flush_job()
                    if (!saw_source_fence) {
                        print "source-fence|source-fence-job"
                    }
                }
            ' .github/workflows/ci.yml
        )
    fi

    for f in "${workflow_files[@]}"; do
        if grep -Eq "$repo_local_artifact_pattern" "$f"; then
            echo "ERROR: Repo-local build artifact path found in $f"
            failed=1
        fi

        while IFS= read -r job_name; do
            [ -n "$job_name" ] || continue
            echo "ERROR: Managed build artifact staging missing in $f job '$job_name'"
            failed=1
        done < <(
            awk -v binary_path_literal="$managed_binary_path_literal" '
                BEGIN {
                    in_jobs = 0
                    current = ""
                    has_binary_path = 0
                }

                function flush_job() {
                    if (current == "build" && !has_binary_path) {
                        print current
                    }
                }

                /^jobs:/ {
                    in_jobs = 1
                    next
                }

                in_jobs && /^[^[:space:]]/ {
                    flush_job()
                    in_jobs = 0
                    current = ""
                    has_binary_path = 0
                    next
                }

                in_jobs && /^  [A-Za-z0-9_-]+:/ {
                    flush_job()
                    current = $0
                    sub(/^  /, "", current)
                    sub(/:.*/, "", current)
                    has_binary_path = 0
                    next
                }

                current != "" && index($0, binary_path_literal) > 0 {
                    has_binary_path = 1
                }

                END {
                    flush_job()
                }
            ' "$f"
        )
    done

    for f in "${rust_invocation_files[@]}"; do
        if grep -En "$bypass_pattern" "$f"; then
            echo "ERROR: Rust wrapper bypass found in $f"
            failed=1
        fi
    done

    if [ "$toml_target" != "$just_target" ]; then
        echo "ERROR: justfile target ($just_target) does not match .claude/rust-verification.toml build target ($toml_target)"
        failed=1
    fi

    if [ "$toml_profile" != "$managed_build_profile" ]; then
        echo "ERROR: managed-build profile ($managed_build_profile) does not match .claude/rust-verification.toml build profile ($toml_profile)"
        failed=1
    fi

    if [ "$failed" -ne 0 ]; then
        echo "All tracked automation must avoid raw cargo workflow commands, explicit Rust-wrapper bypasses, CI setup action drift, repo-local managed build artifact paths, and justfile/TOML build drift."
        exit 1
    fi

    if [ "${#github_automation_files[@]}" -eq 0 ]; then
        echo "OK: No workflow or action files found; automation-specific checks skipped"
    else
        echo "OK: No raw cargo workflow commands, explicit Rust-wrapper bypasses, CI setup action drift, or repo-local managed build artifact paths found"
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

setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Setting git hooks path..."
    git config core.hooksPath .githooks

    echo "Adding {{target}} target..."
    rustup target add {{target}}

    if command -v cargo-nextest >/dev/null 2>&1 && cargo-nextest --version | grep -Eq "^cargo-nextest {{nextest_version}}([[:space:]]|$)"; then
        echo "cargo-nextest {{nextest_version}} already installed"
    else
        echo "Installing cargo-nextest {{nextest_version}}..."
        cargo install cargo-nextest --version {{nextest_version}} --locked
    fi

    if command -v cargo-deny >/dev/null 2>&1 && cargo-deny --version | grep -Eq "^cargo-deny {{deny_version}}([[:space:]]|$)"; then
        echo "cargo-deny {{deny_version}} already installed"
    else
        echo "Installing cargo-deny {{deny_version}}..."
        cargo install cargo-deny --version {{deny_version}} --locked
    fi

    if command -v cargo-zigbuild >/dev/null 2>&1 && cargo-zigbuild --version | grep -Eq "^cargo-zigbuild {{zigbuild_version}}([[:space:]]|$)"; then
        echo "cargo-zigbuild {{zigbuild_version}} already installed"
    else
        echo "Installing cargo-zigbuild {{zigbuild_version}}..."
        cargo install cargo-zigbuild --version {{zigbuild_version}} --locked
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
