#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 4 || "${1:-}" != "--on" || "${2:-}" != "any" ]]; then
  echo "usage: sccache-fail-open.sh --on any <log-path> <compile-command> [args...]" >&2
  exit 2
fi

retry_mode="$2"
log_path="$3"
shift 3

mkdir -p "$(dirname "$log_path")"
: > "$log_path"

run_once() {
  "$@" 2>&1 | tee -a "$log_path"
  return "${PIPESTATUS[0]}"
}

set +e
run_once "$@"
rc="$?"
set -e

if [[ "$retry_mode" == "any" && "$rc" -ne 0 && "${BOLT_RUST_VERIFICATION_SCCACHE:-0}" == "1" ]]; then
  echo "::warning::compile command failed with sccache active; retrying once without sccache"
  set +e
  BOLT_RUST_VERIFICATION_SCCACHE=0 run_once "$@"
  rc="$?"
  set -e
fi

exit "$rc"
