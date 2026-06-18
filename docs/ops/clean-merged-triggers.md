# clean-merged — trigger probe (empirical)

Locked on the deployment machine before any hook install. Re-run if git is
upgraded or the deployment machine changes.

**Machine:** macOS (Apple Git-155)
**git version:** 2.50.1
**Method:** fully config-isolated throwaway temp repos
(`GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null`), all three
candidate hooks instrumented.
**Date:** 2026-06-17

## Verdict

All three Lane H hooks are justified on this machine/git-version. The dominant
"merge PR on GitHub → `git checkout main && git pull`" workflow is covered.

## Results

| Scenario | post-merge | post-rewrite | post-checkout | repo state when hook runs |
|---|---|---|---|---|
| `git pull --ff-only` (origin/main ahead by 1) | **FIRES** (args=[0]) | — | — | origin/main already updated; HEAD = new tip |
| `git pull --rebase` (FF still possible) | **FIRES** (args=[0]) | — | — | same; FF path taken |
| `git pull --no-ff` (real merge commit) | **FIRES** (args=[0]) | — | — | merge commit created |
| `git pull --rebase` (local divergent; actual rebase) | — | **FIRES** (args=[rebase]) | fires (internal checkout) | HEAD rewritten |
| `git checkout main` from feature branch | — | — | **FIRES** ($3=1) | origin/main NOT fetched (stale) |

## Implications

1. **Lane H primary trigger = `post-merge`.** Covers FF pull (the common
   post-PR-merge action) and real-merge pulls.
2. **`post-rewrite` is required** for operators with `pull.rebase=true` whose
   local main has divergent commits.
3. **`post-checkout` is a stale-state trap.** When the operator runs
   `git checkout main` *before* pulling, origin/main is stale at hook time.
   `post-merge` (which fires after the subsequent pull) closes the gap on the
   same session. Do NOT put a fetch inside `post-checkout` — that re-introduces
   network-in-hook (rejected in the design).
4. **No `post-fetch` hook exists in git.** Confirmed by the absence of any
   fetch-only hook firing. The `post-merge`/`post-rewrite` pair is the only
   native surface that fires after origin/main is updated.

## Dispute resolution

Three external reviews split on whether `post-merge` fires on FF pulls
(GPT/Claude: yes; Kimi: no). This probe says **yes** on this machine. Kimi's
negative result does not reproduce here; likely a different git version, a
non-isolated config, or a probe that did not actually achieve FF.

## Re-probe

The probe script lives at `scripts/probe_clean_merged_triggers.sh`. If a
future git version regresses FF post-merge firing, Lane H falls back to a
throttled-fetch-in-post-checkout alternative (documented in the design).
