#!/bin/bash
set -euo pipefail

echo "=== Checking compilation ==="
~/.cargo/bin/cargo check >/dev/null

echo "=== Verifying CLI subcommands ==="
~/.cargo/bin/cargo run --release --bin bolt-v2 -- --help | grep -E "^  (run|secrets|help)"

tmpdir="$(mktemp -d)"
trap 'chmod -R u+w "$tmpdir" 2>/dev/null || true; rm -rf "$tmpdir"' EXIT

echo "=== Rendering generated live config from tracked example input ==="
~/.cargo/bin/cargo run --release --bin render_live_config -- \
  --input config/live.local.example.toml \
  --output "$tmpdir/live.toml" \
  | grep "Generated"

echo "=== Verifying generated config is read-only ==="
if [ -w "$tmpdir/live.toml" ]; then
    echo "ERROR: generated live config should be read-only"
    exit 1
fi

echo "=== Verifying secret config completeness ==="
~/.cargo/bin/cargo run --release --bin bolt-v2 -- secrets check --config "$tmpdir/live.toml" | grep "POLYMARKET: secret config complete"
