#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin"

cat > "$test_root/bin/gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail

: "${FAKE_GH_SCENARIO:?}"
: "${FAKE_GH_LOG:?}"

printf '%s\n' "$*" >> "$FAKE_GH_LOG"

render_query() {
    local json="$1"
    shift
    local query=""
    while (( $# > 0 )); do
        if [[ "$1" == "--jq" ]]; then
            query="$2"
            break
        fi
        shift
    done
    if [[ -z "$query" ]]; then
        printf '%s\n' "$json"
        return
    fi
    jq -r "$query" <<< "$json"
}

pull_json() {
    local number="$1"
    local state="$2"
    local draft="$3"
    local base="$4"
    local head="$5"
    local head_repository="$6"
    local body="$7"
    jq -cn \
        --argjson number "$number" \
        --arg state "$state" \
        --argjson draft "$draft" \
        --arg base "$base" \
        --arg head "$head" \
        --arg head_repository "$head_repository" \
        --arg body "$body" \
        '{number: $number, state: $state, isDraft: $draft,
          baseRefName: $base, headRefName: $head,
          headRepository: {nameWithOwner: $head_repository}, body: $body}'
}

fixture() {
    local number="$1"
    local previous
    case "$FAKE_GH_SCENARIO:$number" in
        standalone:101)
            pull_json 101 OPEN false main feature-101 seungpyoson/bolt-v2 ""
            ;;
        two:201)
            pull_json 201 OPEN false main stack-201 seungpyoson/bolt-v2 ""
            ;;
        two:202)
            pull_json 202 OPEN false stack-201 stack-202 seungpyoson/bolt-v2 'Depends-On: #201'
            ;;
        three:301)
            pull_json 301 OPEN false main stack-301 seungpyoson/bolt-v2 ""
            ;;
        three:302)
            pull_json 302 OPEN false stack-301 stack-302 seungpyoson/bolt-v2 'Depends-On: #301'
            ;;
        three:303)
            pull_json 303 OPEN false stack-302 stack-303 seungpyoson/bolt-v2 'Depends-On: #302'
            ;;
        missing:401)
            pull_json 401 OPEN false stack-400 stack-401 seungpyoson/bolt-v2 ""
            ;;
        malformed:501)
            pull_json 501 OPEN false stack-500 stack-501 seungpyoson/bolt-v2 'Depends-On: #0500'
            ;;
        malformed_suffix:511)
            pull_json 511 OPEN false stack-510 stack-511 seungpyoson/bolt-v2 'Depends-On: #510 trailing'
            ;;
        malformed_unicode:521)
            pull_json 521 OPEN false stack-520 stack-521 seungpyoson/bolt-v2 'Depends-On: #５２０'
            ;;
        malformed_multiple:531)
            pull_json 531 OPEN false stack-530 stack-531 seungpyoson/bolt-v2 $'Depends-On: #530\nDepends-On: #529'
            ;;
        crlf:540)
            pull_json 540 OPEN false main stack-540 seungpyoson/bolt-v2 ""
            ;;
        crlf:541)
            pull_json 541 OPEN false stack-540 stack-541 seungpyoson/bolt-v2 $'Depends-On: #540\r'
            ;;
        mismatch:601)
            pull_json 601 OPEN false main wrong-head seungpyoson/bolt-v2 ""
            ;;
        mismatch:602)
            pull_json 602 OPEN false expected-head stack-602 seungpyoson/bolt-v2 'Depends-On: #601'
            ;;
        foreign_head:611)
            pull_json 611 OPEN false main expected-head someone/fork ""
            ;;
        foreign_head:612)
            pull_json 612 OPEN false expected-head stack-612 seungpyoson/bolt-v2 'Depends-On: #611'
            ;;
        closed:701)
            pull_json 701 CLOSED false main stack-701 seungpyoson/bolt-v2 ""
            ;;
        closed:702)
            pull_json 702 OPEN false stack-701 stack-702 seungpyoson/bolt-v2 'Depends-On: #701'
            ;;
        draft:711)
            pull_json 711 OPEN true main stack-711 seungpyoson/bolt-v2 ""
            ;;
        draft:712)
            pull_json 712 OPEN false stack-711 stack-712 seungpyoson/bolt-v2 'Depends-On: #711'
            ;;
        cycle:801)
            pull_json 801 OPEN false branch-802 branch-801 seungpyoson/bolt-v2 'Depends-On: #802'
            ;;
        cycle:802)
            pull_json 802 OPEN false branch-801 branch-802 seungpyoson/bolt-v2 'Depends-On: #801'
            ;;
        excessive:9??)
            if (( number == 901 )); then
                pull_json 901 OPEN false main stack-901 seungpyoson/bolt-v2 ""
            else
                previous=$((number - 1))
                pull_json "$number" OPEN false "stack-$previous" "stack-$number" seungpyoson/bolt-v2 "Depends-On: #$previous"
            fi
            ;;
        mixed:1001)
            pull_json 1001 OPEN false main feature-1001 seungpyoson/bolt-v2 ""
            ;;
        mixed:1002)
            pull_json 1002 OPEN false stack-1001 stack-1002 seungpyoson/bolt-v2 ""
            ;;
        overlap:1101)
            pull_json 1101 OPEN false main stack-1101 seungpyoson/bolt-v2 ""
            ;;
        overlap:1102)
            pull_json 1102 OPEN false stack-1101 stack-1102 seungpyoson/bolt-v2 'Depends-On: #1101'
            ;;
        overlap:1103)
            pull_json 1103 OPEN false stack-1102 stack-1103 seungpyoson/bolt-v2 'Depends-On: #1102'
            ;;
        multi:1201)
            pull_json 1201 OPEN false main feature-1201 seungpyoson/bolt-v2 ""
            ;;
        multi:1202)
            pull_json 1202 OPEN false main feature-1202 seungpyoson/bolt-v2 ""
            ;;
        partial:1301)
            pull_json 1301 OPEN false main feature-1301 seungpyoson/bolt-v2 ""
            ;;
        partial:1302)
            pull_json 1302 OPEN false main feature-1302 seungpyoson/bolt-v2 ""
            ;;
        partial:1303)
            pull_json 1303 OPEN false main feature-1303 seungpyoson/bolt-v2 ""
            ;;
        nonmain:1401)
            pull_json 1401 OPEN false trunk feature-1401 seungpyoson/bolt-v2 ""
            ;;
        *)
            return 1
            ;;
    esac
}

case "$1 $2" in
    "repo view")
        default_branch=main
        if [[ "$FAKE_GH_SCENARIO" == nonmain ]]; then
            default_branch=trunk
        fi
        repository_json="$(jq -cn --arg default_branch "$default_branch" \
            '{url: "https://github.com/seungpyoson/bolt-v2",
              nameWithOwner: "seungpyoson/bolt-v2",
              defaultBranchRef: {name: $default_branch}}')"
        render_query "$repository_json" "${@:3}"
        ;;
    "pr view")
        if ! pr_json="$(fixture "$3")"; then
            exit 1
        fi
        render_query "$pr_json" "${@:4}"
        ;;
    "pr comment")
        if (( $# != 7 )) \
            || [[ "$4" != "--repo" ]] \
            || [[ "$5" != "github.com/seungpyoson/bolt-v2" ]] \
            || [[ "$6" != "--body" ]] \
            || [[ "$7" != '@mergifyio queue' ]]; then
            printf 'unexpected queue comment arguments:' >&2
            printf ' %q' "$@" >&2
            printf '\n' >&2
            exit 98
        fi
        if [[ "$FAKE_GH_SCENARIO" == partial && "$3" == 1302 ]]; then
            exit 1
        fi
        ;;
    *)
        exit 99
        ;;
esac
FAKE_GH
chmod +x "$test_root/bin/gh"

case_output=""
case_status=0
case_log="$test_root/gh.log"

run_case() {
    local scenario="$1"
    shift
    : > "$case_log"
    set +e
    case_output="$(
        PATH="$test_root/bin:$PATH" \
        FAKE_GH_SCENARIO="$scenario" \
        FAKE_GH_LOG="$case_log" \
        just --justfile "$repo_root/justfile" --working-directory "$repo_root" \
            merge-queue "$@" 2>&1
    )"
    case_status=$?
    set -e
}

expect_status() {
    local expected="$1"
    if (( case_status != expected )); then
        printf 'expected status %s, got %s\n%s\n' "$expected" "$case_status" "$case_output" >&2
        exit 1
    fi
}

expect_output() {
    local expected="$1"
    if [[ "$case_output" != *"$expected"* ]]; then
        printf 'missing output: %s\n%s\n' "$expected" "$case_output" >&2
        exit 1
    fi
}

expect_comment_targets() {
    local actual
    actual="$(awk '$1 == "pr" && $2 == "comment" {print $3}' "$case_log" | paste -sd ' ' -)"
    local expected="$*"
    if [[ "$actual" != "$expected" ]]; then
        printf 'expected comment targets [%s], got [%s]\n%s\n' "$expected" "$actual" "$case_output" >&2
        exit 1
    fi
}

run_case standalone 101
expect_status 0
expect_comment_targets 101

run_case nonmain 1401
expect_status 0
expect_comment_targets 1401

run_case two 202
expect_status 0
expect_comment_targets 202

run_case three 303
expect_status 0
expect_comment_targets 303

run_case crlf 541
expect_status 0
expect_comment_targets 541

for scenario_and_pr in \
    missing:401 \
    malformed:501 \
    malformed_suffix:511 \
    malformed_unicode:521 \
    malformed_multiple:531; do
    scenario="${scenario_and_pr%%:*}"
    pr_number="${scenario_and_pr##*:}"
    run_case "$scenario" "$pr_number"
    expect_status 2
    expect_output "run mergify stack push"
    expect_output "No queue requests were submitted."
    expect_comment_targets
done

run_case mismatch 602
expect_status 2
expect_output "does not match"
expect_comment_targets

run_case foreign_head 612
expect_status 2
expect_output "does not match"
expect_comment_targets

run_case closed 702
expect_status 2
expect_output "is not open"
expect_comment_targets

run_case draft 712
expect_status 2
expect_output "is a draft"
expect_comment_targets

run_case cycle 801
expect_status 2
expect_output "cycle"
expect_comment_targets

run_case excessive 920
expect_status 0
expect_comment_targets 920

run_case excessive 921
expect_status 2
expect_output "exceeds Mergify's maximum stack depth of 20"
expect_comment_targets

run_case mixed 1001 1002
expect_status 2
expect_output "No queue requests were submitted."
expect_comment_targets

run_case overlap 1102 1103
expect_status 2
expect_output "overlap"
expect_comment_targets

run_case multi 1201 1202
expect_status 0
expect_comment_targets 1201 1202

run_case partial 1301 1302 1303
expect_status 1
expect_output "Confirmed submitted: 1301"
expect_output "Submission outcome unknown: 1302"
expect_output "Not attempted: 1303"
expect_comment_targets 1301 1302

run_case standalone 101 101
expect_status 2
expect_output "duplicate pull request number: 101"
expect_comment_targets

printf 'merge-queue behavior tests passed\n'
