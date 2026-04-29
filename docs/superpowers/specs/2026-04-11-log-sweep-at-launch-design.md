# Log Sweep at Launch — Design Spec

**Issue:** #113
**Date:** 2026-04-11
**Status:** Approved

## Problem

NautilusTrader's kernel hardcodes `FileWriterConfig::default()` (`crates/system/src/kernel.rs:199`),
which sets `directory: None`. The `FileWriter` then writes to `PathBuf::new()` — the current
working directory. Every process launch creates a `{trader_id}_{YYYY-MM-DD}_{uuid}.log` file
(~12KB startup snapshot) in the repo root. These accumulate unboundedly.

PR #74 gitignored the files so they don't get committed, but they still clutter `ls` output.
Every other bolt-v2 runtime artifact (raw capture, catalog, audit spool) has a config-driven
home. NT logs are the only exception, and NT's public API doesn't expose the directory setting.

## Design

### Approach

A pre-flight sweep function in the binary that runs before kernel init. On each launch, it
moves stale log files from CWD into `var/logs/` within the repo. The current run's log file
doesn't exist yet at sweep time, so only previous runs' logs are moved.

### Module

New file: `src/log_sweep.rs`

Single public function:

```rust
pub fn sweep_stale_logs()
```

### Behavior

1. `create_dir_all("var/logs/")` — idempotent, creates the target directory.
2. `read_dir(".")` — scan the current working directory.
3. For each entry:
   - Skip if not `is_file()` (ignore symlinks, directories).
   - Skip if filename doesn't match the NT log naming pattern (see Pattern Matching below).
   - Build destination: `Path::new("var/logs").join(entry.file_name())`.
   - Skip with stderr warning if destination already exists (don't overwrite).
   - `std::fs::rename(source, destination)`.
   - On `EXDEV` (cross-filesystem): fallback to `std::fs::copy` + `std::fs::remove_file`.
   - On any other error: log to stderr and continue.
4. Print summary to stderr: `"log_sweep: moved N file(s) to var/logs/"` (only if N > 0).

### Pattern Matching

No `regex` crate (not in dependency tree). Use string checks on the filename:

- Must end with `.log`
- Must contain a `_YYYY-MM-DD_` substring: scan for `_` followed by 4 digits, `-`, 2 digits, `-`, 2 digits, `_`

This matches the NT naming convention `{anything}_{date}_{anything}.log` regardless of
trader ID. It won't match arbitrary `.log` files that don't have the date pattern.

```rust
fn is_nt_log_filename(name: &str) -> bool {
    if !name.ends_with(".log") {
        return false;
    }
    // Look for _YYYY-MM-DD_ pattern
    let bytes = name.as_bytes();
    if bytes.len() < 15 {
        return false; // too short to contain pattern + .log
    }
    for window in bytes.windows(12) {
        if window[0] == b'_'
            && window[1..5].iter().all(|b| b.is_ascii_digit())
            && window[5] == b'-'
            && window[6..8].iter().all(|b| b.is_ascii_digit())
            && window[8] == b'-'
            && window[9..11].iter().all(|b| b.is_ascii_digit())
            && window[11] == b'_'
        {
            return true;
        }
    }
    false
}
```

### Call Site

`src/main.rs`, at the top of the `Command::Run` branch, before `Config::load`:

```rust
Command::Run { config } => {
    bolt_v2::log_sweep::sweep_stale_logs();
    let cfg = Config::load(&config)?;
    // ... rest of startup
}
```

### Gitignore

Add `var/logs/` to `.gitignore`. The existing root-level log pattern stays
(catches logs between runs when the sweep hasn't run yet).

### Constants

```rust
const LOG_TARGET_DIR: &str = "var/logs";
```

Not in TOML config. This is a project convention for runtime artifact placement,
parallel to `var/raw` (raw capture default). The CLAUDE.md "NO HARDCODES" rule
applies to runtime values — this is a build-time project layout constant.

## What This Does NOT Do

- **No deletion.** All logs are preserved indefinitely for forensic value.
- **No date subdirectories.** Volume is too low to warrant it (~12KB per file).
- **No config knob.** The target directory is a constant, not configurable.
- **No upstream changes.** We don't modify NT or fork it.
- **No external scheduling.** No cron, no launchd, no wrapper scripts.

## Edge Cases

| Scenario | Behavior |
|---|---|
| First run ever (no stale logs) | `create_dir_all` runs, `read_dir` finds nothing, no-op |
| `var/logs/` already has a file with same name | Skip with stderr warning |
| Cross-filesystem mount | Fallback to copy + delete |
| Permission denied on `var/logs/` | Log error, continue startup |
| CWD is not the repo root | Sweeps whatever CWD is — same as where NT writes |
| Two instances from same CWD | Second sweep may move first's active log; rename succeeds on Unix (inode tracking), errors swallowed either way |

## Test Plan

- Unit test `is_nt_log_filename()` with positive and negative cases
- Unit test `sweep_stale_logs()` using a temp directory with planted log files
- Manual: run binary twice, verify first run's log moved to `var/logs/`
- Manual: verify `ls *.log` in repo root shows at most 1 file after launch
