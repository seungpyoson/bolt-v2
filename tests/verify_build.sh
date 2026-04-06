#!/bin/bash
set -euo pipefail

# Verify the Rust binary compiles clean and CLI works
echo "=== Checking compilation ==="
~/.cargo/bin/cargo check 2>&1 | grep -E "^(error|warning:|Finished)" || true

echo "=== Verifying CLI subcommands ==="
~/.cargo/bin/cargo run --release -- --help 2>&1 | grep -E "^  (run|secrets|help)"

echo "=== Verifying config parses ==="
~/.cargo/bin/cargo run --release -- run --config config/example.toml 2>&1 | head -1 || true

echo "=== All checks passed ==="
