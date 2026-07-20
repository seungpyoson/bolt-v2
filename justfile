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
require-live-profile:
    #!/usr/bin/env bash
    if [ -z "${BOLT_LIVE_PROFILE:-}" ]; then
        echo "ERROR: set BOLT_LIVE_PROFILE to an opaque profile ID"
        exit 2
    fi

# Repository-wide formatting through the managed Rust wrapper (root + BTE workspaces).
fmt: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fmt
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- fmt

[private]
managed-clippy: check-workspace
    if [ "${BOLT_MANAGED_JUST:-}" != "1" ]; then echo "ERROR: managed-clippy must run through scripts/rust_verification.py run"; exit 2; fi
    cargo clippy --locked -- -D warnings

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
# Routed through the same managed wrapper as bolt-v2; --repo selects the crate's policy,
# Justfile, and cache namespace.
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

rust-probe *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" rust-probe --repo "{{repo_root}}" {{args}}

# Render the systemd unit from deploy/install-layout.env + the .in template. The
# committed deploy/systemd/bolt-v2.service is a GENERATED artifact — edit the template
# or layout and regenerate; never hand-edit the unit. Drift is caught by the
# deploy_systemd Rust integration tests in the platform_config harness.
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

# cache-prune: age-only managed Rust target sweep across root + BTE caches. Default = dry-run; pass --apply to execute.
cache-prune *args:
    python3 "{{rust_verification_owner}}" cache-prune --repo "{{repo_root}}" --repo "{{repo_root}}/crates/backtesting-vertical-slice" --age-only --json {{args}}

setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Setting git hooks path..."
    git config core.hooksPath .githooks

    echo "Asserting machine-global Cargo target dir..."
    python3 "{{rust_verification_owner}}" assert-global-cargo-target-dir --repo "{{repo_root}}"

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
