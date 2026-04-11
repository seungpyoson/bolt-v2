# Log Sweep at Launch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move stale NT runtime logs from the repo root into `var/logs/` at binary launch, so the working directory stays clean.

**Architecture:** A single `log_sweep` module with one public function `sweep_stale_logs()`, called from `main.rs` before kernel init. Uses string matching (no regex crate) to identify NT log files, `std::fs::rename` with EXDEV fallback.

**Tech Stack:** Rust std only (fs, path, io). No new dependencies.

---

### Task 1: Write `is_nt_log_filename` with tests

**Files:**
- Create: `src/log_sweep.rs`
- Modify: `src/lib.rs` (add `pub mod log_sweep;`)
- Create: `tests/log_sweep.rs`

- [ ] **Step 1: Create `src/log_sweep.rs` with the filename matcher**

```rust
const LOG_TARGET_DIR: &str = "var/logs";

/// Returns true if the filename matches the NautilusTrader log naming convention:
/// `{anything}_{YYYY-MM-DD}_{anything}.log`
fn is_nt_log_filename(name: &str) -> bool {
    if !name.ends_with(".log") {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes.len() < 15 {
        return false;
    }
    // Scan for _YYYY-MM-DD_ pattern
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

- [ ] **Step 2: Register the module in `src/lib.rs`**

Add after `pub mod lake_batch;`:
```rust
pub mod log_sweep;
```

- [ ] **Step 3: Write tests in `tests/log_sweep.rs`**

```rust
use bolt_v2::log_sweep;

#[test]
fn matches_standard_nt_log() {
    assert!(log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_e21185e1-8222-454d-ac77-62f3dad8e95c.log"
    ));
}

#[test]
fn matches_different_trader_id() {
    assert!(log_sweep::is_nt_log_filename(
        "TRADER-XYZ_2025-01-01_abcdef.log"
    ));
}

#[test]
fn rejects_no_date_pattern() {
    assert!(!log_sweep::is_nt_log_filename("application.log"));
}

#[test]
fn rejects_non_log_extension() {
    assert!(!log_sweep::is_nt_log_filename(
        "BOLT-001_2026-04-11_uuid.txt"
    ));
}

#[test]
fn rejects_date_without_surrounding_underscores() {
    assert!(!log_sweep::is_nt_log_filename("2026-04-11.log"));
}

#[test]
fn rejects_too_short() {
    assert!(!log_sweep::is_nt_log_filename("a.log"));
}

#[test]
fn rejects_empty() {
    assert!(!log_sweep::is_nt_log_filename(""));
}
```

Note: `is_nt_log_filename` is currently private. Make it `pub` for testing:
```rust
pub fn is_nt_log_filename(name: &str) -> bool {
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test log_sweep -v`
Expected: All 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/log_sweep.rs src/lib.rs tests/log_sweep.rs
git commit -m "feat(log_sweep): add NT log filename matcher with tests (#113)"
```

---

### Task 2: Write `sweep_stale_logs` with tests

**Files:**
- Modify: `src/log_sweep.rs`
- Modify: `tests/log_sweep.rs`

- [ ] **Step 1: Add `sweep_stale_logs` to `src/log_sweep.rs`**

```rust
use std::fs;
use std::path::Path;

const LOG_TARGET_DIR: &str = "var/logs";

/// Move stale NautilusTrader log files from the current directory into `var/logs/`.
///
/// Called before kernel init so only previous runs' logs exist. Errors are
/// logged to stderr and swallowed — a failed sweep must never prevent startup.
pub fn sweep_stale_logs() {
    if let Err(e) = sweep_stale_logs_inner() {
        eprintln!("log_sweep: {e}");
    }
}

fn sweep_stale_logs_inner() -> Result<(), std::io::Error> {
    let target = Path::new(LOG_TARGET_DIR);
    fs::create_dir_all(target)?;

    let mut moved = 0u32;
    let entries = fs::read_dir(".")?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("log_sweep: skipping unreadable entry: {e}");
                continue;
            }
        };

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !is_nt_log_filename(&name) {
            continue;
        }

        let dest = target.join(&*file_name);
        if dest.exists() {
            eprintln!("log_sweep: skipping {name} (already exists in {LOG_TARGET_DIR})");
            continue;
        }

        let source = entry.path();
        match fs::rename(&source, &dest) {
            Ok(()) => moved += 1,
            // EXDEV (18 on Linux/macOS): cross-filesystem rename not supported
            Err(e) if e.raw_os_error() == Some(18) => {
                // Cross-filesystem: copy then remove
                match fs::copy(&source, &dest).and_then(|_| fs::remove_file(&source)) {
                    Ok(()) => moved += 1,
                    Err(e) => eprintln!("log_sweep: failed to copy+remove {name}: {e}"),
                }
            }
            Err(e) => eprintln!("log_sweep: failed to move {name}: {e}"),
        }
    }

    if moved > 0 {
        eprintln!("log_sweep: moved {moved} file(s) to {LOG_TARGET_DIR}/");
    }

    Ok(())
}
```

- [ ] **Step 2: Add sweep integration tests to `tests/log_sweep.rs`**

```rust
use std::fs;
use std::path::Path;

#[test]
fn sweep_moves_matching_logs_to_target() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();

    // Plant fake NT log files
    let log1 = "BOLT-001_2026-04-10_aaaa-bbbb.log";
    let log2 = "BOLT-001_2026-04-11_cccc-dddd.log";
    fs::write(cwd.join(log1), "log content 1").unwrap();
    fs::write(cwd.join(log2), "log content 2").unwrap();

    // Plant a non-matching file that should NOT be moved
    let keep = "readme.log";
    fs::write(cwd.join(keep), "not a NT log").unwrap();

    // Run sweep from the temp dir
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(cwd).unwrap();
    bolt_v2::log_sweep::sweep_stale_logs();
    std::env::set_current_dir(original_dir).unwrap();

    // Verify logs moved
    let target = cwd.join("var/logs");
    assert!(target.join(log1).exists(), "log1 should be in var/logs/");
    assert!(target.join(log2).exists(), "log2 should be in var/logs/");

    // Verify non-matching file stayed
    assert!(cwd.join(keep).exists(), "non-matching file should remain");

    // Verify originals removed from root
    assert!(!cwd.join(log1).exists(), "log1 should not be in root");
    assert!(!cwd.join(log2).exists(), "log2 should not be in root");
}

#[test]
fn sweep_skips_existing_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();

    let log_name = "BOLT-001_2026-04-10_aaaa.log";
    fs::write(cwd.join(log_name), "new content").unwrap();

    // Pre-create the destination
    let target = cwd.join("var/logs");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(log_name), "old content").unwrap();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(cwd).unwrap();
    bolt_v2::log_sweep::sweep_stale_logs();
    std::env::set_current_dir(original_dir).unwrap();

    // Original should still exist (not moved because dest exists)
    assert!(cwd.join(log_name).exists());
    // Destination should have old content (not overwritten)
    assert_eq!(fs::read_to_string(target.join(log_name)).unwrap(), "old content");
}

#[test]
fn sweep_noop_when_no_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();

    fs::write(cwd.join("not_a_log.txt"), "hello").unwrap();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(cwd).unwrap();
    bolt_v2::log_sweep::sweep_stale_logs();
    std::env::set_current_dir(original_dir).unwrap();

    // var/logs/ should be created but empty
    let target = cwd.join("var/logs");
    assert!(target.exists(), "var/logs/ should be created");
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}
```

- [ ] **Step 3: Run all log_sweep tests**

Run: `cargo test --test log_sweep -- --test-threads=1`

Note: `--test-threads=1` because tests change CWD which is process-global.

Expected: All tests PASS (7 unit + 3 integration).

- [ ] **Step 4: Commit**

```bash
git add src/log_sweep.rs tests/log_sweep.rs
git commit -m "feat(log_sweep): add sweep_stale_logs with integration tests (#113)"
```

---

### Task 3: Wire into `main.rs` and update `.gitignore`

**Files:**
- Modify: `src/main.rs:58` (add sweep call)
- Modify: `.gitignore` (add `var/logs/`)

- [ ] **Step 1: Add sweep call to `main.rs`**

In `src/main.rs`, inside `Command::Run { config }` branch, add as the first line before `Config::load`:

```rust
Command::Run { config } => {
    bolt_v2::log_sweep::sweep_stale_logs();
    let cfg = Config::load(&config)?;
```

- [ ] **Step 2: Add `var/logs/` to `.gitignore`**

Add after the existing log pattern line:

```
var/logs/
```

- [ ] **Step 3: Run full test suite**

Run: `cargo test --test-threads=1`

Expected: All tests PASS. The `--test-threads=1` is needed because log_sweep tests change CWD.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- -D warnings`

Expected: No warnings.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs .gitignore
git commit -m "feat(log_sweep): wire sweep into startup and gitignore var/logs (#113)"
```

---

### Task 4: Manual verification and cleanup

- [ ] **Step 1: Verify sweep works on actual log files**

Run: `cargo run -- run -c config/live.toml 2>&1 | head -5`

(This will fail to connect to Polymarket but will create a log file and trigger the sweep on next run.)

Run again: `cargo run -- run -c config/live.toml 2>&1 | head -5`

Check: `ls var/logs/` should contain the first run's log file. Root should have only the second run's log.

- [ ] **Step 2: Clean up existing log files in repo root**

Move the 8 existing root-level log files manually:

```bash
mkdir -p var/logs
mv BOLT-*.log var/logs/
```

- [ ] **Step 3: Verify worktree logs too**

If the chainlink worktree is still active, run the same manual move there.

- [ ] **Step 4: Final commit if any cleanup**

Only if Step 2/3 produced gitignore or other tracked changes.
