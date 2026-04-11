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
tmp="$(mktemp "${TMPDIR:-/tmp}/rust-verification-owner.XXXXXX")"
tmp_repo="$(mktemp -d "${TMPDIR:-/tmp}/rust-verification-owner-repo.XXXXXX")"

repo_url="https://github.com/${source_repo}.git"
if [ -n "${CLAUDE_CONFIG_READ_TOKEN:-}" ]; then
    repo_url="https://x-access-token:${CLAUDE_CONFIG_READ_TOKEN}@github.com/${source_repo}.git"
fi

trap 'rm -f "$tmp"; rm -rf "$tmp_repo"' EXIT

mkdir -p "$dest_dir"
git init -q "$tmp_repo"
git -C "$tmp_repo" remote add origin "$repo_url"
git -C "$tmp_repo" fetch --depth=1 --no-tags origin "$source_sha"

fetched_sha="$(git -C "$tmp_repo" rev-parse FETCH_HEAD)"
if [ "$fetched_sha" != "$source_sha" ]; then
    echo "ERROR: fetched commit $fetched_sha does not match requested $source_sha" >&2
    exit 1
fi

git -C "$tmp_repo" show "FETCH_HEAD:lib/rust_verification.py" > "$tmp"
python3 -S "$tmp" scrub-env-keys >/dev/null
install -m 0644 "$tmp" "$dest"

echo "Installed managed Rust owner to $dest from ${source_repo}@${source_sha}"
