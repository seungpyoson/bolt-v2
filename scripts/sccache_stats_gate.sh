#!/usr/bin/env bash
set -euo pipefail

if (( $# != 4 )); then
  echo "::error::sccache statistics gate requires stats path, cache mode, expected version, and compiler-request policy"
  exit 1
fi

stats_path="$1"
cache_mode="$2"
expected_version="$3"
require_compiler_requests="$4"

if [[ "$cache_mode" != "read_only" && "$cache_mode" != "read_write" ]]; then
  echo "::error::enabled sccache cache mode must be read_only or read_write"
  exit 1
fi
if [[ ! "$expected_version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::enabled sccache expected version is invalid"
  exit 1
fi
if [[ "$require_compiler_requests" != "true" \
      && "$require_compiler_requests" != "false" ]]; then
  echo "::error::require-compiler-requests input must be exactly true or false"
  exit 1
fi

if ! jq -e --arg expected_version "${expected_version#v}" '
  def nonnegative_integer:
    type == "number" and . >= 0 and floor == .;
  .version == $expected_version
  and (.stats.compile_requests | nonnegative_integer)
  and (.stats.requests_unsupported_compiler | nonnegative_integer)
  and (.stats.requests_not_compile | nonnegative_integer)
  and (.stats.requests_not_cacheable | nonnegative_integer)
  and (.stats.requests_executed | nonnegative_integer)
  and (.stats.cache_errors.counts | type == "object")
  and (all(.stats.cache_errors.counts[]; nonnegative_integer))
  and (.stats.cache_errors.adv_counts | type == "object")
  and (all(.stats.cache_errors.adv_counts[]; nonnegative_integer))
  and (.stats.cache_misses.counts | type == "object")
  and (all(.stats.cache_misses.counts[]; nonnegative_integer))
  and (.stats.cache_timeouts | nonnegative_integer)
  and (.stats.cache_read_errors | nonnegative_integer)
  and (.stats.cache_write_errors | nonnegative_integer)
  and (.stats.cache_writes | nonnegative_integer)
  and (.stats.dist_errors | nonnegative_integer)
' "$stats_path" > /dev/null; then
  echo "::error::official sccache JSON statistics are malformed or from an unexpected version"
  exit 1
fi

jq . "$stats_path"
compile_requests="$(jq -er '.stats.compile_requests' "$stats_path")"
classified_requests="$(
  jq -er '
    .stats.requests_unsupported_compiler
    + .stats.requests_not_compile
    + .stats.requests_not_cacheable
    + .stats.requests_executed
  ' "$stats_path"
)"
if (( classified_requests != compile_requests )); then
  echo "::error::sccache request accounting reports an incomplete server/protocol request"
  exit 1
fi
runtime_errors="$(
  jq -er '
    .stats.cache_timeouts
    + .stats.cache_read_errors
    + .stats.dist_errors
  ' "$stats_path"
)"
if (( runtime_errors != 0 )); then
  echo "::error::sccache statistics report server, read, or timeout errors"
  exit 1
fi
# `cache_errors` is deliberately not fatal, which is not the same as deciding it
# does not matter. sccache increments that one counter from three unrelated
# places: a `ProcessError` out of `generate_hash_key`, where the compiler itself
# failed; a compile future returning an error, which sets the client return code
# to 1; and `MissType::CacheReadError`, which is also counted as a cache miss and
# a compilation, so the unit is simply built.
#
# The first two already fail the build through cargo, so failing here as well
# adds nothing. The third cannot fail a build at all -- and it is the one that
# fires, at roughly one occurrence per thousand Rust lookups, which was turning
# about a fifth of otherwise green runs red on a cache read that sccache had
# already handled by compiling.
#
# It is still reported, because a rate that stops looking like noise is worth
# seeing.
cache_errors="$(jq -er '([.stats.cache_errors.counts[]] | add // 0)' "$stats_path")"
if (( cache_errors != 0 )); then
  echo "::notice::sccache reported ${cache_errors} cache error(s); each was compiled instead"
fi

cache_write_errors="$(jq -er '.stats.cache_write_errors' "$stats_path")"
cache_writes="$(jq -er '.stats.cache_writes' "$stats_path")"
cache_misses="$(jq -er '([.stats.cache_misses.counts[]] | add // 0)' "$stats_path")"
if [[ "$cache_mode" == "read_write" ]] && (( cache_write_errors != 0 )); then
  echo "::error::read-write sccache statistics report write errors"
  exit 1
fi
if [[ "$cache_mode" == "read_only" ]] \
    && (( cache_write_errors != cache_misses || cache_writes != 0 )); then
  echo "::error::read-only sccache statistics do not show exactly one rejected write per cache miss"
  exit 1
fi
if [[ "$cache_mode" == "read_only" ]] && (( cache_write_errors != 0 )); then
  echo "::notice::read-only sccache rejected ${cache_write_errors} writes after ${cache_misses} cache misses"
fi
if [[ "$require_compiler_requests" == "true" ]] && (( compile_requests == 0 )); then
  echo "::error::sccache was enabled but observed zero compiler requests"
  exit 1
fi
