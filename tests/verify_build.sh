#!/bin/bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
rust_verification_owner="${HOME}/.claude/lib/rust_verification.py"
rust_verification_source_repo="$(just -f "$repo_root/justfile" --evaluate rust_verification_source_repo 2>/dev/null || true)"
rust_verification_source_sha="$(just -f "$repo_root/justfile" --evaluate rust_verification_source_sha 2>/dev/null || true)"

if [ -n "$rust_verification_source_repo" ] && [ -n "$rust_verification_source_sha" ]; then
    RUST_VERIFICATION_SOURCE_REPO="$rust_verification_source_repo" \
    RUST_VERIFICATION_SOURCE_SHA="$rust_verification_source_sha" \
    bash "$repo_root/scripts/require_rust_verification_owner.sh" "$rust_verification_owner"
else
    bash "$repo_root/scripts/require_rust_verification_owner.sh" "$rust_verification_owner"
fi

managed_cargo() {
    python3 "$rust_verification_owner" cargo --repo "$repo_root" -- "$@"
}

echo "=== Checking compilation ==="
managed_cargo check >/dev/null

echo "=== Verifying CLI subcommands ==="
managed_cargo run --release --bin bolt-v2 -- --help | grep -E "^  (run|secrets|help)"

tmpdir="$(mktemp -d)"
trap 'chmod -R u+w "$tmpdir" 2>/dev/null || true; rm -rf "$tmpdir"' EXIT

echo "=== Verifying bolt-v3 root secret config completeness ==="
managed_cargo run --release --bin bolt-v2 -- secrets check --config tests/fixtures/bolt_v3/root.toml \
  | grep "venues.polymarket_main: required secret fields present"

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
