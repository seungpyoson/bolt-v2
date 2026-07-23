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
manifest_path="${RUST_PROBE_MANIFEST_PATH:-}"
expected_sha="${RUST_PROBE_EXPECTED_SHA:-}"
probe_id="${RUST_PROBE_ID:-}"

# Rust Probe contract:
# - RUST_PROBE_TEST_TARGET is a Cargo [[test]] harness target name.
# - RUST_PROBE_TEST_NAME is an optional nextest filter. Suggestions pass
#   "<member_stem>::" for consolidated harness members so nextest stays scoped
#   to that module instead of matching same-named tests in sibling modules.
# - RUST_PROBE_MANIFEST_PATH is an optional repo-relative Cargo.toml path so
#   nested workspaces (crates/backtesting-vertical-slice) are targetable.
# - check-test-target and nextest-no-run-test-target compile the whole harness.
# - nextest-lib-name runs a single nextest filter against lib tests.

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
manifest_regex='^[A-Za-z0-9_][A-Za-z0-9_./-]*Cargo\.toml$'
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

manifest_args=()
if [ -n "$manifest_path" ]; then
  if [[ ! "$manifest_path" =~ $manifest_regex ]]; then
    reject "manifest_path must be a repo-relative path ending in Cargo.toml"
  fi
  if [[ "$manifest_path" == *".."* ]]; then
    reject "manifest_path must not contain .."
  fi
  if [ ! -f "$workspace/$manifest_path" ]; then
    reject "manifest_path does not exist: $manifest_path"
  fi
  manifest_args=(--manifest-path "$manifest_path")
fi

root_test_feature_args=()
if [ -z "$manifest_path" ]; then
  root_test_feature_args=(--features test-current-evidence-inspection)
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
    probe_args=(check --locked "${manifest_args[@]}" --lib)
    ;;
  check-test-target)
    require_target
    forbid_name
    probe_args=(check --locked "${manifest_args[@]}" "${root_test_feature_args[@]}" --test "$test_target")
    ;;
  nextest-no-run-test-target)
    require_target
    forbid_name
    probe_args=(nextest run --locked "${manifest_args[@]}" "${root_test_feature_args[@]}" --no-run --test "$test_target")
    ;;
  nextest-lib-name)
    forbid_target
    require_name
    probe_args=(nextest run --locked "${manifest_args[@]}" "${root_test_feature_args[@]}" --lib "$test_name")
    ;;
  nextest-test-target)
    require_target
    forbid_name
    probe_args=(nextest run --locked "${manifest_args[@]}" "${root_test_feature_args[@]}" --test "$test_target")
    ;;
  nextest-test-target-name)
    require_target
    require_name
    probe_args=(nextest run --locked "${manifest_args[@]}" "${root_test_feature_args[@]}" --test "$test_target" "$test_name")
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
echo "Rust Probe test_target: ${test_target:-<empty>}"
echo "Rust Probe test_name: ${test_name:-<empty>}"
echo "Rust Probe manifest_path: ${manifest_path:-<empty>}"

set -x
cargo "${probe_args[@]}"
