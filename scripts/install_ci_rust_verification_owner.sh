#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <github-owner/repo> <commit-sha>" >&2
    exit 2
fi

source_repo="$1"
source_sha="$2"
dest="${HOME}/.claude/lib/rust_verification.py"
dest_dir="$(dirname "$dest")"
url="https://raw.githubusercontent.com/${source_repo}/${source_sha}/lib/rust_verification.py"
tmp="$(mktemp "${TMPDIR:-/tmp}/rust-verification-owner.XXXXXX")"

trap 'rm -f "$tmp"' EXIT

mkdir -p "$dest_dir"
curl -fsSL "$url" -o "$tmp"
python3 -S "$tmp" scrub-env-keys >/dev/null
install -m 0644 "$tmp" "$dest"

echo "Installed managed Rust owner to $dest from ${source_repo}@${source_sha}"
