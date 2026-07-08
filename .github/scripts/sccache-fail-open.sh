#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 2 ]]; then
  echo "usage: sccache-fail-open.sh <log-path> <command> [args...]" >&2
  exit 2
fi

log_path="$1"
shift

mkdir -p "$(dirname "$log_path")"
: > "$log_path"

run_once() {
  "$@" 2>&1 | tee -a "$log_path"
  return "${PIPESTATUS[0]}"
}

sccache_infrastructure_failed() {
  grep -Eiq '(sccache|RUSTC_WRAPPER|compiler wrapper)' "$log_path"
}

set +e
run_once "$@"
rc="$?"
set -e

if [[ "$rc" -ne 0 && "${BOLT_RUST_VERIFICATION_SCCACHE:-0}" == "1" ]] && sccache_infrastructure_failed; then
  echo "::warning::command failed with sccache active and cache infrastructure markers; retrying once without sccache"
  set +e
  BOLT_RUST_VERIFICATION_SCCACHE=0 run_once "$@"
  rc="$?"
  set -e
fi

exit "$rc"
