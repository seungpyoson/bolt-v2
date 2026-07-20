#!/bin/bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

echo "=== Checking compilation ==="
cargo check --locked >/dev/null

echo "=== Verifying CLI subcommands ==="
cargo run --locked --release --bin bolt-v2 -- --help | grep -E "^  (run|secrets|help)"

tmpdir="$(mktemp -d)"
trap 'chmod -R u+w "$tmpdir" 2>/dev/null || true; rm -rf "$tmpdir"' EXIT

echo "=== Verifying bolt-v3 root secret config completeness ==="
cargo run --locked --release --bin bolt-v2 -- secrets check --config tests/fixtures/bolt_v3/root.toml \
  | grep "clients.polymarket_main: required secret fields present"

echo "=== Verifying exec_tester purge gate ==="
if rg -ni -g '!tests/verify_build.sh' "exec_tester|nautilus-testkit|nautilus_testkit::testers" -- \
  Cargo.toml Cargo.lock src tests config; then
    echo "ERROR: exec_tester purge gate matched forbidden references"
    exit 1
fi

echo "=== Verifying Gamma fee-field gate ==="
if rg -n -g '!tests/verify_build.sh' "maker_base_fee|taker_base_fee" -- \
  Cargo.toml Cargo.lock src tests config; then
    echo "ERROR: Gamma fee-field gate matched forbidden raw fee fields"
    exit 1
fi
