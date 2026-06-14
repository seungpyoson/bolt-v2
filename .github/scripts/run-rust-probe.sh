#!/usr/bin/env bash
set -euo pipefail

reject() {
  echo "ERROR: $*" >&2
  exit 2
}

if [ "$#" -ne 0 ]; then
  reject "run-rust-probe.sh does not accept command-line arguments"
fi

workspace="${GITHUB_WORKSPACE:-}"
mode="${RUST_PROBE_MODE:-}"
test_target="${RUST_PROBE_TEST_TARGET:-}"
test_name="${RUST_PROBE_TEST_NAME:-}"

if [ -z "$workspace" ]; then
  reject "GITHUB_WORKSPACE is required"
fi
if [ ! -d "$workspace" ]; then
  reject "GITHUB_WORKSPACE must be an existing directory"
fi

target_regex='^[A-Za-z0-9_.-]+$'
name_regex='^[A-Za-z0-9_:.@/-]+$'

require_target() {
  if [ -z "$test_target" ]; then
    reject "test_target is required for mode $mode"
  fi
  if [[ ! "$test_target" =~ $target_regex ]]; then
    reject "test_target must match $target_regex"
  fi
}

forbid_target() {
  if [ -n "$test_target" ]; then
    reject "test_target is forbidden for mode $mode"
  fi
}

require_name() {
  if [ -z "$test_name" ]; then
    reject "test_name is required for mode $mode"
  fi
  if [[ ! "$test_name" =~ $name_regex ]]; then
    reject "test_name must match $name_regex"
  fi
}

forbid_name() {
  if [ -n "$test_name" ]; then
    reject "test_name is forbidden for mode $mode"
  fi
}

case "$mode" in
  check-lib)
    forbid_target
    forbid_name
    probe_args=(check --locked --lib)
    ;;
  check-test-target)
    require_target
    forbid_name
    probe_args=(check --locked --test "$test_target")
    ;;
  nextest-no-run-test-target)
    require_target
    forbid_name
    probe_args=(nextest run --locked --no-run --test "$test_target")
    ;;
  nextest-test-target)
    require_target
    forbid_name
    probe_args=(nextest run --locked --test "$test_target")
    ;;
  nextest-test-target-name)
    require_target
    require_name
    probe_args=(nextest run --locked --test "$test_target" "$test_name")
    ;;
  *)
    reject "unsupported mode: $mode"
    ;;
esac

echo "Rust Probe mode: $mode"
echo "Rust Probe test_target: ${test_target:-<empty>}"
echo "Rust Probe test_name: ${test_name:-<empty>}"

cd "$workspace"
python3 "$workspace/scripts/rust_verification.py" cargo --repo "$workspace" -- "${probe_args[@]}"
