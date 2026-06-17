# clean-merged — recovery runbook

How to recover work removed by `clean-merged`. **Read this before purging
quarantine or pruning backup refs.**

## Two recovery surfaces

### 1. Branch refs (Lane H/R) — reflog + backup refs

Lane H/R never bare-`-D`. Before every deletion they write a backup ref:
`refs/clean-merged/<branch>-<short-sha>-<unix-ts>`.

Recover a deleted branch:

```sh
# Find the backup ref
git for-each-ref refs/clean-merged/ | grep <branch>

# Restore under the original name (use the tip-sha from the ref or audit log)
git branch <branch> <tip-sha>
```

Backup refs are pruned by `just clean-merged --prune-backups <days>` (default
30). Until pruned, the commits are reachable and safe from gc.

If the backup ref has already been pruned, the tip is still in the reflog as
an unreachable object until `gc.reflogExpireUnreachable` (default ~30d for
unreachable tips). Find it:

```sh
git reflog --all | grep <tip-sha-prefix>
git fsck --unreachable | grep commit
```

Then restore with `git branch <branch> <sha>`.

### 2. Worktrees (Lane W) — quarantine

Lane W never deletes a worktree directory directly. It runs
`git worktree move <wt> <quarantine>/<branch>-<ts>`, preserving the entire
tree through the grace period.

Locate quarantined worktrees:

```sh
quarantine="$(git rev-parse --git-common-dir)/clean-merged-quarantine"
ls -la "$quarantine"
cat "$quarantine"/<dir>/*.manifest.json   # branch, tip-sha, moved-from, file count
```

Restore a worktree from quarantine (before purge):

```sh
# Re-create the branch from the manifest tip-sha
git branch <branch> <tip-sha>
# Move the worktree back into active use
git worktree add <dest-path> <branch>
# Or simply inspect the quarantine dir in place — it is a full working tree
```

Purge: `just clean-merged --purge-quarantine` removes quarantine dirs older
than `quarantine_grace_days` (default 30). **Once purged, the working tree is
gone.** Files that were never committed (untracked, gitignored) are
irrecoverable after purge — this is why Lane W refuses any ignored content by
default and requires explicit `--discard-ignored`.

## Time windows (defaults; configurable in `config/clean-merged.toml`)

| Surface | Default retention | Governed by |
|---|---|---|
| Backup refs (`refs/clean-merged/…`) | 30 days | `--prune-backups` |
| Quarantine worktrees | 30 days | `--purge-quarantine` / `quarantine_grace_days` |
| Unreachable commits (post prune) | ~30 days | `gc.reflogExpireUnreachable` |

Align the first two to your tolerance; do NOT shorten the unreachable window
without understanding gc.

## If something seems missing

1. Check the audit log: `$(git rev-parse --git-common-dir)/clean-merged.log`
   (JSONL; each record has `recovery_hint` pointing to ref or quarantine).
2. Run `just clean-merged-doctor` — reports quarantine disk usage, backup-ref
   count, last-run heartbeat.
3. If a deletion appears to have bypassed these surfaces (e.g., a worktree
   was bricked by an older buggy run), file an issue; do NOT `--force`
   anything without understanding the state.
