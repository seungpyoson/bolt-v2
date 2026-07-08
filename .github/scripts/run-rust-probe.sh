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
expected_sha="${RUST_PROBE_EXPECTED_SHA:-}"
probe_id="${RUST_PROBE_ID:-}"
compile_only="${RUST_PROBE_COMPILE_ONLY:-0}"

# Rust Probe contract:
# - RUST_PROBE_TEST_TARGET is a Cargo [[test]] harness target name.
# - RUST_PROBE_TEST_NAME is an optional nextest filter. Suggestions pass
#   "<member_stem>::" for consolidated harness members so nextest stays scoped
#   to that module instead of matching same-named tests in sibling modules.
# - check-test-target and nextest-no-run-test-target compile the whole harness.

if [ -z "$workspace" ]; then
  reject "GITHUB_WORKSPACE is required"
fi
if [ ! -d "$workspace" ]; then
  reject "GITHUB_WORKSPACE must be an existing directory"
fi

# Require an alphanumeric/underscore first character so user input cannot
# become a leading cargo or nextest option such as --help.
target_regex='^[A-Za-z0-9_][A-Za-z0-9_.-]*$'
name_regex='^[A-Za-z0-9_][A-Za-z0-9_:.@/-]*$'
sha_regex='^[0-9a-fA-F]{40}$'
probe_id_regex='^[A-Za-z0-9][A-Za-z0-9_.-]*$'

if [ -z "$expected_sha" ]; then
  reject "RUST_PROBE_EXPECTED_SHA is required"
fi
if [[ ! "$expected_sha" =~ $sha_regex ]]; then
  reject "RUST_PROBE_EXPECTED_SHA must be a full 40-character hex SHA"
fi
if [ -z "$probe_id" ]; then
  reject "RUST_PROBE_ID is required"
fi
if [[ ! "$probe_id" =~ $probe_id_regex ]]; then
  reject "RUST_PROBE_ID must match $probe_id_regex"
fi

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
    if [ "$compile_only" = "1" ]; then
      probe_args=(nextest run --locked --no-run --test "$test_target")
    else
      probe_args=(nextest run --locked --test "$test_target")
    fi
    ;;
  nextest-test-target-name)
    require_target
    require_name
    if [ "$compile_only" = "1" ]; then
      probe_args=(nextest run --locked --no-run --test "$test_target" "$test_name")
    else
      probe_args=(nextest run --locked --test "$test_target" "$test_name")
    fi
    ;;
  *)
    reject "unsupported mode: $mode"
    ;;
esac

cd "$workspace"
actual_sha="$(git rev-parse HEAD)"
expected_sha_lower="$(printf '%s' "$expected_sha" | tr '[:upper:]' '[:lower:]')"
actual_sha_lower="$(printf '%s' "$actual_sha" | tr '[:upper:]' '[:lower:]')"
if [ "$actual_sha_lower" != "$expected_sha_lower" ]; then
  reject "checked-out SHA does not match RUST_PROBE_EXPECTED_SHA: actual=$actual_sha expected=$expected_sha"
fi

echo "Rust Probe id: $probe_id"
echo "Rust Probe checkout SHA: $actual_sha_lower"
echo "Rust Probe mode: $mode"
echo "Rust Probe compile_only: $compile_only"
echo "Rust Probe test_target: ${test_target:-<empty>}"
echo "Rust Probe test_name: ${test_name:-<empty>}"

python3 "$workspace/scripts/rust_verification.py" cargo --repo "$workspace" -- "${probe_args[@]}"
