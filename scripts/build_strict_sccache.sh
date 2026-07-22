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
snapshot_max_bytes="$(jq -r '.snapshot_max_bytes' "$target_json")"
max_runtime_timeout_ms="$(jq -r '.max_runtime_timeout_ms' "$target_json")"
abstract_name_max_bytes="$(jq -r '.abstract_name_max_bytes' "$target_json")"
cache_format_token="$(jq -r '.cache_format_token' "$target_json")"
consumer_socket_template="$(jq -r '.verification_consumer.abstract_socket_template' "$target_json")"
consumer_compiler_path="$(jq -r '.verification_consumer.compiler_path' "$target_json")"
consumer_compiler_family="$(jq -r '.verification_consumer.compiler_family' "$target_json")"
s3_bucket="$(jq -r '.verification_consumer.s3_bucket' "$target_json")"
s3_region="$(jq -r '.verification_consumer.s3_region' "$target_json")"
s3_key_prefix="$(jq -r '.verification_consumer.s3_key_prefix' "$target_json")"
derivative_identity="$(jq -r '.derivative_identity' "$target_json")"
machine="$(jq -r '.elf_machine' "$target_json")"
[[ "$default_features" == "false" ]]

source_dir="$build_root/source"
cargo_home="$build_root/cargo-home"
debug_target_dir="$build_root/debug-target"
release_target_dir="$build_root/release-target"
candidate_dir="$build_root/candidate"
mkdir -p "$source_dir" "$cargo_home" "$debug_target_dir" "$candidate_dir"

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
  -v "$debug_target_dir:/target" \
  -w /volume \
  -e CARGO_HOME=/cargo-home \
  -e CARGO_INCREMENTAL=0 \
  -e CARGO_NET_OFFLINE=true \
  -e CARGO_TARGET_DIR=/target \
  -e LC_ALL=C \
  -e RUSTC_WRAPPER= \
  -e SCCACHE_STRICT_DERIVATIVE_ID="$derivative_identity" \
  -e SCCACHE_STRICT_SNAPSHOT_MAX_BYTES="$snapshot_max_bytes" \
  -e SCCACHE_STRICT_MAX_RUNTIME_TIMEOUT_MS="$max_runtime_timeout_ms" \
  -e SCCACHE_STRICT_ABSTRACT_NAME_MAX_BYTES="$abstract_name_max_bytes" \
  -e SCCACHE_STRICT_CACHE_FORMAT_TOKEN="$cache_format_token" \
  -e SOURCE_DATE_EPOCH="$source_epoch" \
  -e STRICT_BUILD_FEATURES="$features" \
  -e STRICT_BUILD_PROFILE="$profile" \
  -e STRICT_BUILD_TARGET="$target" \
  -e TZ=UTC \
  "$container" \
  sh -ceu 'umask 022; cargo build --locked --offline --no-default-features --features "$STRICT_BUILD_FEATURES" --target "$STRICT_BUILD_TARGET"; cargo test --locked --offline --no-default-features --features "$STRICT_BUILD_FEATURES" --target "$STRICT_BUILD_TARGET" --lib'

[[ ! -e "$release_target_dir" ]]
mkdir "$release_target_dir"
docker run --rm --network none \
  -v "$source_dir:/volume:ro" \
  -v "$cargo_home:/cargo-home:ro" \
  -v "$release_target_dir:/target" \
  -w /volume \
  -e CARGO_HOME=/cargo-home \
  -e CARGO_INCREMENTAL=0 \
  -e CARGO_NET_OFFLINE=true \
  -e CARGO_TARGET_DIR=/target \
  -e LC_ALL=C \
  -e RUSTC_WRAPPER= \
  -e SCCACHE_STRICT_DERIVATIVE_ID="$derivative_identity" \
  -e SCCACHE_STRICT_SNAPSHOT_MAX_BYTES="$snapshot_max_bytes" \
  -e SCCACHE_STRICT_MAX_RUNTIME_TIMEOUT_MS="$max_runtime_timeout_ms" \
  -e SCCACHE_STRICT_ABSTRACT_NAME_MAX_BYTES="$abstract_name_max_bytes" \
  -e SCCACHE_STRICT_CACHE_FORMAT_TOKEN="$cache_format_token" \
  -e SOURCE_DATE_EPOCH="$source_epoch" \
  -e STRICT_BUILD_FEATURES="$features" \
  -e STRICT_BUILD_PROFILE="$profile" \
  -e STRICT_BUILD_TARGET="$target" \
  -e TZ=UTC \
  "$container" \
  sh -ceu 'umask 022; cargo build --locked --offline --profile "$STRICT_BUILD_PROFILE" --no-default-features --features "$STRICT_BUILD_FEATURES" --target "$STRICT_BUILD_TARGET"'

binary="$release_target_dir/$target/$profile/sccache"
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

consumer_config="$build_root/consumer.toml"
python3.12 scripts/sccache_strict.py write-verification-consumer \
  --config ci/sccache-strict.toml --output "$consumer_config" > "$build_root/consumer-config.json"
grep -F "abstract_socket_template = \"$consumer_socket_template\"" "$consumer_config"
grep -F "path = \"$consumer_compiler_path\"" "$consumer_config"
grep -F "family = \"$consumer_compiler_family\"" "$consumer_config"
snapshot_dir="$build_root/snapshots"
mkdir "$snapshot_dir"
snapshot_locator="$(env -i HOME="$build_root" TMPDIR="$build_root" \
  "$binary" --materialize-consumer-snapshot "$consumer_config" \
  "$RUN_ID-$RUN_ATTEMPT-$ARCHITECTURE-$REPLICA" "$snapshot_dir")"
[[ -f "$snapshot_locator" && "$snapshot_locator" == "$snapshot_dir"/* ]]
if env -i HOME="$build_root" TMPDIR="$build_root" \
  "$binary" --materialize-consumer-snapshot "$consumer_config" \
  "$RUN_ID-$RUN_ATTEMPT-$ARCHITECTURE-$REPLICA" "$snapshot_dir" \
  > "$build_root/rematerialize-stdout" 2> "$build_root/rematerialize-stderr"; then
  echo 'strict consumer snapshot was unexpectedly overwritten' >&2
  exit 1
fi
grep -F 'consumer snapshot destination already exists or is unsafe' \
  "$build_root/rematerialize-stderr"

run_startup_negative() {
  local name="$1"
  local expected="$2"
  shift 2
  local smoke_home="$build_root/smoke-$name"
  mkdir -p "$smoke_home"
  if env -i \
    HOME="$smoke_home" \
    SCCACHE_STRICT_BOOTSTRAP="$snapshot_locator" \
    TMPDIR="$smoke_home" \
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

endpoint_file="$build_root/s3-endpoint"
python3.12 -c '
import http.server
import pathlib
import sys

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()
    do_HEAD = do_GET
    def do_PUT(self):
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()
    def log_message(self, format, *args):
        pass

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
pathlib.Path(sys.argv[1]).write_text(f"http://127.0.0.1:{server.server_port}")
server.serve_forever()
' "$endpoint_file" &
s3_server_pid=$!
trap 'kill "$s3_server_pid" 2>/dev/null || true; wait "$s3_server_pid" 2>/dev/null || true' EXIT
# The inner shell expands its positional parameter.
# shellcheck disable=SC2016
timeout "${verification_timeout}ms" sh -ceu 'while [ ! -s "$1" ]; do :; done' sh "$endpoint_file"
s3_endpoint="$(< "$endpoint_file")"
backend_environment=(
  SCCACHE_BUCKET="$s3_bucket"
  SCCACHE_REGION="$s3_region"
  SCCACHE_ENDPOINT="$s3_endpoint"
  SCCACHE_S3_KEY_PREFIX="$s3_key_prefix"
  SCCACHE_S3_NO_CREDENTIALS=true
  SCCACHE_S3_USE_SSL=false
  SCCACHE_S3_ENABLE_VIRTUAL_HOST_STYLE=false
)

run_server_control() {
  local smoke_home="$1"
  shift
  mkdir -p "$smoke_home"
  env -i \
    HOME="$smoke_home" \
    SCCACHE_STRICT_BOOTSTRAP="$snapshot_locator" \
    TMPDIR="$smoke_home" \
    "${backend_environment[@]}" \
    "$binary" "$@"
}

lifecycle_home="$build_root/smoke-lifecycle"
run_server_control "$lifecycle_home" --start-server
if find "$lifecycle_home" -type s -print -quit | grep -q .; then
  echo 'strict server created a filesystem socket' >&2
  exit 1
fi
run_startup_negative occupied-abstract-socket 'Address in use' "${backend_environment[@]}"
run_server_control "$lifecycle_home" --stop-server
run_server_control "$lifecycle_home" --start-server
run_server_control "$lifecycle_home" --stop-server
if find "$lifecycle_home" -type s -print -quit | grep -q .; then
  echo 'strict server left a filesystem socket after lifecycle verification' >&2
  exit 1
fi

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
