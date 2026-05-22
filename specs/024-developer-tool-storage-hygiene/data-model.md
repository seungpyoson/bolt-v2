# Data Model: Developer-Tool Storage Hygiene

## StorageSurface

Represents a developer-tool path family.

Fields:
- `id`: stable identifier such as `codex_tui_log`.
- `category`: AI agent, version manager, build tool, package manager, IDE/editor, cloud CLI, browser tooling, MCP/plugin, or Python tooling.
- `path_family`: path template such as `~/.codex/sessions/**/*.jsonl`.
- `growth_shape`: `single_file`, `many_files`, `tree`, or `sqlite_with_wal`.
- `native_policy`: `yes`, `partial`, `none_found`, or `not_applicable`.
- `owner`: `this_issue`, `tracked_elsewhere`, `out_of_repo`, or `report_only`.
- `cleanup_mode`: `rotate`, `ttl_prune`, `toolchain_retention`, `preflight_only`, or `none`.
- `protected`: boolean derived from policy and current state.

Validation:
- `owner=this_issue` requires `cleanup_mode` other than `none`.
- `owner=report_only` forbids destructive apply actions.
- `path_family` must be selected from config, not inferred from arbitrary substrings.

## CleanupPolicy

Represents configured #375 retention behavior.

Fields:
- `codex_log.max_bytes`
- `codex_log.retained_rotations`
- `codex_sessions.ttl_days`
- `factory_log.max_bytes`
- `factory_log.retained_rotations`
- `rustup_toolchains.retain_recent`
- `rustup_toolchains.stale_after_days`
- `preflight.warning_bytes`
- `preflight.error_bytes`
- `report_only_surfaces`

Validation:
- Size and day values must be non-negative integers.
- `retained_rotations` and `retain_recent` must be bounded integers.
- Report-only surfaces cannot have apply actions.

## ToolchainState

Represents one installed rustup toolchain.

Fields:
- `name`
- `path`
- `bytes`
- `last_modified`
- `is_project_pinned`
- `is_active`
- `is_default`
- `is_recent_retained`

Validation:
- Any true protection flag prevents removal.
- Candidate status requires stale age, not pinned, not active, not default, and outside retained recent set.

## CleanupCandidate

Represents one dry-run or apply action.

Fields:
- `surface_id`
- `path`
- `action`: `rotate`, `delete_file`, `remove_toolchain`, or `report`
- `bytes_estimate`
- `reason`
- `mode`: `dry_run` or `apply`

Validation:
- `apply` requires prior policy validation and must not target report-only or protected items.
- Candidate path must resolve under its configured path family.

## PreflightReport

Represents read-only storage status.

Fields:
- `total_owned_bytes`
- `available_disk_bytes`
- `warning`
- `error`
- `surfaces`
- `candidates`
- `protected_items`
- `report_only_items`
- `out_of_scope_items`

Validation:
- Preflight must not mutate files.
- Error status must be fail-closed for heavy local verification recommendations.
