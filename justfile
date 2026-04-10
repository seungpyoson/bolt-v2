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
live: live-generate require-rust-verification-owner
    # Run with the generated runtime config artifact.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- run --config {{live_config}}

# Optional diagnostics for the live operator config.
live-check: live-generate require-rust-verification-owner
    # Validate secret-config completeness only; do not resolve secrets.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- secrets check --config {{live_config}}

live-resolve: live-generate require-rust-verification-owner
    # Perform actual secret resolution against the generated runtime config.
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --release --bin bolt-v2 -- secrets resolve --config {{live_config}}

ci-lint-workflow:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    files=(.github/workflows/*.yml .github/workflows/*.yaml)
    rust_invocation_files=(justfile scripts/*.sh tests/*.sh .github/workflows/*.yml .github/workflows/*.yaml)

    if [ "${#files[@]}" -eq 0 ]; then
        echo "No workflow files found — skipping"
    fi

    failed=0
    pattern='(^|[^[:alnum:]_])cargo[[:space:]]+(fmt|clippy|test|nextest|zigbuild|deny|audit|build|check)([^[:alnum:]_]|$)'
    bypass_pattern='(^|[^[:alnum:]_./-])(command[[:space:]]+cargo|~\/\.cargo\/bin\/cargo|\/[^[:space:]]*\/\.cargo\/bin\/cargo)([^[:alnum:]_./-]|$)'
    just_lane_pattern='(^|[^[:alnum:]_./-])just[[:space:]]+(fmt-check|deny|deny-advisories|clippy|test|build)([^[:alnum:]_]|$)'
    owner_bootstrap_literal='steps.shared.outputs.rust_verification_ci_install_script'
    owner_repo_eval_literal='just --evaluate rust_verification_source_repo'
    owner_sha_eval_literal='just --evaluate rust_verification_source_sha'
    owner_install_eval_literal='just --evaluate rust_verification_ci_install_script'
    managed_binary_path_literal='binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'
    repo_local_artifact_pattern='(^|[^[:alnum:]_./-])target/.*/release/bolt-v2(\.sha256)?([^[:alnum:]_./-]|$)'

    for f in "${files[@]}"; do
        if grep -En "$pattern" "$f"; then
            echo "ERROR: Raw cargo commands found in $f"
            failed=1
        fi
    done

    for f in "${files[@]}"; do
        while IFS='|' read -r job_name reason; do
            [ -n "$job_name" ] || continue
            case "$reason" in
                bootstrap)
                    echo "ERROR: Managed Rust owner bootstrap missing in $f job '$job_name'"
                    ;;
                pin-repo)
                    echo "ERROR: Managed Rust owner repo pin must come from justfile in $f job '$job_name'"
                    ;;
                pin-sha)
                    echo "ERROR: Managed Rust owner SHA pin must come from justfile in $f job '$job_name'"
                    ;;
                install-eval)
                    echo "ERROR: Managed Rust owner install script must come from justfile in $f job '$job_name'"
                    ;;
            esac
            failed=1
        done < <(
            awk -v lane_pattern="$just_lane_pattern" \
                -v bootstrap_literal="$owner_bootstrap_literal" \
                -v repo_eval_literal="$owner_repo_eval_literal" \
                -v sha_eval_literal="$owner_sha_eval_literal" \
                -v install_eval_literal="$owner_install_eval_literal" '
                BEGIN {
                    in_jobs = 0
                    current = ""
                    has_lane = 0
                    has_bootstrap = 0
                    has_repo_eval = 0
                    has_sha_eval = 0
                    has_install_eval = 0
                }

                function flush_job() {
                    if (current == "" || !has_lane) {
                        return
                    }
                    if (!has_bootstrap) {
                        print current "|bootstrap"
                    }
                    if (!has_repo_eval) {
                        print current "|pin-repo"
                    }
                    if (!has_sha_eval) {
                        print current "|pin-sha"
                    }
                    if (!has_install_eval) {
                        print current "|install-eval"
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
                    has_bootstrap = 0
                    next
                }

                in_jobs && /^  [A-Za-z0-9_-]+:/ {
                    flush_job()
                    current = $0
                    sub(/^  /, "", current)
                    sub(/:.*/, "", current)
                    has_lane = 0
                    has_bootstrap = 0
                    has_repo_eval = 0
                    has_sha_eval = 0
                    has_install_eval = 0
                    next
                }

                current != "" {
                    if ($0 ~ lane_pattern) {
                        has_lane = 1
                    }
                    if (index($0, bootstrap_literal) > 0) {
                        has_bootstrap = 1
                    }
                    if (index($0, repo_eval_literal) > 0) {
                        has_repo_eval = 1
                    }
                    if (index($0, sha_eval_literal) > 0) {
                        has_sha_eval = 1
                    }
                    if (index($0, install_eval_literal) > 0) {
                        has_install_eval = 1
                    }
                }

                END {
                    flush_job()
                }
            ' "$f"
        )
    done

    for f in "${files[@]}"; do
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

    if [ "$failed" -ne 0 ]; then
        echo "All tracked automation must avoid raw cargo workflow commands, explicit Rust-wrapper bypasses, drifted CI owner pins, and repo-local managed build artifact paths."
        exit 1
    fi

    echo "OK: No raw cargo workflow commands, explicit Rust-wrapper bypasses, drifted CI owner pins, or repo-local managed build artifact paths found"

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
