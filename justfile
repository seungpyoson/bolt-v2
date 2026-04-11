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

fmt-check: check-workspace require-rust-verification-owner
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
    workflow_files=(.github/workflows/*.yml .github/workflows/*.yaml)
    action_files=(.github/actions/*/action.yml .github/actions/*/action.yaml)
    github_automation_files=("${workflow_files[@]}" "${action_files[@]}")
    rust_invocation_files=(justfile scripts/*.sh tests/*.sh "${github_automation_files[@]}")

    if [ "${#github_automation_files[@]}" -eq 0 ]; then
        echo "No workflow or action files found — skipping"
    fi

    failed=0
    pattern='(^|[^[:alnum:]_])cargo[[:space:]]+(fmt|clippy|test|nextest|zigbuild|deny|audit|build|check)([^[:alnum:]_]|$)'
    bypass_pattern='(^|[^[:alnum:]_./-])(command[[:space:]]+cargo|~\/\.cargo\/bin\/cargo|\/[^[:space:]]*\/\.cargo\/bin\/cargo)([^[:alnum:]_./-]|$)'
    just_lane_pattern='(^|[^[:alnum:]_./-])just[[:space:]]+(fmt-check|deny|deny-advisories|clippy|test|build)([^[:alnum:]_]|$)'
    setup_action_literal='uses: ./.github/actions/setup-environment'
    setup_token_literal='claude-config-read-token:'
    setup_just_version_literal='just-version:'
    managed_binary_path_literal='binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'
    repo_local_artifact_pattern='(^|[^[:alnum:]_./-])target/.*/release/bolt-v2(\.sha256)?([^[:alnum:]_./-]|$)'
    just_target='{{target}}'
    managed_build_profile='release'
    toml_target="$(python3 -c "import pathlib, tomllib; print(tomllib.load(pathlib.Path('.claude/rust-verification.toml').open('rb'))['commands']['build']['target'])")"
    toml_profile="$(python3 -c "import pathlib, tomllib; print(tomllib.load(pathlib.Path('.claude/rust-verification.toml').open('rb'))['commands']['build']['profile'])")"
    action_file='.github/actions/setup-environment/action.yml'
    action_required_literals=(
        "inputs.just-version"
        "CLAUDE_CONFIG_READ_TOKEN:"
        "inputs.claude-config-read-token"
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
                setup-just-version)
                    echo "ERROR: Managed CI just version wiring missing in $f job '$job_name'"
                    ;;
            esac
            failed=1
        done < <(
            awk -v lane_pattern="$just_lane_pattern" \
                -v setup_action_literal="$setup_action_literal" \
                -v setup_token_literal="$setup_token_literal" \
                -v setup_just_version_literal="$setup_just_version_literal" '
                BEGIN {
                    in_jobs = 0
                    current = ""
                    has_lane = 0
                    has_setup_step = 0
                    has_setup_token = 0
                    has_setup_just_version = 0
                    step_has_setup = 0
                    step_has_token = 0
                    step_has_just_version = 0
                }

                function flush_step() {
                    if (step_has_setup) {
                        has_setup_step = 1
                        if (step_has_token) {
                            has_setup_token = 1
                        }
                        if (step_has_just_version) {
                            has_setup_just_version = 1
                        }
                    }
                    step_has_setup = 0
                    step_has_token = 0
                    step_has_just_version = 0
                }

                function flush_job() {
                    flush_step()
                    if (current == "" || !has_lane) {
                        return
                    }
                    if (!has_setup_step) {
                        print current "|setup-action"
                    }
                    if (has_setup_step && !has_setup_token) {
                        print current "|setup-token"
                    }
                    if (has_setup_step && !has_setup_just_version) {
                        print current "|setup-just-version"
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
                    has_setup_token = 0
                    has_setup_just_version = 0
                    step_has_setup = 0
                    step_has_token = 0
                    step_has_just_version = 0
                    next
                }

                in_jobs && /^  [A-Za-z0-9_-]+:/ {
                    flush_job()
                    current = $0
                    sub(/^  /, "", current)
                    sub(/:.*/, "", current)
                    has_lane = 0
                    has_setup_step = 0
                    has_setup_token = 0
                    has_setup_just_version = 0
                    step_has_setup = 0
                    step_has_token = 0
                    step_has_just_version = 0
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
                    if (index($0, setup_token_literal) > 0) {
                        step_has_token = 1
                    }
                    if (index($0, setup_just_version_literal) > 0) {
                        step_has_just_version = 1
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
        for literal in "${action_required_literals[@]}"; do
            if ! grep -Fq "$literal" "$action_file"; then
                echo "ERROR: Managed CI setup action missing expected literal '$literal'"
                failed=1
            fi
        done
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
