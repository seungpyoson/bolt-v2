# Data Model: Developer-Tool Storage Hygiene

## StorageSurface

Represents a developer-tool path family.

Fields:
- `id`: stable identifier such as `codex.log`.
- `category`: AI agent, version manager, build tool, package manager, IDE/editor, cloud CLI, browser tooling, MCP/plugin, or Python tooling.
- `path_family`: path template such as `~/.codex/sessions/**/*.jsonl`.
- `growth_shape`: `single_file`, `many_files`, `tree`, or `sqlite_with_wal`.
- `native_policy`: `yes`, `partial`, `none_found`, or `not_applicable`.
- `owner`: `owned`, `report_only`, or `out_of_scope`.
- `native_guidance`: optional native configuration key family for report-only surfaces such as Codex history.
- `cleanup_mode`: `rotate`, `ttl_prune`, `toolchain_retention`, or `none`.
- `protected`: boolean derived from policy and current state.
- `active_writer_processes`: explicit configured process names that block apply for mutable Codex and Factory surfaces.

Validation:
- Only `owner=owned` surfaces may use mutating cleanup modes.
- `owner=report_only` and `owner=out_of_scope` require `cleanup_mode=none`.
- Adjacent-context entries cannot use `owner=owned`.
- `path_family` must be selected from config, not inferred from arbitrary substrings.
- Enumerated candidates must remain under configured path-family roots after normalization and must not follow symlinks.

## CleanupPolicy

Represents configured #375 retention behavior.

Field names below are logical model names. The TOML source remains authoritative and uses the section names shown in the contract, for example `[codex.log]` maps to `codex_log`.

Fields:
- `codex_log.max_bytes`
- `codex_log.retained_rotations`
- `codex_log.active_writer_processes`
- `codex_sessions.ttl_days`
- `codex_sessions.active_writer_processes`
- `factory_log.max_bytes`
- `factory_log.retained_rotations`
- `factory_log.active_writer_processes`
- `native_guidance.codex_history.max_bytes`
- `native_guidance.codex_history.persistence`
- `rustup_toolchains.retain_exact_names`
- `rustup_toolchains.remove_exact_names`
- `preflight.free_disk_warning_bytes`
- `preflight.free_disk_error_bytes`
- `preflight.owned_storage_warning_bytes`
- `preflight.owned_storage_error_bytes`
- `adjacent`

Validation:
- Size and day values must be non-negative integers.
- `retained_rotations` must be a bounded integer.
- `retain_exact_names` and `remove_exact_names` must use exact installed toolchain names; no wildcard, substring, or pattern matching is allowed.
- Active, default, and repository-root project-pinned rustup protections are unconditional and cannot be disabled by TOML.
- Free-disk error threshold must be less than or equal to free-disk warning threshold.
- Owned-storage error threshold must be greater than or equal to owned-storage warning threshold.
- Report-only, out-of-scope, adjacent, and native-guidance values cannot have apply actions.

## ToolchainState

Represents one installed rustup toolchain.

Fields:
- `name`
- `path`
- `bytes`
- `is_project_pinned`
- `is_active`
- `is_default`
- `is_explicitly_retained`
- `is_explicitly_removable`

Validation:
- Any true protection flag prevents removal.
- Candidate status requires an exact `remove_exact_names` match, not pinned, not active, not default, and not explicitly retained.
- File age and mtime may appear in reports but must never create a rustup removal candidate.
- `is_project_pinned` is derived from the repository-root `rust-toolchain.toml` for this #375 slice.

## ProcessSnapshot

Represents process names used for active-writer refusal.

Fields:
- `process_names`: observed process names supplied by a host collector or synthetic test fixture.

Validation:
- Matching uses exact configured process names from TOML.
- Shell command strings and wrapper/parser semantics are not interpreted.

## CleanupCandidate

Represents one dry-run or apply action.

Fields:
- `surface_id`
- `path`
- `action`: `rotate`, `delete`, `remove_tree`, or `refuse`
- `bytes`
- `estimated_reclaim_bytes`
- `reason`
- `state_token`: filesystem identity used by apply revalidation for mutating candidates.

Validation:
- `apply` requires immediate policy validation and filesystem re-scan before mutation and must not target report-only or protected items.
- Candidate path must resolve under its configured path family.
- Mutable Codex and Factory candidates must be refused when configured active writer processes are detected.

## PreflightReport

Represents read-only storage status.

Fields:
- `owned_storage_bytes`
- `available_disk_bytes`
- `warnings`
- `errors`
- `status`
- `surface_measurements`
- `candidates`
- `protected`
- `report_only`
- `adjacent_context`
- `owned_storage_measurement_errors`
- `follow_up_classes`

Validation:
- Preflight must not mutate files.
- Status is `error` if configured free-disk, owned-storage, or owned-measurement checks fail; otherwise `warning` if warning thresholds trigger, else `ok`.
