# clean-merged — design

Issue: https://github.com/seungpyoson/bolt-v2/issues/802

Auto-cleanup of merged branches and worktrees. Always-on, agent-agnostic
(works for Claude Code, Codex, Aider, raw `git`), no per-session instruction.

## Problem

Merged PRs leave local git branches and git worktrees. Current accumulation:
~40 worktrees, ~14 branches merged into `origin/main`, ~5 `[gone]`-upstream
branches.

## Hard constraints

- Universal across every tool that touches git (native git hooks only — no
  agent-specific lifecycle hooks).
- Must not delete unmerged work. Branch refs are reflog-recoverable (~30d for
  unreachable tips via `gc.reflogExpireUnreachable`); worktree filesystem
  removal is **irreversible**.
- Idempotent. Never breaks the git operation that triggered it.
- Repo rules: NO HARDCODES (TOML config), NO DUAL PATHS, NO DEBTS.

## Architecture: three lanes split by trust/speed profile

### Lane H — Hook (always-on, offline, fast, reflog-safe)

- **Triggers:** `post-merge` (primary; FF verified — see
  `clean-merged-triggers.md`), `post-rewrite` (divergent rebase pull),
  `post-checkout` (gated `$3==1 && target==trunk`, best-effort).
- **Synchronous by default.** Typical sweep is sub-second. Detach only if
  profiling demands, with non-blocking `fcntl.flock`, stdin→`/dev/null`,
  stdout/stderr→audit log.
- **Does ONLY:** kill-switch check → cheap candidate precheck → for each
  non-trunk, non-current branch NOT bound to a worktree: re-read tip, check
  `git merge-base --is-ancestor <B> <trunk>` immediately before delete, write
  timestamped backup ref, then `git branch -d <B>`. **No gh. No `-D`.**
- Worktree-bound eligible branches logged as "Lane W candidate."

### Lane R — Reconcile (hook-spawned detached OR manual; network-bound)

- `post-merge` spawns Lane R detached (`setsid`, stdin→`/dev/null`,
  stdout/stderr→log) with strict Python `subprocess.run(timeout=cfg)` on the
  gh call and a 5-min cache. The git op returns instantly. Manual
  `just clean-merged --reconcile` for on-demand.
- **Skip worktree-bound branches** — those flow to Lane W.
- **Per-branch gh query** (not `--limit 200` newest-first which misses old
  backlog PRs): for each non-ancestor, non-worktree-bound branch:
  `gh pr list --head <B> --state merged --json number,headRefOid,baseRefName,headRepositoryOwner,isCrossRepository --limit 5`
  (`GH_PROMPT_DISABLED=1 GIT_TERMINAL_PROMPT=0`, Python timeout).
- Match only if `headRefOid == git rev-parse <B>` AND `baseRefName == trunk`
  AND `headRepositoryOwner == <origin owner>` AND NOT `isCrossRepository`.
- On exact match: timestamped backup ref, then
  `git update-ref -d refs/heads/<B> <expected-sha>` (CAS; refuses on SHA drift).
- Any gh non-zero / timeout / malformed JSON → keep + log "gh unavailable,
  Lane R skipped." Validate `headRefOid` is SHA-shaped before compare.

### Lane W — Worktree (explicit; archive-then-remove)

**Primitive:** `tar -czf <quarantine>/worktree.tar.gz <wt>` followed by
`git worktree remove <wt>` — the worktree directory is archived (with `tar -tzf`
integrity check) before removal. With the fail-closed guards below, the only
files in the worktree at archive time are tracked (committed) files, so the
archive captures everything; the TOCTOU window is microseconds under flock.

Why not `git worktree move`: a moved worktree keeps its administrative entry
and the branch stays bound, so `git branch -D` still refuses (branch used by
worktree at the new path). Move-then-keep-dir-through-grace would leave the
branch ref alive for the grace period, blocking branch-name reuse. Archive +
remove lets us delete the branch immediately while preserving the tree in the
archive through the grace period.

Candidates: worktrees whose bound branch is **eligible for deletion by Lane
H/R rules** (ancestor of trunk OR gh-confirmed), OR detached-HEAD worktrees
whose HEAD is ancestor of trunk. **Lane W runs BEFORE the branch ref is
deleted** (the inversion that fixes the round-3 structural P0).

Per candidate, atomic sequence under `fcntl.flock` on
`$(git rev-parse --git-common-dir)/clean-merged.lock`:

1. Verify eligibility while the branch ref still exists.
2. Refuse if `git -C <wt> ls-files -v` shows assume-unchanged or skip-worktree
   entries (any lowercase letter), unless `--discard-hidden-index-bits`.
3. Refuse if ANY ignored content (default fail-closed). Enumerate via
   `git -C <wt> -c status.showUntrackedFiles=all ls-files --others --ignored
   --exclude-standard -z`. `--discard-ignored` overrides. Walk into ignored
   dirs for nested `.git` detection; refuse unless `--remove-nested-repos`.
4. Revalidate clean: `git -C <wt> -c status.showUntrackedFiles=all status
   --porcelain -z` empty, immediately before the archive (TOCTOU).
5. `tar -czf <quarantine>/worktree.tar.gz -C <wt-parent> <wt-name>` + verify
   with `tar -tzf`. Abort removal if archive fails integrity.
6. `git worktree remove <wt>` (plain; refuses if dirty — final TOCTOU safety
   net). On failure, archive is preserved; operator can intervene.
7. NOW delete the branch ref: timestamped backup ref, then `git branch -D <B>`
   (safe because eligibility was just verified and the worktree is gone).
8. Write `<quarantine>/clean-merged.manifest.json`.

Purge: `just clean-merged --purge-quarantine` removes quarantine dirs older
than `<cfg grace_days>` (default 30). Pre-purge warning for items within 7
days of purge. **Purge only dirs whose manifest records `worktree_remove_ok`.**

## Config — `config/clean-merged.toml` (single source of truth)

Read from the **main worktree** path (not the current worktree, which may be
on a branch predating the config). Missing required keys fail loud; missing
optional keys use documented defaults + warning. See the file for the schema.

## Backup refs

`refs/clean-merged/<branch>-<short-sha>-<unix-ts>` — timestamped + SHA-addressed
(survives branch-name reuse and D/F collisions like feat vs feat/x). These refs
keep objects reachable so they do NOT expire via `gc.reflogExpireUnreachable`.
Explicit pruning via `just clean-merged --prune-backups <days>` (default 30,
aligned to quarantine grace). Recovery: `git branch <name> <sha>`.

## Installation — `just setup`

- Resolve active hooks dir; install additively with markers; abort loudly on
  foreign-content ambiguity.
- Restructure existing `post-checkout` (its `prev_head != 000…` early-exit
  moves BELOW our dispatch so cleanup runs first when gated).
- Extend existing `post-rewrite` (Entire CLI line preserved).
- `git config remote.origin.prune true` owned here (NO DUAL PATHS).
- `post-merge` also spawns Lane R detached.
- `just clean-merged-doctor`: install state, hook-marker presence, config
  validity, gh availability, last-run heartbeat freshness, quarantine disk
  usage, backup-ref count.

## Audit log (JSONL)

Path: `$(git rev-parse --git-common-dir)/clean-merged.log`. Rotation under
`fcntl.flock` at `max_log_bytes`. Each record: `ts, lane, branch, tip_sha,
action, reason, backup_ref, quarantine_path, recovery_hint` (structured
pointer, not literal shell — survives shell-sensitive branch names).

## Contract: what "always-on" actually means

(round-soundness GPT/Kimi/Claude: the original "always-on" framing overclaimed
in two specific ways. Stated precisely:)

- **Always-on per clone, after `just setup`.** The tool is inert in a fresh
  clone until `just setup` runs `git config core.hooksPath .githooks`. Git
  cannot auto-run hooks on clone without local config; there is no in-tree
  bootstrap. `just clean-merged-doctor` reports an unset hooksPath as a problem.
- **Always-on for branch ref cleanup (Lane H + Lane R).** Every `git pull`
  (incl. FF — empirically verified) and `git checkout main` fires the hooks
  and cleans eligible branches automatically.
- **NOT always-on for worktree removal (Lane W).** Lane W is opt-in
  (`--include-worktrees` / `just clean-merged-backlog`). Worktree-bound
  branches that are merged upstream accumulate until the operator runs Lane W
  deliberately. This is the cost of the design's foundational invariant:
  never do irreversible work in a hook. The operator must periodically run
  `just clean-merged-backlog` (dry-run first) to reclaim the worktree backlog.

## Accepted risks

- Lane H only cleans non-worktree-bound branches. Worktree-bound branches
  flow to Lane W (explicit). Cost of never doing irreversible work in a hook.
- **Detached-HEAD worktrees are refused by default** (round-soundness GPT P0 /
  Kimi RECOVERY_HOLE). Scenario defeated: operator makes an exploratory commit
  in a detached worktree, resets back to trunk, then runs Lane W. The worktree's
  HEAD is at trunk (eligible), but the reset-away commit lives only in the
  worktree's reflog. The archive captures the post-reset working tree (NOT the
  orphaned commit); `git worktree remove` deletes the worktree's reflog with
  the admin entry; the commit becomes unreachable. `--allow-detached-removal`
  overrides after accepting that reflog-only commits are not preserved.
- Lane R hook-spawn runs gh per merge (detached, timeout-bounded). Persistent
  gh unavailability → squash-merged branches accumulate until online manual
  reconcile. Documented; no data loss.
- Quarantine grace 30d. After purge, moved tree is gone. Mitigated by
  pre-purge warning + doctor disk-usage report + 30d alignment with
  backup-ref pruning.
- Assume-unchanged/skip-worktree files default-refused; explicit override
  destroys hidden modifications.
- **Silent hook-death detection latency** (round-soundness GPT/Kimi/Claude).
  If `python3` becomes unavailable (brew upgrade removes the symlink, PATH
  change after OS update), the hooks silently no-op via `command -v python3 ||
  exit 0`. The fail-open contract is intentional — a hook that broke `git pull`
  over a missing dep would be strictly worse. The only detector is
  `--doctor`'s heartbeat-freshness check (7-day floor); doctor is opt-in/pull-
  based (no cron/launchd wiring), so real detection latency is 7 days + however
  long until someone runs doctor. The failure cost equals the no-tool baseline
  (merged branches linger; committed work stays reachable via reflog); prior
  deletions are backup-ref protected. Run `just clean-merged-doctor`
  periodically.

## Design provenance

Three external adversarial review rounds (GPT, Kimi, Claude 44-subagent).
Round 3 converged on the lane-ordering inversion (the structural P0: Lane R's
`update-ref -d` bypasses git's worktree guard, bricking worktrees, so Lane W
cannot clean them without `--force`). v4 fixes via Lane-W-owns-the-sequence.

Implementation later revised Lane W's primitive from `git worktree move` to
`tar` + `git worktree remove`: a moved worktree keeps its admin entry, leaving
the branch bound and blocking immediate deletion. Archive-then-remove (under
fail-closed guards + flock + integrity check) preserves the tree while
allowing the branch to be deleted immediately.
