#!/bin/bash
set -euo pipefail

echo "=== Checking compilation ==="
~/.cargo/bin/cargo check >/dev/null

echo "=== Verifying CLI subcommands ==="
~/.cargo/bin/cargo run --release -- --help | grep -E "^  (run|secrets|help)"

echo "=== Verifying secret reference detection in config ==="
~/.cargo/bin/cargo run --release -- secrets --config config/examples/polymarket-exec-tester.toml | grep "POLYMARKET: secret references found in config"
