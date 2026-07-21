set shell := ["bash", "-euo", "pipefail", "-c"]

# bolt-v2 build commands — single source of truth for pinned tool versions and
# the build target. CI runs the same raw cargo commands these recipes run; a
# red CI check is reproduced locally by running the identical cargo command.

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
bte_root := repo_root + "/crates/backtesting-vertical-slice"

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
require-live-profile:
    #!/usr/bin/env bash
    if [ -z "${BOLT_LIVE_PROFILE:-}" ]; then
        echo "ERROR: set BOLT_LIVE_PROFILE to an opaque profile ID"
        exit 2
    fi

# Repository-wide formatting (root + BTE workspaces).
fmt: check-workspace
    cargo fmt
    cargo fmt --manifest-path "{{bte_root}}/Cargo.toml"

clippy: check-workspace
    cargo clippy --locked -- -D warnings

test *args: check-workspace
    cargo nextest run --locked {{args}}

test-archive archive *args: check-workspace
    cargo nextest archive --locked --archive-file "{{archive}}" {{args}}

test-archive-run archive extract_root *args: check-workspace
    cargo nextest run --archive-file "{{archive}}" --extract-to "{{extract_root}}" --extract-overwrite --workspace-remap "{{repo_root}}" {{args}}

build: check-workspace
    cargo zigbuild --release --target {{target}} --locked

# backtesting-vertical-slice crate (separate workspace at crates/backtesting-vertical-slice/).
# Its build target pin lives in that crate's own justfile.
bte-clippy: check-workspace
    cd "{{bte_root}}" && just clippy

bte-test *args: check-workspace
    cd "{{bte_root}}" && just test {{args}}

bte-test-archive archive *args: check-workspace
    archive_path="{{archive}}"; \
      case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac; \
      mkdir -p "$(dirname "$archive_path")"; \
      cd "{{bte_root}}" && cargo nextest archive --locked --archive-file "$archive_path" {{args}}

bte-test-archive-run archive extract_root *args: check-workspace
    archive_path="{{archive}}"; \
      case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac; \
      cd "{{bte_root}}" && cargo nextest run --archive-file "$archive_path" --extract-to "{{extract_root}}" --extract-overwrite --workspace-remap "{{bte_root}}" {{args}}

bte-build: check-workspace
    cd "{{bte_root}}" && just build

check-aarch64: check-workspace
    cargo check --target {{target}} --locked

# Submit explicit pull requests to Mergify after validating the complete list.
[positional-arguments]
merge-queue *pr_numbers:
    #!/usr/bin/env bash
    set -euo pipefail

    if (( $# == 0 )); then
        echo "ERROR: provide one or more pull request numbers" >&2
        exit 2
    fi

    pr_numbers=()
    for candidate in "$@"; do
        if [[ ! "$candidate" =~ ^[1-9][0-9]*$ ]]; then
            echo "ERROR: invalid pull request number: $candidate" >&2
            exit 2
        fi
        if (( ${#pr_numbers[@]} > 0 )); then
            for existing in "${pr_numbers[@]}"; do
                if [[ "$candidate" == "$existing" ]]; then
                    echo "ERROR: duplicate pull request number: $candidate" >&2
                    exit 2
                fi
            done
        fi
        pr_numbers+=("$candidate")
    done

    if ! origin_url="$(git remote get-url origin)"; then
        echo "ERROR: could not resolve the origin remote" >&2
        exit 2
    fi
    if ! queue_repository="$(gh repo view "$origin_url" --json url --jq '.url | ltrimstr("https://")')"; then
        echo "ERROR: could not resolve the queue repository from origin" >&2
        exit 2
    fi
    validation_failed=0
    for pr_number in "${pr_numbers[@]}"; do
        if ! metadata="$(gh pr view "$pr_number" --repo "$queue_repository" --json number,state,baseRefName --jq '[.number, .state, .baseRefName] | @tsv')"; then
            echo "ERROR: could not confirm pull request #$pr_number" >&2
            validation_failed=1
            continue
        fi

        IFS=$'\t' read -r returned_number state base_ref <<< "$metadata"
        if [[ "$returned_number" != "$pr_number" ]]; then
            echo "ERROR: pull request lookup mismatch for #$pr_number" >&2
            validation_failed=1
        elif [[ "$state" != "OPEN" ]]; then
            echo "ERROR: pull request #$pr_number is not open" >&2
            validation_failed=1
        elif [[ "$base_ref" != "main" ]]; then
            echo "ERROR: pull request #$pr_number targets $base_ref, not main" >&2
            validation_failed=1
        fi
    done

    if (( validation_failed != 0 )); then
        echo "No queue requests were submitted." >&2
        exit 2
    fi

    submitted=()
    for (( index=0; index<${#pr_numbers[@]}; index++ )); do
        pr_number="${pr_numbers[$index]}"
        if gh pr comment "$pr_number" --repo "$queue_repository" --body '@mergifyio queue'; then
            submitted+=("$pr_number")
            continue
        fi

        not_attempted=("${pr_numbers[@]:$((index + 1))}")
        if (( ${#submitted[@]} == 0 )); then
            echo "Confirmed submitted: none" >&2
        else
            echo "Confirmed submitted: ${submitted[*]}" >&2
        fi
        echo "Submission outcome unknown: $pr_number" >&2
        if (( ${#not_attempted[@]} == 0 )); then
            echo "Not attempted: none" >&2
        else
            echo "Not attempted: ${not_attempted[*]}" >&2
        fi
        exit 1
    done

    echo "Submitted queue requests: ${submitted[*]}"

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
live-generate: check-workspace require-live-profile
    cargo run --locked --release --bin bolt-v2 -- ops generate-live-config --profile "{{live_profile}}" --config-root config

# Prove a deployed runtime config regenerates from the tracked profile and still
# loads against this exact binary (byte parity + independent schema load).
live-verify: check-workspace require-live-profile
    cargo run --locked --release --bin bolt-v2 -- ops verify-live-config --profile "{{live_profile}}" --config-root config

# Canonical repo-local operator lane for bolt-v2 from this checkout.
live: live-generate
    cargo run --locked --release --bin bolt-v2 -- ops launch --profile "{{live_profile}}" --config-root config

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
