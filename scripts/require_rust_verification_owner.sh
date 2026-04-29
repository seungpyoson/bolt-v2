#!/usr/bin/env bash
set -euo pipefail

owner_path="${1:-${HOME}/.claude/lib/rust_verification.py}"

if [ -f "$owner_path" ]; then
    exit 0
fi

echo "ERROR: managed Rust owner not found at $owner_path" >&2
echo "This repo's tracked Rust automation fails closed when the owner is absent." >&2

if [ -n "${RUST_VERIFICATION_SOURCE_REPO:-}" ] && [ -n "${RUST_VERIFICATION_SOURCE_SHA:-}" ]; then
    echo "Pinned owner source: ${RUST_VERIFICATION_SOURCE_REPO}@${RUST_VERIFICATION_SOURCE_SHA}" >&2
    echo "CI bootstrap: bash scripts/install_ci_rust_verification_owner.sh \"${RUST_VERIFICATION_SOURCE_REPO}\" \"${RUST_VERIFICATION_SOURCE_SHA}\"" >&2
fi

echo "Local fix: install or update claude-config so ~/.claude/lib/rust_verification.py exists." >&2
exit 1
