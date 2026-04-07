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

fmt-check: check-workspace
    cargo fmt --check

fmt: check-workspace
    cargo fmt

deny: check-workspace
    cargo deny check bans

deny-advisories: check-workspace
    cargo deny check advisories

clippy: check-workspace
    cargo clippy --locked -- -D warnings

test: check-workspace
    cargo nextest run --locked

build: check-workspace
    cargo zigbuild --release --target {{target}} --locked

live-generate: check-workspace
    #!/usr/bin/env bash
    if [ ! -f "{{live_input}}" ]; then
        echo "Missing {{live_input}}"
        echo "Create it from {{live_input_example}}, then rerun."
        exit 1
    fi

    cargo run --quiet --bin render_live_config -- --input {{live_input}} --output {{live_config}}

# Canonical repo-local operator lane for bolt-v2 from this checkout.
live: live-generate
    cargo run --release --bin bolt-v2 -- run --config {{live_config}}

# Optional diagnostics for the live operator config.
live-check: live-generate
    cargo run --release --bin bolt-v2 -- secrets check --config {{live_config}}

live-resolve: live-generate
    cargo run --release --bin bolt-v2 -- secrets resolve --config {{live_config}}

ci-lint-workflow:
    #!/usr/bin/env bash
    shopt -s nullglob
    files=(.github/workflows/*.yml .github/workflows/*.yaml)

    if [ "${#files[@]}" -eq 0 ]; then
        echo "No workflow files found — skipping"
        exit 0
    fi

    failed=0
    pattern='\bcargo[[:space:]]+(fmt|clippy|test|nextest|zigbuild|deny|audit|build|check)\b'

    for f in "${files[@]}"; do
        if rg -n -e "$pattern" "$f" | rg -v '\bcargo[[:space:]]+install\b'; then
            echo "ERROR: Raw cargo commands found in $f"
            failed=1
        fi
    done

    if [ "$failed" -ne 0 ]; then
        echo "All build/check commands must go through justfile recipes."
        exit 1
    fi

    echo "OK: No raw cargo build/check commands in workflow files"

worktree branch:
    #!/usr/bin/env bash
    dest="{{worktree_root}}/{{branch}}"
    mkdir -p "$(dirname "$dest")"

    if git show-ref --verify --quiet "refs/heads/{{branch}}"; then
        git worktree add "$dest" "{{branch}}"
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
    echo "Setting git hooks path..."
    git config core.hooksPath .githooks

    echo "Adding {{target}} target..."
    rustup target add {{target}}

    if cargo-nextest --version | rg -q "^cargo-nextest {{nextest_version}}\\b"; then
        echo "cargo-nextest {{nextest_version}} already installed"
    else
        echo "Installing cargo-nextest {{nextest_version}}..."
        cargo install cargo-nextest --version {{nextest_version}} --locked
    fi

    if cargo-deny --version | rg -q "^cargo-deny {{deny_version}}\\b"; then
        echo "cargo-deny {{deny_version}} already installed"
    else
        echo "Installing cargo-deny {{deny_version}}..."
        cargo install cargo-deny --version {{deny_version}} --locked
    fi

    if cargo-zigbuild --version | rg -q "^cargo-zigbuild {{zigbuild_version}}\\b"; then
        echo "cargo-zigbuild {{zigbuild_version}} already installed"
    else
        echo "Installing cargo-zigbuild {{zigbuild_version}}..."
        cargo install cargo-zigbuild --version {{zigbuild_version}} --locked
    fi

    if command -v zig >/dev/null 2>&1 && [ "$(zig version)" = "{{zig_version}}" ]; then
        echo "Zig {{zig_version}} already installed"
    else
        echo "Zig {{zig_version}} required; install it locally with 'brew install zig'"
    fi

    echo "Setup complete."
