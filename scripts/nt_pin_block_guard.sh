#!/usr/bin/env bash
set -euo pipefail

repo_root="${1:?usage: nt_pin_block_guard.sh <repo_root> <base_branch> <pr_number>}"
base_branch="${2:?usage: nt_pin_block_guard.sh <repo_root> <base_branch> <pr_number>}"
pr_number="${3:?usage: nt_pin_block_guard.sh <repo_root> <base_branch> <pr_number>}"

cd "$repo_root"

base_ref="refs/remotes/origin/pr-base-${pr_number}"
head_ref="refs/remotes/origin/pr-head-${pr_number}"

git fetch --no-tags origin \
  "+refs/heads/${base_branch}:${base_ref}" \
  "+refs/pull/${pr_number}/head:${head_ref}"

just nt-pointer-probe-check-nt-mutation "$base_ref" "$head_ref"
