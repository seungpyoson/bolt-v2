# Data Model: Managed Rust Cache Retention

## CacheStatus

- `repo`: repository path
- `policy`: policy path
- `target_dir`: managed target root
- `filesystem`: free/used/total bytes for containing filesystem
- `thresholds`: `min_free_bytes` and `soft_limit_bytes` from policy when configured
- `pressure`: whether total managed cache exceeds `soft_limit_bytes` or filesystem free bytes are below `min_free_bytes`
- `pressure_reasons`: machine-readable explanations for pressure state
- `total_bytes`: total managed target allocated disk bytes, measured compatibly with `du -sk`
- `subtrees`: list of `CacheSubtree`
- `skipped_special_entries`: count of sockets, FIFOs, devices, or other special entries skipped by the scanner

## CacheSubtree

- `path`: absolute path
- `relative_path`: path relative to managed target root
- `class`: `debug`, `release`, `cross-target`, managed-root `tmp`, or `other`
- `bytes`: subtree allocated disk bytes, measured compatibly with `du -sk`
- `latest_mtime`: latest modification time seen in subtree
- `candidate`: whether prune policy marks it removable
- `reason`: human-readable candidate reason

`tmp` is limited to `<managed-target-root>/tmp`. `/private/tmp/bolt-v2-*` is not part of this data model. `other` is preserved unless policy explicitly marks it prunable.

Class rules:

- `debug`, `release`, and managed-root `tmp` are direct managed-target-root children with those exact names.
- `cross-target` is a direct child whose name has at least three non-empty hyphen-separated components, matching normal Rust target triples.
- `other` is any remaining direct child.

Scanner rules: use `du -sk` for direct subtree allocated disk bytes, use `lstat` for metadata, never follow symlinks, and skip sockets, FIFOs, devices, and other special entries.

## PrunePlan

- `dry_run`: boolean
- `target_dir`: managed target root
- `pressure`: copied from `CacheStatus`
- `pressure_reasons`: copied from `CacheStatus`
- `candidates`: list of `CacheSubtree`
- `reclaimable_bytes`: total allocated disk bytes for candidates
- `refused`: boolean
- `refusal_code`: machine-readable code such as `active_process`, `invalid_policy`, `missing_policy`, or `insufficient_process_visibility`
- `refusal_reason`: human-readable explanation present when active process, invalid policy, missing policy, or insufficient process visibility blocks apply

## ActiveProcess

- `pid`
- `command`
- `cwd`: process working directory when inspectable
- `reason`: why it is considered related to repo/cache

Active-process matching patterns come from `ci/rust-verification.toml`, not from hidden code constants. A process is related when its command matches a configured pattern and its cwd or argv references the repo root or managed target root. Linux cwd inspection uses `/proc/<pid>/cwd`; platforms without cwd visibility still fail closed when argv does not prove scope. If a matching process exists but cwd/argv cannot be inspected, apply mode fails closed with `insufficient_process_visibility`.
