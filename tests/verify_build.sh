#!/bin/bash
set -euo pipefail

echo "=== Checking compilation ==="
~/.cargo/bin/cargo check >/dev/null

echo "=== Verifying CLI subcommands ==="
~/.cargo/bin/cargo run --release -- --help | grep -E "^  (run|secrets|help)"

echo "=== Verifying secret config completeness ==="
~/.cargo/bin/cargo run --release -- secrets check --config config/examples/polymarket-exec-tester.toml | grep "POLYMARKET: secret config complete"
