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

verify-bolt-v3-no-exit-market-command: check-workspace
    python3 scripts/test_verify_bolt_v3_no_exit_market_command.py
    python3 scripts/verify_bolt_v3_no_exit_market_command.py

verify-bolt-v3-strategy-policy-fence: check-workspace
    python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py
    python3 scripts/verify_bolt_v3_strategy_policy_fence.py

[private]
fmt-workspace-check-inner workspace: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{workspace}}" -- fmt --check

[private]
deny-workspace-inner workspace: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{workspace}}" -- deny check bans

[private]
fmt-workspace-inner workspace: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{workspace}}" -- fmt

# Sole repository-wide local evidence command. This runs non-compile checks only.
preflight: check-workspace require-rust-verification-owner
    python3 scripts/local_verification_gate.py preflight -- just preflight-inner

[private]
preflight-inner: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 scripts/repo_preflight.py --governance "{{repo_root}}" --subject "{{repo_root}}"

fmt: check-workspace require-rust-verification-owner
    python3 scripts/repo_format.py --governance "{{repo_root}}" --subject "{{repo_root}}"

deny-advisories: check-workspace require-rust-verification-owner
    python3 scripts/workspace_advisories.py --governance "{{repo_root}}" --subject "{{repo_root}}"

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
# Justfile, and cache namespace. Local non-compile evidence is owned only by `just preflight`.
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

sandbox-safe-push: check-workspace
    python3 scripts/sandbox_safe_push.py

[positional-arguments]
merge-queue *args:
    python3 scripts/merge_queue_operator.py -- "$@"

rust-probe *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" rust-probe --repo "{{repo_root}}" {{args}}

ci-runner-minutes *args:
    python3 scripts/ubicloud_runner_minutes.py {{args}}

ci-storage-audit *args: check-workspace
    python3 scripts/ci_storage_audit.py {{args}}

ci-storage-tripwire *args: check-workspace
    python3 scripts/ci_storage_tripwire.py {{args}}

[private]
source-fence-static-inner subject='.': require-local-verification-gate check-workspace require-rust-verification-owner
    python3 scripts/run_fences.py --root "{{subject}}"

[private]
source-fence-static-fences-only-inner: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 scripts/run_fences.py --fences-only

# Cargo shim guard tests (pytest-based, unlike the self-running script tests)
cargo-shim-tests:
    python3 -m pytest scripts/test_cargo_shim.py -q

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

[private]
ci-lint-workflow-inner subject=repo_root: require-local-verification-gate check-workspace require-rust-verification-owner
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{subject}}"
    shopt -s nullglob
    workflow_files=(.github/workflows/*.yml .github/workflows/*.yaml)
    action_files=(.github/actions/*/action.yml .github/actions/*/action.yaml)
    github_script_files=()
    github_script_files=(.github/scripts/*.sh)

    github_automation_files=("${workflow_files[@]}" "${action_files[@]}" "${github_script_files[@]}")
    repo_governance_files=()
    [ -f .no-mistakes.yaml ] && repo_governance_files+=(.no-mistakes.yaml)
    rust_invocation_files=(justfile "${repo_governance_files[@]}" scripts/*.sh tests/*.sh "${github_automation_files[@]}")

    if [ "${#github_automation_files[@]}" -eq 0 ]; then
        echo "No workflow or action files found — skipping"
    fi

    actionlint "${workflow_files[@]}"

    failed=0
    pattern='(^|[^[:alnum:]_])cargo[[:space:]]+(audit|bench|build|check|clean|clippy|deny|doc|fetch|fmt|install|nextest|run|rustc|test|version|zigbuild)([^[:alnum:]_]|$)'
    bypass_pattern='(^|[^[:alnum:]_./-])(command[[:space:]]+cargo|~\/\.cargo\/bin\/cargo|\/[^[:space:]]*\/\.cargo\/bin\/cargo)([^[:alnum:]_./-]|$)'
    just_target='{{target}}'
    managed_build_profile='release'
    policy_json="$(python3 "{{rust_verification_owner}}" validate-policy --repo "{{repo_root}}")"
    toml_target="$(printf '%s\n' "$policy_json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["build_target"])')"
    toml_profile="$(printf '%s\n' "$policy_json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["build_profile"])')"
    if [ -n "${BOLT_CI_LINT_WORKFLOW_WORKERS:-}" ]; then
        set -- --workers "$BOLT_CI_LINT_WORKFLOW_WORKERS"
    else
        set --
    fi
    if ! python3 scripts/run_ci_lint_suites.py "$@"; then
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

# cache-prune: age-only managed Rust target sweep across root + BTE caches. Default = dry-run; pass --apply to execute.
cache-prune *args:
    python3 "{{rust_verification_owner}}" cache-prune --repo "{{repo_root}}" --repo "{{repo_root}}/crates/backtesting-vertical-slice" --age-only --json {{args}}

# clean-merged: print install/heartbeat/quarantine/gh health.
clean-merged-doctor:
    python3 scripts/clean_merged_artifacts.py --doctor

# clean-merged: post-merge-wave sync + one-time bulk reclaim of the worktree backlog.
# Prints a dry-run first; pass --apply to actually archive+remove.
clean-merged-backlog *args:
    python3 scripts/clean_merged_artifacts.py --sync-main --reconcile --include-worktrees {{args}}

# clean-merged: prune quarantine archives and backup refs older than DAYS (default 30).
clean-merged-purge days='30':
    python3 scripts/clean_merged_artifacts.py --purge-quarantine {{days}}
    python3 scripts/clean_merged_artifacts.py --prune-backups {{days}}

setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Installing generated git hooks..."
    python3 scripts/clean_merged_artifacts.py --install-hooks

    echo "Asserting machine-global Cargo target dir..."
    python3 "{{rust_verification_owner}}" assert-global-cargo-target-dir --repo "{{repo_root}}"

    clean_merged_remote="$(python3 scripts/clean_merged_artifacts.py --print-remote-name)"
    echo "Enabling remote.${clean_merged_remote}.prune (auto-prune deleted upstreams on fetch)..."
    git config "remote.${clean_merged_remote}.prune" true

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

    actionlint_version="$(python3 - <<'PY'
    import tomllib
    with open("ci/ai-review.toml", "rb") as handle:
        print(tomllib.load(handle)["final_review"]["actionlint_version"])
    PY
    )"
    if command -v actionlint >/dev/null 2>&1 && actionlint -version | grep -Eq "^${actionlint_version}([[:space:]]|$)"; then
        echo "actionlint ${actionlint_version} already installed"
    else
        echo "ERROR: actionlint ${actionlint_version} is required for just preflight"
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
