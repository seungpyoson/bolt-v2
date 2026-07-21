#!/usr/bin/env bash
set -euo pipefail

for name in ARCHITECTURE GITHUB_OUTPUT HEAD_SHA REPLICA REPOSITORY RUN_ATTEMPT RUN_ID RUNNER_TEMP; do
  [[ -n "${!name:-}" ]] || {
    echo "required build input is absent: $name" >&2
    exit 1
  }
done

build_root="$RUNNER_TEMP/strict-sccache-$RUN_ID-$RUN_ATTEMPT-$ARCHITECTURE-$REPLICA"
[[ ! -e "$build_root" ]]
mkdir -p "$build_root"
target_json="$build_root/target.json"
python3.12 scripts/sccache_strict.py show-target \
  --config ci/sccache-strict.toml --architecture "$ARCHITECTURE" > "$target_json"
target="$(jq -r '.target' "$target_json")"
source_version="$(jq -r '.source_version' "$target_json")"
source_url="$(jq -r '.source_url' "$target_json")"
source_sha="$(jq -r '.source_sha256' "$target_json")"
container="$(jq -r '.container' "$target_json")"
rustc_release="$(jq -r '.rustc_release' "$target_json")"
rustc_commit="$(jq -r '.rustc_commit' "$target_json")"
features="$(jq -r '.features | join(",")' "$target_json")"
default_features="$(jq -r '.default_features' "$target_json")"
profile="$(jq -r '.profile' "$target_json")"
source_epoch="$(jq -r '.source_date_epoch' "$target_json")"
patch="$(jq -r '.patch' "$target_json")"
verification_timeout="$(jq -r '.verification_timeout_ms' "$target_json")"
max_frame_bytes="$(jq -r '.max_frame_bytes' "$target_json")"
verification_cache_mode="$(jq -r '.verification_cache_mode' "$target_json")"
derivative_identity="$(jq -r '.derivative_identity' "$target_json")"
machine="$(jq -r '.elf_machine' "$target_json")"
[[ "$default_features" == "false" ]]

source_dir="$build_root/source"
cargo_home="$build_root/cargo-home"
target_dir="$build_root/target"
candidate_dir="$build_root/candidate"
mkdir -p "$source_dir" "$cargo_home" "$target_dir" "$candidate_dir"

archive="$build_root/source.tar.gz"
curl --fail --location --proto '=https' --tlsv1.2 "$source_url" --output "$archive"
echo "$source_sha  $archive" | sha256sum --check --strict
tar -xzf "$archive" --strip-components=1 -C "$source_dir"
patch_summary="$(git -C "$source_dir" apply --unidiff-zero --numstat "$patch")"
[[ -n "$patch_summary" ]]
git -C "$source_dir" apply --unidiff-zero --check --verbose "$patch"
git -C "$source_dir" apply --unidiff-zero --verbose "$patch"
git -C "$source_dir" apply --unidiff-zero --reverse --check "$patch"

docker run --rm "$container" rustc --version --verbose > "$build_root/rustc-version.txt"
grep -Fx "release: $rustc_release" "$build_root/rustc-version.txt"
grep -Fx "commit-hash: $rustc_commit" "$build_root/rustc-version.txt"
docker run --rm \
  -v "$source_dir:/volume" \
  -v "$cargo_home:/cargo-home" \
  -w /volume \
  -e CARGO_HOME=/cargo-home \
  "$container" \
  cargo fetch --locked --target "$target"

docker run --rm --network none \
  -v "$source_dir:/volume" \
  -v "$cargo_home:/cargo-home:ro" \
  -v "$target_dir:/target" \
  -w /volume \
  -e CARGO_HOME=/cargo-home \
  -e CARGO_INCREMENTAL=0 \
  -e CARGO_NET_OFFLINE=true \
  -e CARGO_TARGET_DIR=/target \
  -e LC_ALL=C \
  -e RUSTC_WRAPPER= \
  -e SCCACHE_STRICT_DERIVATIVE_ID="$derivative_identity" \
  -e SCCACHE_S3_RW_MODE="$verification_cache_mode" \
  -e SCCACHE_STRICT_CACHE_READ_TIMEOUT_MS="$verification_timeout" \
  -e SCCACHE_STRICT_CACHE_WRITE_TIMEOUT_MS="$verification_timeout" \
  -e SCCACHE_STRICT_IPC_TIMEOUT_MS="$verification_timeout" \
  -e SCCACHE_STRICT_MAX_FRAME_BYTES="$max_frame_bytes" \
  -e SCCACHE_STRICT_STARTUP_TIMEOUT_MS="$verification_timeout" \
  -e SOURCE_DATE_EPOCH="$source_epoch" \
  -e STRICT_BUILD_FEATURES="$features" \
  -e STRICT_BUILD_PROFILE="$profile" \
  -e STRICT_BUILD_TARGET="$target" \
  -e TZ=UTC \
  "$container" \
  sh -ceu 'umask 022; cargo build --locked --offline --no-default-features --features "$STRICT_BUILD_FEATURES" --target "$STRICT_BUILD_TARGET"; cargo test --locked --offline --no-default-features --features "$STRICT_BUILD_FEATURES" --target "$STRICT_BUILD_TARGET" --lib; cargo build --locked --offline --profile "$STRICT_BUILD_PROFILE" --no-default-features --features "$STRICT_BUILD_FEATURES" --target "$STRICT_BUILD_TARGET"'

binary="$target_dir/$target/$profile/sccache"
readelf -h "$binary" | grep -F 'Machine:' | grep -F "$machine"
readelf -l "$binary" > "$build_root/program-headers.txt"
if grep -Fq 'INTERP' "$build_root/program-headers.txt"; then
  echo 'strict candidate has a runtime program interpreter' >&2
  exit 1
fi
readelf -d "$binary" > "$build_root/dynamic-section.txt"
if grep -Fq '(NEEDED)' "$build_root/dynamic-section.txt"; then
  echo 'strict candidate has a dynamic shared-library dependency' >&2
  exit 1
fi
[[ "$("$binary" --version)" == "sccache $source_version" ]]

run_startup_negative() {
  local name="$1"
  local expected="$2"
  shift 2
  local smoke_home="$build_root/smoke-$name"
  mkdir -p "$smoke_home"
  if env -i \
    HOME="$smoke_home" \
    SCCACHE_SERVER_UDS="$smoke_home/server.sock" \
    SCCACHE_S3_RW_MODE="$verification_cache_mode" \
    TMPDIR="$smoke_home" \
    SCCACHE_STRICT_STARTUP_TIMEOUT_MS="$verification_timeout" \
    SCCACHE_STRICT_IPC_TIMEOUT_MS="$verification_timeout" \
    SCCACHE_STRICT_MAX_FRAME_BYTES="$max_frame_bytes" \
    SCCACHE_STRICT_CACHE_READ_TIMEOUT_MS="$verification_timeout" \
    SCCACHE_STRICT_CACHE_WRITE_TIMEOUT_MS="$verification_timeout" \
    "$@" \
    "$binary" --start-server > "$smoke_home/stdout" 2> "$smoke_home/stderr"; then
    cat "$smoke_home/stdout" "$smoke_home/stderr" >&2
    echo "strict candidate unexpectedly passed negative startup control: $name" >&2
    exit 1
  fi
  if ! grep -F "$expected" "$smoke_home/stderr"; then
    cat "$smoke_home/stdout" "$smoke_home/stderr" >&2
    echo "strict candidate returned the wrong startup failure: $name" >&2
    exit 1
  fi
}

run_startup_negative missing-s3 'strict sccache requires the governed S3 cache backend'
run_startup_negative disk-backend 'strict sccache permits exactly one S3 cache backend' \
  SCCACHE_DIR="$build_root/smoke-disk-backend/cache"

cp "$binary" "$candidate_dir/sccache"
python3.12 scripts/sccache_strict.py candidate-manifest \
  --config ci/sccache-strict.toml \
  --binary "$candidate_dir/sccache" \
  --repository "$REPOSITORY" \
  --run-id "$RUN_ID" \
  --run-attempt "$RUN_ATTEMPT" \
  --head-sha "$HEAD_SHA" \
  --architecture "$ARCHITECTURE" \
  --replica "$REPLICA" \
  --output "$candidate_dir/manifest.json"

echo "candidate_path=$candidate_dir" >> "$GITHUB_OUTPUT"
