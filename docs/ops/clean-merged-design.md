# clean-merged — design

Issue: #802

Auto-cleanup of merged branches and worktrees. Always-on, agent-agnostic
(works for Claude Code, Codex, Aider, raw `git`), no per-session instruction.

## Problem

Merged PRs leave local git branches and git worktrees. After Mergify merge
waves, local state can also lag behind `<remote>/<configured-trunk>`, so cleanup
may evaluate branches and worktrees against stale remote-tracking refs unless
the operator runs the manual sync lane first.

## Hard constraints

- Universal across every tool that touches git (native git hooks only — no
  agent-specific lifecycle hooks).
- Must not delete unmerged work. Branch refs are reflog-recoverable (~30d for
  unreachable tips via `gc.reflogExpireUnreachable`); worktree filesystem
  removal is **irreversible**.
- Idempotent. Never breaks the git operation that triggered it.
- Repo rules: NO HARDCODES (TOML config), NO DUAL PATHS, NO DEBTS.

## Architecture: lanes split by trust/speed profile

### Lane S — Sync (manual, network-bound, fast-forward-only)

- **Trigger:** manual only via `--sync-main`, normally through
  `just clean-merged-backlog`. Hooks do not fetch or update the configured
  trunk.
- **Dry-run default:** reports that `git fetch --prune <remote>` would run and
  that fast-forward safety would be evaluated after that fetch; it does not
  leave local branch or remote-tracking ref mutations, and it does not claim
  remote freshness from stale remote-tracking refs. When composed with cleanup
  lanes, dry-run fetches the remote trunk into a temporary preview ref, deletes
  that temp ref, and uses the preview SHA so Lane H/R/W can report exact
  cleanup actions and refusals without advancing the configured trunk or deleting
  branches/worktrees. If the preview ref cannot be deleted, Lane S reports a
  refusal and later cleanup lanes do not run from that preview.
- **Apply:** runs `git fetch --prune <remote>`, verifies local trunk is an
  ancestor of the refreshed remote-tracking trunk, refuses a dirty checked-out
  trunk worktree, then advances local trunk with `git merge --ff-only` in the
  trunk worktree (or a CAS `update-ref` if the trunk branch is not checked out
  in any worktree).
- **Refusals:** missing local trunk, missing remote-tracking trunk, fetch
  failure, non-fast-forward, dirty trunk worktree, merge failure, or CAS drift.
  A Lane S apply refusal stops later cleanup lanes so stale refs are not used
  as authority.

### Lane H — Hook (always-on, offline, fast, reflog-safe)

- **Triggers:** `post-merge` (primary; FF verified — see
  `clean-merged-triggers.md`), `post-rewrite` (divergent rebase pull),
  `post-checkout` (shell-gated `$3==1`, then Python-gated
  `target==configured trunk`, best-effort).
- **Synchronous by default.** Typical sweep is sub-second. Detach only if
  profiling demands, with non-blocking `fcntl.flock`, stdin→`/dev/null`,
  stdout/stderr→audit log.
- **Does ONLY:** kill-switch check → cheap candidate precheck → for each
  non-trunk, non-current branch NOT bound to a worktree: re-read tip, check
  `git merge-base --is-ancestor <B> <trunk>` immediately before delete, write
  timestamped backup ref, then CAS-delete `refs/heads/<B>` with
  `git update-ref -d refs/heads/<B> <fresh-sha>`. **No gh. No `-D`.**
- Worktree-bound eligible branches logged as "Lane W candidate."

### Lane R — Reconcile (hook-spawned detached OR manual; network-bound)

- `post-merge` spawns Lane R detached (`setsid` when available, otherwise a
  background subprocess), stdin→`/dev/null`, and stdout/stderr redirected by
  Python to the configured Lane R log path. It uses strict Python
  `subprocess.run(timeout=cfg)` on the gh call and a config-backed cache TTL.
  The git op returns instantly. Manual
  `just clean-merged --reconcile` for on-demand, or
  `just clean-merged-backlog` after merge waves when sync + worktree cleanup
  are both wanted.
- **Skip worktree-bound branches** — those flow to Lane W.
- **Per-branch gh query** (`--limit <cfg gh_limit>` plus exact `headRefOid`
  matching to cover realistic branch reuse without accepting false positives):
  for each non-ancestor, non-worktree-bound branch:
  `gh pr list --head <B> --state merged --json number,headRefOid,baseRefName,headRepositoryOwner,isCrossRepository --limit <cfg gh_limit>`
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
deleted** so worktree-bound refs are removed only by Lane W's archive-first
sequence.

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
7. Re-read the bound branch tip, revalidate that fresh tip with the same
   ancestor/PR authority, then delete the branch ref by timestamped backup ref
   plus CAS `git update-ref -d refs/heads/<B> <fresh-sha>`. If the fresh tip is
   no longer eligible, keep the branch even though the archived worktree was
   removed.
8. Write `<quarantine>/clean-merged.manifest.json`.

Purge: `just clean-merged --purge-quarantine` removes quarantine dirs older
than `<cfg grace_days>` (default 30). **Purge only dirs whose manifest records
`worktree_remove_ok`.**

### Lane T — Target Dirs (explicit; raw-Cargo straggler reaper)

Lane T removes stale worktree-local raw Cargo output directories from linked
worktrees that survive normal branch cleanup. It is explicit only:
`just clean-merged --include-target-dirs` or `clean_merged_artifacts.py --lane t`.
Hooks do not run Lane T.

Candidates are `<worktree>/<cfg target_dir_name>` for linked worktrees other
than the main worktree. A candidate is eligible only when the latest mtime
anywhere inside the subtree is older than `<cfg idle_after_days>`. The top-level
directory mtime is not enough; a fresh build artifact inside an old `target/`
keeps the subtree.

Apply mode refuses rather than deleting when a configured Cargo/Rust process is
visible from `ps` with cwd in the worktree/target tree or command text that
mentions the target dir. If process visibility is unavailable, apply also
refuses. Dry-run reports `target-dir-reap-candidate` records and never deletes.

If `[clean-merged.lane_t]` is absent, Lane T is a no-op. If the table is
present, all Lane T keys are required and validated like the other runtime
config.

## Config — `config/clean-merged.toml` (single source of truth)

Read from the **main worktree** path (not the current worktree, which may be
on a branch predating the config). Missing or unknown runtime keys fail loud;
every enabled lane's runtime value lives in `config/clean-merged.toml`. See the
file for the schema.

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
- `git config remote.<configured-remote>.prune true` owned here (NO DUAL PATHS).
- `post-merge` also spawns Lane R detached.
- `just clean-merged-doctor`: install state, hook-marker presence, config
  validity, gh availability, gh cache health, last-run heartbeat freshness,
  quarantine disk usage, backup-ref count, and rotated-log usage.

## Audit log (JSONL)

Path: `$(git rev-parse --git-common-dir)/clean-merged.log`. Rotation under
`fcntl.flock` at `max_log_bytes`; rotated audit and Lane R log segments are
retained for `rotated_log_retention_days` and reported by `--doctor`. Each
record: `ts, lane, branch, tip_sha, action, reason, backup_ref,
quarantine_path, recovery_hint` (structured pointer, not literal shell —
survives shell-sensitive branch names). Subprocess diagnostics stored in
`reason` fields are secret-redacted first and then bounded by
`report_error_max_chars`.

## Contract: what "always-on" actually means

The "always-on" contract has two precise limits:

- **Always-on per clone, after `just setup`.** The tool is inert in a fresh
  clone until `just setup` runs `git config core.hooksPath .githooks`. Git
  cannot auto-run hooks on clone without local config; there is no in-tree
  bootstrap. `just clean-merged-doctor` reports an unset hooksPath as a problem.
- **Always-on for branch ref cleanup (Lane H + Lane R).** Every `git pull`
  (incl. FF — empirically verified) and checkout of the configured trunk fires
  the hooks and cleans eligible branches automatically.
- **NOT always-on for worktree removal (Lane W).** Lane W is opt-in
  (`--include-worktrees` / `just clean-merged-backlog`). Worktree-bound
  branches that are merged upstream accumulate until the operator runs Lane W
  deliberately. This is the cost of the design's foundational invariant:
  never do irreversible work in a hook. The operator must periodically run
  `just clean-merged-backlog` (dry-run first) to reclaim the worktree backlog.
- **NOT always-on for target-dir reaping (Lane T).** Lane T is opt-in
  (`--include-target-dirs` / `--lane t`) because it removes ignored build
  output from still-surviving worktrees. Run dry-run first, then pass
  `--apply` only after reviewing candidates.
- **NOT always-on for remote fetch/prune or local trunk sync.** Lane S is
  manual because fetching/pruning and moving local trunk are network/ref
  mutations. After Mergify merge waves, `just clean-merged-backlog` is the
  existing cleanup path that composes Lane S, Lane R, and Lane W.
- **Detached-HEAD worktrees require an additional explicit override.** Even
  inside Lane W (`--include-worktrees`), detached-HEAD worktrees are REFUSED
  by default. The operator must pass `--allow-detached-removal` to override,
  accepting that reflog-only commits in the detached worktree are NOT
  preserved by the archive. See "Accepted risks" below for the recovery hole
  this closes.

## Accepted risks

- Lane H only cleans non-worktree-bound branches. Worktree-bound branches
  flow to Lane W (explicit). Cost of never doing irreversible work in a hook.
- **Detached-HEAD worktrees are refused by default**. Scenario defeated:
  operator makes an exploratory commit in a detached worktree, resets back to
  trunk, then runs Lane W. The worktree's HEAD is at trunk (eligible), but the
  reset-away commit lives only in the worktree's reflog. The archive captures
  the post-reset working tree (NOT the orphaned commit); `git worktree remove`
  deletes the worktree's reflog with the admin entry; the commit becomes
  unreachable. `--allow-detached-removal` overrides after accepting that
  reflog-only commits are not preserved.
- Lane R hook-spawn runs gh per merge (detached, timeout-bounded). Persistent
  gh unavailability → squash-merged branches accumulate until online manual
  reconcile. Documented; no data loss.
- Quarantine grace 30d. After purge, moved tree is gone. Mitigated by
  doctor disk-usage report + 30d alignment with backup-ref pruning.
- Assume-unchanged/skip-worktree files default-refused; explicit override
  destroys hidden modifications.
- **Silent hook-death detection latency**. If `python3` becomes unavailable
  (brew upgrade removes the symlink, PATH change after OS update), the hooks
  silently no-op via `command -v python3 || exit 0`. If `python3` exists but
  lacks stdlib `tomllib` (Python <3.11), the script fail-opens for hook lanes
  and `--doctor` reports `tomllib=no` with the Python 3.11+ requirement. The
  fail-open contract is intentional — a hook that broke `git pull` over a
  missing dep would be strictly worse. Detection still depends on `--doctor`'s
  heartbeat-freshness check (configured heartbeat stale threshold (default 7
  days)); doctor is opt-in/pull-based (no cron/launchd wiring), so real
  detection latency is the configured heartbeat stale threshold (default 7
  days) + however long until someone runs doctor. The failure cost equals the
  no-tool baseline (merged branches linger; committed work stays reachable via
  reflog); prior deletions are backup-ref protected. Run
  `just clean-merged-doctor` periodically.
- **Lane R gh cost under slowdown**. Lane R is spawned detached on every
  `post-merge`; each non-ancestor branch triggers a per-branch gh query
  (config-backed timeout each). On a repo with ~40 branches during a gh
  slowdown, background work could last several minutes per merge. The git op
  returns instantly (detached); the cost is background CPU + potential gh
  rate-limit pressure. Mitigated by per-branch TTL cache + the fail-safe "gh
  trouble → keep branch" contract. If gh is persistently slow, squash-merged
  branches accumulate until an online manual reconcile. No data loss.
- **Invalid or future-dated gh cache entries fail closed.** A cache entry with
  malformed PR payloads, non-finite timestamps, or `fetched_at` in the future
  keeps the branch and avoids a fresh gh call for that branch until a later
  successful save prunes the invalid entry. Whole-file cache corruption also
  keeps branches and is surfaced by `--doctor` with the cache path so the
  operator can delete the file if needed. This favors no false deletes over
  cache self-healing.
- **No `fsync` before `os.replace`**. Atomic manifest/cache writes use
  `tmp.write_text()` + `os.replace()` without an intervening
  `fsync`. A power-loss between write and replace could lose the tmp file's
  content on some filesystems. Standard tradeoff for this class of tool; the
  target file is either the previous content or the new content (atomic rename
  guarantee), never partial. The worst case is a lost manifest update, not
  corruption. Accepted.
- **`has_nested_git` not re-checked at Lane W TOCTOU point**. The hidden-bits,
  ignored-content, and dirty guards are all re-run inside the `if apply:`
  block; `has_nested_git` is not (it walks the full worktree and doubling the
  walk cost wasn't justified for the implausible race — someone cloning a repo
  into the worktree mid-sweep). The upfront check covers the common case.
  Accepted.
- **TOCTOU under `--discard-ignored`**. In default mode, the tool refuses ANY
  ignored content upfront (fail-closed). Under `--discard-ignored` (explicit
  operator opt-in), a microsecond window exists between tar-finish and
  `git worktree remove` where a new ignored file could appear and be deleted
  without being captured in the archive. The operator explicitly accepted
  destructive treatment of ignored content by passing the flag. Accepted.
