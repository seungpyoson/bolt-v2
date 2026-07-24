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

_test-merge-queue:
    bash scripts/test_merge_queue.sh

test *args: check-workspace _test-merge-queue
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
    if ! repository_metadata="$(gh repo view "$origin_url" --json url,nameWithOwner,defaultBranchRef --jq '[(.url | ltrimstr("https://")), .nameWithOwner, .defaultBranchRef.name] | @tsv')"; then
        echo "ERROR: could not resolve the queue repository from origin" >&2
        exit 2
    fi
    IFS=$'\t' read -r queue_repository queue_repository_name default_branch <<< "$repository_metadata"
    if [[ -z "$queue_repository" || -z "$queue_repository_name" || -z "$default_branch" ]]; then
        echo "ERROR: GitHub returned incomplete queue repository metadata" >&2
        exit 2
    fi

    contains_pr() {
        local needle="$1"
        shift
        local item
        for item in "$@"; do
            if [[ "$item" == "$needle" ]]; then
                return 0
            fi
        done
        return 1
    }

    reject_chain() {
        echo "ERROR: $1" >&2
        validation_failed=1
    }

    claim_chain_member() {
        local current="$1"
        local requested="$2"
        local overlap_target=""
        local index

        if (( ${#chain_prs[@]} > 0 )) && contains_pr "$current" "${chain_prs[@]}"; then
            reject_chain "pull request #$requested has a dependency cycle at #$current"
            return 1
        fi

        for (( index=0; index<${#claimed_prs[@]}; index++ )); do
            if [[ "${claimed_prs[$index]}" == "$current" ]]; then
                overlap_target="${claimed_targets[$index]}"
                break
            fi
        done
        if [[ -n "$overlap_target" ]]; then
            reject_chain "requested pull request chains #$overlap_target and #$requested overlap at #$current"
            return 1
        fi

        chain_prs+=("$current")
        if (( ${#chain_prs[@]} > max_stack_depth )); then
            reject_chain "pull request #$requested exceeds Mergify's maximum stack depth of $max_stack_depth"
            return 1
        fi
    }

    load_pull_metadata() {
        local current="$1"
        local metadata

        if ! metadata="$(gh pr view "$current" --repo "$queue_repository" \
            --json number,state,isDraft,baseRefName,headRefName,headRepository,body \
            --jq '
                def strip_terminal_cr:
                    if endswith("\r") then .[0:-1] else . end;
                def stack_dependency:
                    "Depends-On:" as $label
                    | ($label + " #") as $prefix
                    | ((.body // "") | split("\n") | map(strip_terminal_cr)
                       | map(select(startswith($label)))) as $markers
                    | if ($markers | length) != 1 then "invalid"
                      elif ($markers[0] | startswith($prefix) | not) then "invalid"
                      else $markers[0][($prefix | length):] as $suffix
                      | ($suffix | explode) as $digits
                      | if (($digits | length) > 0
                            and $digits[0] >= 49
                            and $digits[0] <= 57
                            and ($digits | all(. >= 48 and . <= 57)))
                        then "valid:" + $suffix
                        else "invalid"
                        end
                      end;
                [.number, .state, .isDraft, .baseRefName, .headRefName,
                 (.headRepository.nameWithOwner // ""), stack_dependency] | @tsv
            ')"; then
            reject_chain "could not confirm pull request #$current"
            return 1
        fi

        IFS=$'\t' read -r returned_number state is_draft base_ref head_ref head_repository dependency <<< "$metadata"
    }

    validate_pull_metadata() {
        local current="$1"
        local dependent="$2"
        local expected="$3"

        case "$returned_number:$state:$is_draft" in
            "$current:OPEN:false") ;;
            "$current:OPEN:"*)
                reject_chain "pull request #$current is a draft"
                return 1
                ;;
            "$current:"*)
                reject_chain "pull request #$current is not open"
                return 1
                ;;
            *)
                reject_chain "pull request lookup mismatch for #$current"
                return 1
                ;;
        esac
        if [[ -n "$expected" && ( "$head_repository" != "$queue_repository_name" || "$head_ref" != "$expected" ) ]]; then
            reject_chain "pull request #$current head $head_repository:$head_ref does not match pull request #$dependent base $expected"
            return 1
        fi
    }

    max_stack_depth=20
    claimed_prs=()
    claimed_targets=()
    validation_failed=0
    for requested_pr in "${pr_numbers[@]}"; do
        chain_prs=()
        current_pr="$requested_pr"
        dependent_pr=""
        expected_head=""
        chain_complete=0

        while :; do
            claim_chain_member "$current_pr" "$requested_pr" || break
            load_pull_metadata "$current_pr" || break
            validate_pull_metadata "$current_pr" "$dependent_pr" "$expected_head" || break

            if [[ "$base_ref" == "$default_branch" ]]; then
                chain_complete=1
                break
            fi
            if [[ "$dependency" != valid:* ]]; then
                reject_chain "pull request #$current_pr lacks one exact Depends-On: #<number> marker; run mergify stack push"
                break
            fi

            dependent_pr="$current_pr"
            expected_head="$base_ref"
            current_pr="${dependency#valid:}"
        done

        if (( chain_complete != 0 && ${#chain_prs[@]} > 1 )); then
            bottom_index=$(( ${#chain_prs[@]} - 1 ))
            bottom_pr="${chain_prs[$bottom_index]}"
            reject_chain "pull request #$requested_pr has open dependencies; queue bottom pull request #$bottom_pr first, then sync and reapprove each successor"
        fi

        if (( ${#chain_prs[@]} > 0 )); then
            for current_pr in "${chain_prs[@]}"; do
                claimed_prs+=("$current_pr")
                claimed_targets+=("$requested_pr")
            done
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
