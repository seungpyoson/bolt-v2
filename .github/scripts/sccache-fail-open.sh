#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 4 || "${1:-}" != "--on" || ! "${2:-}" =~ ^(any|cache-error)$ ]]; then
  echo "usage: sccache-fail-open.sh --on any|cache-error <log-path> <compile-command> [args...]" >&2
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

cache_infrastructure_failed() {
  grep -Eiq \
    '(sccache:.*(error|fail|failed|could not|unable|timeout|timed out|connection|server)|failed to communicate with sccache|compiler wrapper.*(error|fail|failed)|RUSTC_WRAPPER.*(not found|error|fail|failed))' \
    "$log_path"
}

set +e
run_once "$@"
rc="$?"
set -e

should_retry=false
if [[ "$rc" -ne 0 && "${BOLT_RUST_VERIFICATION_SCCACHE:-0}" == "1" ]]; then
  if [[ "$retry_mode" == "any" ]]; then
    should_retry=true
  elif cache_infrastructure_failed; then
    should_retry=true
  fi
fi

if [[ "$should_retry" == "true" ]]; then
  echo "::warning::compile command failed with sccache active; retrying once without sccache"
  set +e
  BOLT_RUST_VERIFICATION_SCCACHE=0 run_once "$@"
  rc="$?"
  set -e
fi

exit "$rc"
