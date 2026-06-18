#!/usr/bin/env bash
# Empirical probe: which git hooks fire on `git pull --ff-only`, `git pull --rebase`,
# and a regular merge pull? Locks the Lane H trigger architecture to fact.
# Re-run if git is upgraded or the deployment machine changes; update
# docs/ops/clean-merged-triggers.md with the new results.
set -u

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

REMOTE="$TMP/remote.git"
git init --bare -b main "$REMOTE" >/dev/null

git init -b main "$TMP/work" >/dev/null
cd "$TMP/work"
git remote add origin "$REMOTE"

HOOKLOG="$TMP/hooks.log"
: > "$HOOKLOG"
for h in post-merge post-rewrite post-checkout; do
  cat > ".git/hooks/$h" <<EOF
#!/usr/bin/env bash
echo "\$(date +%s) $h argc=\$# args=[\$*] head=\$(git rev-parse --short HEAD) branch=\$(git symbolic-ref --short HEAD 2>/dev/null)" >> "$HOOKLOG"
EOF
  chmod +x ".git/hooks/$h"
done

git commit --allow-empty -m "init" >/dev/null
git push -u origin main >/dev/null 2>&1

advance_remote() {
  local msg="$1"
  git clone -q -b main "$REMOTE" "$TMP/other.$$" >/dev/null
  git -C "$TMP/other.$$" commit --allow-empty -m "$msg" >/dev/null
  git -C "$TMP/other.$$" push -q origin main >/dev/null 2>&1
  rm -rf "$TMP/other.$$"
}

echo "git version: $(git --version)"
echo "============================================"

advance_remote "ff-1"
: > "$HOOKLOG"
echo "=== Scenario 1: git pull --ff-only (origin ahead by 1, FF possible) ==="
git pull --ff-only origin main 2>&1 | sed 's/^/  out: /'
echo "--- hooks fired ---"
cat "$HOOKLOG" | sed 's/^/  hook: /' || echo "  (none)"
echo

advance_remote "rebase-1"
: > "$HOOKLOG"
echo "=== Scenario 2: git pull --rebase (origin ahead by 1, FF possible) ==="
git pull --rebase origin main 2>&1 | sed 's/^/  out: /'
echo "--- hooks fired ---"
cat "$HOOKLOG" | sed 's/^/  hook: /' || echo "  (none)"
echo

git commit --allow-empty -m "local-diverge" >/dev/null
advance_remote "remote-diverge"
: > "$HOOKLOG"
echo "=== Scenario 3: git pull --no-ff (real merge commit) ==="
git pull --no-ff origin main 2>&1 | sed 's/^/  out: /'
echo "--- hooks fired ---"
cat "$HOOKLOG" | sed 's/^/  hook: /' || echo "  (none)"
echo

git checkout -b feat/x >/dev/null 2>&1
: > "$HOOKLOG"
echo "=== Scenario 4: git checkout main (from feat/x) ==="
git checkout main 2>&1 | sed 's/^/  out: /'
echo "--- hooks fired ---"
cat "$HOOKLOG" | sed 's/^/  hook: /' || echo "  (none)"

# Divergent rebase (actual rebase, not FF) — probes post-rewrite coverage.
cd "$TMP"
git init -b main "$TMP/work2" >/dev/null
cd "$TMP/work2"
git remote add origin "$REMOTE"
git fetch -q origin main >/dev/null 2>&1
git reset --hard origin/main >/dev/null 2>&1
HOOKLOG2="$TMP/hooks2.log"; : > "$HOOKLOG2"
for h in post-merge post-rewrite post-checkout; do
  cat > ".git/hooks/$h" <<EOF
#!/usr/bin/env bash
echo "$h args=[\$*]" >> "$HOOKLOG2"
EOF
  chmod +x ".git/hooks/$h"
done
git commit --allow-empty -m local-only >/dev/null
git clone -q -b main "$REMOTE" "$TMP/other2" >/dev/null
git -C "$TMP/other2" commit --allow-empty -m remote-only >/dev/null
git -C "$TMP/other2" push -q origin main >/dev/null 2>&1
: > "$HOOKLOG2"
echo
echo "=== Scenario 5: git pull --rebase with divergent local (actual rebase) ==="
git pull --rebase origin main 2>&1 | sed 's/^/  out: /'
echo "--- hooks fired ---"
cat "$HOOKLOG2" | sed 's/^/  hook: /' || echo "  (none)"
