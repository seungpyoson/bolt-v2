set shell := ["bash", "-euo", "pipefail", "-c"]

# bolt-v2 build commands — single source of truth.
# CI and local both call these recipes. No raw cargo build/check commands in workflow YAML.

nextest_version := "0.9.132"
deny_version := "0.19.0"
zigbuild_version := "0.22.1"
zigbuild_x86_64_unknown_linux_gnu_sha256 := "21e18a5f8ae64b9ed34c5c1cf7bba5af3bd96d77fd43d713eae85b922506d941"
zig_version := "0.15.2"

target := "aarch64-unknown-linux-gnu"
worktree_root := env_var('HOME') + "/worktrees/bolt-v2"
live_root := "config/live.local.toml"
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

verify-bolt-v3-legacy-default-fence: check-workspace
    python3 scripts/test_verify_bolt_v3_legacy_default_fence.py
    python3 scripts/verify_bolt_v3_legacy_default_fence.py

verify-bolt-v3-strategy-policy-fence: check-workspace
    python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/verify_bolt_v3_strategy_policy_fence.py

verify-bolt-v3-dependency-direction: check-workspace
    python3 scripts/test_verify_bolt_v3_dependency_direction.py
    python3 scripts/verify_bolt_v3_dependency_direction.py

test-verify-runtime-capture-yaml: check-workspace
    python3 scripts/test_verify_runtime_capture_yaml.py

verify-runtime-capture-yaml: test-verify-runtime-capture-yaml
    python3 scripts/verify_runtime_capture_yaml.py

fmt-check: check-workspace require-rust-verification-owner verify-bolt-v3-runtime-literals verify-bolt-v3-provider-leaks
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
    python3 scripts/test_verify_bolt_v3_dependency_direction.py
    python3 scripts/verify_bolt_v3_dependency_direction.py
    python3 scripts/test_verify_bolt_v3_status_map_current.py
    python3 scripts/verify_bolt_v3_status_map_current.py
    python3 scripts/test_verify_bolt_v3_schema_current.py
    python3 scripts/verify_bolt_v3_schema_current.py
    python3 scripts/test_verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/verify_bolt_v3_pure_rust_runtime.py
    python3 scripts/test_verify_bolt_v3_legacy_default_fence.py
    python3 scripts/verify_bolt_v3_legacy_default_fence.py
    python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/test_verify_runtime_capture_yaml.py
    # Fresh CI runners need the pinned NT checkout before source-capture checks.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fetch --locked
    python3 scripts/verify_runtime_capture_yaml.py
    # #342 owns these canonical source-fence checks. Until #332 changes full
    # nextest ownership, `test` intentionally still duplicates them under `gate`.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- test --locked --test bolt_v3_controlled_connect --test bolt_v3_production_entrypoint -- --nocapture

require-live-root: check-workspace
    #!/usr/bin/env bash
    if [ ! -f "{{live_root}}" ]; then
        echo "Missing {{live_root}}"
        echo "Refresh the ignored operator root before running live commands."
        exit 1
    fi

# Canonical repo-local operator lane for bolt-v2 from this checkout.
live: require-live-root require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- run --config {{live_root}}

# Optional diagnostics for the live operator config.
live-check: require-live-root require-rust-verification-owner
    # Validate secret-config completeness only; do not resolve secrets.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- secrets check --config {{live_root}}

live-resolve: require-live-root require-rust-verification-owner
    # Perform actual secret resolution against the bolt-v3 root config.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- secrets resolve --config {{live_root}}

ci-lint-workflow:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    workflow_files=()
    action_files=()

    [ -f .github/workflows/ci.yml ] && workflow_files+=(.github/workflows/ci.yml)
    [ -f .github/workflows/ci-docs-pass-stub.yml ] && workflow_files+=(.github/workflows/ci-docs-pass-stub.yml)
    [ -f .github/workflows/advisory.yml ] && workflow_files+=(.github/workflows/advisory.yml)
    [ -f .github/actions/setup-environment/action.yml ] && action_files+=(.github/actions/setup-environment/action.yml)

    github_automation_files=("${workflow_files[@]}" "${action_files[@]}")
    rust_invocation_files=(justfile scripts/*.sh tests/*.sh "${github_automation_files[@]}")

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
    if ! python3 scripts/test_find_same_sha_main_evidence.py; then
        failed=1
    fi
    if ! python3 scripts/test_verify_ci_path_filters.py; then
        failed=1
    fi
    if ! python3 scripts/test_rust_verification.py; then
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
