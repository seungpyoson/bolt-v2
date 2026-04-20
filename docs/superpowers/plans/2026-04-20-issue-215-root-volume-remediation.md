# Issue #215 Root-Volume Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use `- [ ]` for tracking.

**Goal:** Make root-volume exhaustion impossible by design. Move runtime writes to a dedicated data volume, refuse relative runtime paths at validation, stop NT file logging, cap journald growth, and ship the systemd + host-setup artifacts as single-source-of-truth in the repo.

**Architecture:** `deploy/` owns the systemd unit, the journald drop-in, and the host installer. The Rust validator rejects `audit.local_dir`, `raw_capture.output_dir`, and `streaming.catalog_path` (when non-empty) unless absolute, and rejects `logging.file_level != "Off"`. NT file-logging plumbing is removed from `main.rs` because NT's kernel calls `FileWriterConfig::default()` (empty directory) — any non-Off setting writes to cwd regardless. `src/log_sweep.rs` goes with it; it only existed to compensate for that behavior. All fixtures move to absolute paths under `/srv/bolt-v2`. `tests/deploy_assets.rs` locks the systemd directives so they cannot silently regress.

**Tech Stack:** Rust 2024, NautilusTrader pin `af2aefc`, cargo-nextest, cargo-deny, cargo-zigbuild (aarch64-unknown-linux-gnu), systemd, ext4 on EBS, Ubuntu 22.04 aarch64.

**Root-cause chain being eliminated** (from the 2026-04-20 postmortem):

1. `bolt-v2.service` had no `WorkingDirectory` → cwd was `/`.
2. NT file logging on (`file_level=Debug`) + NT kernel passes `FileWriterConfig::default()` (no directory) → logs land in cwd.
3. `src/log_sweep.rs` swept stale NT logs into relative `var/logs` → `/var/logs`.
4. `audit.local_dir = "var/audit"`, `raw_capture.output_dir = "var/raw"` → `/var/audit`, `/var/raw`.
5. stdout/stderr → journald/syslog uncapped (~2 GiB each).
6. SSM `RunShellScript` stdout retained in `/var/lib/amazon/ssm/` (~714 MiB).
7. 8 GiB root → ENOSPC → SSM control plane broke.

**Out of scope:**

- Cutting the broken host over. Forensic resources (`i-08dee6aefe9a5b02c`, `i-0bd63c88cd82e3b35`, `snap-0c5f98e69aa4ac0bd`, `vol-0d24ceef69cd27b33`) remain untouched.
- Audit S3 destination correctness (separate fail-close issue; secondary for disk per postmortem).

---

## File Structure

**New:**
- `deploy/systemd/bolt-v2.service`
- `deploy/systemd/journald-bolt-v2.conf`
- `deploy/install.sh` (executable)
- `deploy/README.md`
- `tests/deploy_assets.rs`
- `docs/superpowers/plans/2026-04-20-issue-215-root-volume-remediation.md` (this file)

**Modified:**
- `src/validate.rs` — reject relative runtime paths + non-Off file_level in both `validate_live_local` and `validate_runtime`; add missing `raw_capture.output_dir` validation.
- `src/validate/tests.rs` — coverage for the new rejection rules + fixture updates.
- `src/config.rs` — `default_raw_capture_output_dir` → `/srv/bolt-v2/var/raw`.
- `src/main.rs` — drop `sweep_stale_logs()` call; drop `fileout_level` from `LoggerConfig`.
- `src/lib.rs` — drop `pub mod log_sweep;`.
- `src/platform/runtime.rs` — audit fixture at line 1514 becomes absolute; anything else grep surfaces.
- `src/live_config.rs` — any test-body TOML using `local_dir = "var/audit"` becomes absolute.
- `config/live.local.example.toml` — `file_level = "Off"`, absolute paths.
- Every test fixture enumerated in Task 3 Step 3.2 / Step 3.3.

**Deleted:**
- `src/log_sweep.rs`
- `tests/log_sweep.rs`
- `BOLT-001_2026-04-20_031fc0d6-f752-4e0d-b65e-a58710866938.log` (stray artifact at worktree root)

**Superseded docs (banner added):**
- `docs/superpowers/specs/2026-04-11-log-sweep-at-launch-design.md`
- `docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md`

**Not touched:** `config/operator-snapshots/2026-04-16/live.local.toml` — frozen historical snapshot, not loaded by any test (verified by grep).

---

## Task 1: Ship repo-owned deploy artifacts

**Files:**
- Create `deploy/systemd/bolt-v2.service`
- Create `deploy/systemd/journald-bolt-v2.conf`
- Create `deploy/install.sh`
- Create `deploy/README.md`

- [ ] **Step 1.1: Write `deploy/systemd/bolt-v2.service`**

```ini
[Unit]
Description=bolt-v2 Polymarket LiveNode
After=network-online.target srv-bolt\x2dv2.mount
Wants=network-online.target
RequiresMountsFor=/srv/bolt-v2

[Service]
Type=simple
User=bolt
Group=bolt
WorkingDirectory=/srv/bolt-v2
ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal
ReadWritePaths=/srv/bolt-v2
ProtectSystem=strict
ProtectHome=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 1.2: Write `deploy/systemd/journald-bolt-v2.conf`**

```ini
# /etc/systemd/journald.conf.d/ drop-in capping journald growth on the bolt-v2 host.
[Journal]
SystemMaxUse=500M
SystemMaxFileSize=50M
MaxRetentionSec=7day
ForwardToSyslog=no
```

- [ ] **Step 1.3: Write `deploy/install.sh`** (executable)

Script contract:
- Env: `BOLT_DATA_DEVICE` (required) — block device to back `/srv/bolt-v2`.
- Idempotent: safe to re-run. Skips `mkfs.ext4` if device already has a filesystem; skips `useradd` if user exists; skips fstab append if label already present; skips `mount` if mountpoint is already mounted.
- Creates `bolt` system user, ensures `/srv/bolt-v2/var/{raw,audit,state}` exist chowned `bolt:bolt`.
- Installs `deploy/systemd/bolt-v2.service` → `/etc/systemd/system/bolt-v2.service`.
- Installs `deploy/systemd/journald-bolt-v2.conf` → `/etc/systemd/journald.conf.d/bolt-v2.conf`.
- `systemctl daemon-reload && systemctl restart systemd-journald && systemctl enable bolt-v2.service`.
- Does not start `bolt-v2.service`; operator starts it after verifying binary + config are in place.

Use `LABEL=bolt-v2-data` for fstab mount identity (stable across device renames). fstab line: `LABEL=bolt-v2-data /srv/bolt-v2 ext4 defaults,nofail,x-systemd.device-timeout=30s 0 2`.

Set `set -euo pipefail` at the top. Use `install -m 0644` for the unit files.

- [ ] **Step 1.4: Write `deploy/README.md`**

Must include:
- Host layout (`/opt/bolt-v2/` binary+config on root volume; `/srv/bolt-v2/var/...` on data volume).
- Provisioning runbook: launch from AMI `ami-037d87f13f7e014c5`, attach EBS data volume, upload binary + rendered config, run `sudo BOLT_DATA_DEVICE=/dev/nvme1n1 bash deploy/install.sh`, then `sudo systemctl start bolt-v2`.
- Verification steps: `journalctl -u bolt-v2 -n 200 --no-pager`, `df -h /srv/bolt-v2`, `df -h /`.
- SSM hygiene: every `send-command` must pass `--output-s3-bucket-name` + `--output-s3-key-prefix`. Document that this is how 714 MiB accumulated in `/var/lib/amazon/ssm` during the incident.
- Why no file logging: NT kernel constructs `FileWriterConfig::default()` with no directory; file logs otherwise land in cwd. Validator rejects non-Off. Use `stdout_level` + `journalctl`.
- Forensic resources currently preserved (list the four IDs).

- [ ] **Step 1.5: Mark install.sh executable**

Run: `chmod +x deploy/install.sh`.

- [ ] **Step 1.6: Compile-sanity check**

Run: `cargo check --all-targets`
Expected: success (no Rust changes yet; confirms the new files don't break anything).

- [ ] **Step 1.7: Commit**

```bash
git add deploy/ docs/superpowers/plans/2026-04-20-issue-215-root-volume-remediation.md
git commit -m "feat(deploy): add repo-owned systemd unit, journald cap, and installer

Ships the single source of truth for the bolt-v2 host: WorkingDirectory
=/srv/bolt-v2, StandardOutput=journal, journald SystemMaxUse=500M, and
an idempotent install.sh that provisions the data-volume mount plus
the chowned runtime dirs. Scope: #215 host storage layout."
```

---

## Task 2: Integration test locking the deploy directives

**Files:** Create `tests/deploy_assets.rs`.

- [ ] **Step 2.1: Write the test file**

```rust
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("git should resolve repo root");
    assert!(output.status.success(), "git rev-parse failed");
    PathBuf::from(
        String::from_utf8(output.stdout).expect("utf-8").trim().to_string(),
    )
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn unit_pins_working_directory() {
    assert!(read("deploy/systemd/bolt-v2.service")
        .contains("WorkingDirectory=/srv/bolt-v2"));
}

#[test]
fn unit_logs_to_journal() {
    let c = read("deploy/systemd/bolt-v2.service");
    assert!(c.contains("StandardOutput=journal"));
    assert!(c.contains("StandardError=journal"));
}

#[test]
fn unit_runs_as_bolt_user() {
    let c = read("deploy/systemd/bolt-v2.service");
    assert!(c.contains("User=bolt"));
    assert!(c.contains("Group=bolt"));
}

#[test]
fn unit_execstart_points_to_opt_bolt_v2() {
    assert!(read("deploy/systemd/bolt-v2.service").contains(
        "ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml"
    ));
}

#[test]
fn journald_drop_in_caps_growth() {
    let c = read("deploy/systemd/journald-bolt-v2.conf");
    assert!(c.contains("SystemMaxUse=500M"));
    assert!(c.contains("SystemMaxFileSize=50M"));
    assert!(c.contains("MaxRetentionSec=7day"));
}

#[test]
fn install_script_targets_srv_bolt_v2() {
    let c = read("deploy/install.sh");
    assert!(c.contains("BOLT_DATA_DEVICE"));
    assert!(c.contains("/srv/bolt-v2"));
    assert!(c.contains("systemctl enable bolt-v2.service"));
}
```

- [ ] **Step 2.2: Run**

Run: `cargo nextest run --test deploy_assets`
Expected: all 6 tests pass.

- [ ] **Step 2.3: Commit**

```bash
git add tests/deploy_assets.rs
git commit -m "test(deploy): lock systemd + journald directives against drift

Source-of-truth assertions on deploy/ so the 2026-04-20 remediation
commitments (WorkingDirectory, journal output, journald cap) cannot
silently regress. Scope: #215."
```

---

## Task 3: Move every fixture to absolute paths + `file_level = "Off"`

> Fixtures migrate before the validator tightens, so each commit leaves the suite green.

**Files modified (enumerated in steps):** `config/live.local.example.toml`, `tests/support/mod.rs`, `tests/config_parsing.rs`, `tests/config_schema.rs`, `tests/render_live_config.rs`, `tests/cli.rs`, `tests/live_node_run.rs`, `tests/eth_chainlink_taker_runtime.rs`, `tests/platform_runtime.rs`, `tests/reference_pipeline.rs`, `src/validate/tests.rs`, `src/live_config.rs`, `src/platform/runtime.rs`.

- [ ] **Step 3.1: Update the operator example config**

In `config/live.local.example.toml`:
- `file_level = "Debug"` → `file_level = "Off"`.
- `output_dir = "var/raw"` → `output_dir = "/srv/bolt-v2/var/raw"`.
- `local_dir = "var/audit"` → `local_dir = "/srv/bolt-v2/var/audit"`.

- [ ] **Step 3.2: Replace `file_level = "Debug"` in all live fixtures**

Change `"Debug"` to `"Off"` at:
- `tests/config_schema.rs:27`
- `tests/live_node_run.rs:40,141,218`
- `tests/eth_chainlink_taker_runtime.rs:91`
- `tests/platform_runtime.rs:200`
- `tests/cli.rs:67,129,188,246,306,459,521,580`
- `src/validate/tests.rs:1268`
- `src/platform/runtime.rs:1160,1485` (struct literals)
- `src/live_config.rs` test-body TOML literals matching `file_level = "Debug"`

Leave `src/validate/tests.rs:405/:409` alone — that test exercises lowercase `"debug"` rejection via `invalid_log_level`, still valid.

Verify: `grep -rn 'file_level = "Debug"' src/ tests/` returns nothing.

- [ ] **Step 3.3: Replace relative `local_dir`/`output_dir` in test fixtures**

Rule: schema-only tests use literal `/srv/bolt-v2/var/audit` (resp. `/srv/bolt-v2/var/raw`). Tests that actually exercise I/O (audit worker, renderer end-to-end) wrap the path with a `tempfile::tempdir()`-derived absolute path.

Per-file guidance:
- `config/operator-snapshots/2026-04-16/live.local.toml` — **leave as-is**. Frozen snapshot, no test loads it.
- `tests/support/mod.rs:118,166,231` — absolute literals.
- `tests/config_parsing.rs:26,52,64,91,117,156,197,251,321,400` — absolute literals. Line 64 (`assert_eq!(... "var/raw")`) must match whatever literal you put in the fixture.
- `tests/config_schema.rs:184,275,321,394` — absolute literals. Line 321 (`assert_eq!(... Some("var/audit"))`) updates to match.
- `tests/render_live_config.rs:255,344,426,717` — absolute literals.
- `tests/cli.rs:363` — absolute literal.
- `src/validate/tests.rs:696,1172,1174,1175,1340,1376,1435,2011,2156` — replace the source literal `"local_dir = \"var/audit\""` with `"local_dir = \"/srv/bolt-v2/var/audit\""` everywhere; `.replace(...)` call-sites stay structurally identical.
- `src/live_config.rs:1220` — absolute literal.
- `src/platform/runtime.rs:1514` — if the surrounding test constructs an `AuditSpoolConfig` and runs the worker, use a `tempdir()` path; otherwise absolute literal is fine.

Verify: `grep -rn 'local_dir = "var/audit"\|output_dir = "var/raw"' src/ tests/ config/live.local.example.toml` returns nothing.

- [ ] **Step 3.4: Run the full suite**

Run: `cargo nextest run`
Expected: all tests pass. Fixtures are now stricter than the validator requires — no regression.

- [ ] **Step 3.5: Commit**

```bash
git add config/live.local.example.toml tests/ src/validate/tests.rs src/live_config.rs src/platform/runtime.rs
git commit -m "refactor(config): move fixtures to absolute runtime paths + file_level=Off

Pre-tightening step: example config and every test fixture now use
/srv/bolt-v2/... for audit and raw_capture paths, file_level=Off. The
validator still accepts the legacy forms; the next commit tightens it.
Scope: #215."
```

---

## Task 4: Validator enforces absolute runtime paths

**Files:** `src/validate.rs`, `src/validate/tests.rs`.

- [ ] **Step 4.1: Add the absolute-path helper**

In `src/validate.rs`, beside `check_runtime_contract_path_shape` (~line 828):

```rust
fn check_absolute_path(
    errors: &mut Vec<ValidationError>,
    field: &str,
    value: &str,
    code: &'static str,
) {
    if value.is_empty() || value.trim().is_empty() {
        return; // check_non_empty already emits for this case.
    }
    if !std::path::Path::new(value).is_absolute() {
        push_error(
            errors,
            field,
            code,
            format!(
                "{field} must be an absolute path starting with '/', got \"{value}\" (example: \"/srv/bolt-v2/var/...\")"
            ),
        );
    }
}
```

- [ ] **Step 4.2: Wire into `validate_live_local`**

Inside the `if let Some(audit)` block (~line 1276), after `check_non_empty(..., "audit.local_dir", ...)`, add:
```rust
check_absolute_path(&mut errors, "audit.local_dir", &audit.local_dir, "not_absolute");
```

Immediately before the existing `if !config.streaming.catalog_path.trim().is_empty()` block (~line 1298), insert:
```rust
check_non_empty(&mut errors, "raw_capture.output_dir", &config.raw_capture.output_dir);
check_absolute_path(
    &mut errors,
    "raw_capture.output_dir",
    &config.raw_capture.output_dir,
    "not_absolute",
);
```

Inside that streaming block, add:
```rust
check_absolute_path(
    &mut errors,
    "streaming.catalog_path",
    &config.streaming.catalog_path,
    "not_absolute",
);
```

- [ ] **Step 4.3: Wire into `validate_runtime`**

Mirror the same three additions:
- In the `if let Some(audit)` block (~line 1883): `check_absolute_path` for `audit.local_dir`.
- After `check_runtime_contract_path_shape` (~line 1410): `check_non_empty` + `check_absolute_path` for `raw_capture.output_dir`.
- Inside the streaming-catalog guard: `check_absolute_path` for `streaming.catalog_path`.

- [ ] **Step 4.4: Add rejection tests**

In `src/validate/tests.rs`, four new tests. Each starts from a passing fixture and mutates one path:

```rust
#[test]
fn validate_live_local_rejects_relative_audit_local_dir() { /* assert_has_error(... "audit.local_dir", "not_absolute") */ }

#[test]
fn validate_live_local_rejects_relative_raw_capture_output_dir() { /* ... "raw_capture.output_dir" ... */ }

#[test]
fn validate_live_local_rejects_relative_streaming_catalog_path() { /* add [streaming] with relative catalog_path; assert ... "streaming.catalog_path" ... */ }

#[test]
fn validate_runtime_rejects_relative_audit_local_dir() { /* mirror for validate_runtime */ }
```

Use whatever helper already exists in the test module for minimal ruleset fixtures (there is one; existing tests call it). If none exists, inline the TOML once and reuse via a new module-private `fn minimal_ruleset_input_raw() -> String`.

- [ ] **Step 4.5: Run**

Run: `cargo nextest run`
Expected: new tests pass; everything else still green.

- [ ] **Step 4.6: Commit**

```bash
git add src/validate.rs src/validate/tests.rs
git commit -m "feat(validate): reject relative runtime paths

audit.local_dir, raw_capture.output_dir, and streaming.catalog_path
must be absolute in both the operator and rendered runtime configs.
Also closes a pre-existing gap: raw_capture.output_dir was
unvalidated before. Scope: #215."
```

---

## Task 5: Validator requires `logging.file_level = "Off"`

**Files:** `src/validate.rs`, `src/validate/tests.rs`.

- [ ] **Step 5.1: Add the guard in both validators**

Immediately after the existing `check_allowlist` on `logging.file_level` in `validate_live_local` (~line 893–899) AND in `validate_runtime` (~line 1354–1360), insert:

```rust
if config.logging.file_level != "Off" {
    push_error(
        &mut errors,
        "logging.file_level",
        "file_logging_disabled",
        format!(
            "logging.file_level must be \"Off\" (NT writes file logs to the process cwd and we cannot override that; use stdout_level for verbosity). Got \"{}\".",
            config.logging.file_level
        ),
    );
}
```

- [ ] **Step 5.2: Tests**

```rust
#[test]
fn validate_live_local_rejects_non_off_file_level() { /* swap fixture Off->Debug, assert error code file_logging_disabled */ }

#[test]
fn validate_live_local_accepts_off_file_level() { /* assert no error on logging.file_level field */ }

#[test]
fn validate_runtime_rejects_non_off_file_level() { /* mirror */ }
```

- [ ] **Step 5.3: Run**

Run: `cargo nextest run`
Expected: new tests pass; no regression (fixtures were already `Off`).

- [ ] **Step 5.4: Commit**

```bash
git add src/validate.rs src/validate/tests.rs
git commit -m "feat(validate): require logging.file_level = \"Off\"

NT's kernel hardcodes FileWriterConfig::default(), so any non-Off
file_level writes log files into the process cwd regardless of config.
Reject at validation and steer operators to stdout_level + journald.
Scope: #215."
```

---

## Task 6: Remove NT file-logging plumbing and `log_sweep`

**Files:** `src/main.rs`, `src/lib.rs`, `src/config.rs`. Delete `src/log_sweep.rs`, `tests/log_sweep.rs`, `BOLT-001_2026-04-20_031fc0d6-f752-4e0d-b65e-a58710866938.log`.

- [ ] **Step 6.1: Strip `main.rs`**

In `src/main.rs::Command::Run`:
- Remove line 60: `bolt_v2::log_sweep::sweep_stale_logs();`.
- In the `LoggerConfig` block (lines 66–70), remove the `fileout_level: parse_log_level(&cfg.logging.file_level)?` field; keep only `stdout_level` plus `..Default::default()`. `LoggerConfig::default().fileout_level == LevelFilter::Off`, so the runtime result is unambiguously "no file logging" regardless of the unused config value.

- [ ] **Step 6.2: Drop the module export**

In `src/lib.rs`, remove `pub mod log_sweep;`.

- [ ] **Step 6.3: Update the raw_capture default**

In `src/config.rs`, change `default_raw_capture_output_dir` return from `"var/raw"` to `"/srv/bolt-v2/var/raw"`.

- [ ] **Step 6.4: Delete module + test + stray artifact**

```bash
git rm src/log_sweep.rs tests/log_sweep.rs
rm -f BOLT-001_2026-04-20_031fc0d6-f752-4e0d-b65e-a58710866938.log
```

(Untracked `.log` file is not in `git rm`'s reach, hence `rm -f`.)

- [ ] **Step 6.5: Verify**

Run: `cargo nextest run` — all tests pass.

Run: `grep -rn "log_sweep\|sweep_stale_logs\|sweep_logs_in\|is_nt_log_filename" src/ tests/` — zero matches.

- [ ] **Step 6.6: Commit**

```bash
git add src/main.rs src/lib.rs src/config.rs
git add -u  # captures the git rm deletions
git commit -m "refactor(logging): remove NT file-logging plumbing and log_sweep

NT cannot be told where to put file logs (kernel hardcodes
FileWriterConfig::default()); the validator now refuses any non-Off
file_level. With file logging off, src/log_sweep.rs has no purpose —
it only existed to compensate for cwd-anchored log files. Stdout via
journald (capped in deploy/systemd/journald-bolt-v2.conf) is the
single logging path. Scope: #215."
```

---

## Task 7: Mark superseded docs

**Files:**
- `docs/superpowers/specs/2026-04-11-log-sweep-at-launch-design.md`
- `docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md`

- [ ] **Step 7.1: Prepend banner to each file**

```markdown
> SUPERSEDED BY #215 (plan: 2026-04-20-issue-215-root-volume-remediation.md). NT file logging is now disabled repo-wide; this design is historical.
```

- [ ] **Step 7.2: Commit**

```bash
git add docs/superpowers/specs/2026-04-11-log-sweep-at-launch-design.md docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md
git commit -m "docs(plans): mark 2026-04-11 log-sweep design superseded by #215"
```

---

## Task 8: Exact-head local verification (mirrors CI gate)

- [ ] **Step 8.1:** `cargo fmt --all -- --check` → exit 0.
- [ ] **Step 8.2:** `cargo deny --all-features check` → no errors.
- [ ] **Step 8.3:** `cargo clippy --all-targets --all-features -- -D warnings` → exit 0.
- [ ] **Step 8.4:** `cargo clippy --target aarch64-unknown-linux-gnu --all-targets --all-features -- -D warnings` → exit 0. (Ensure target installed: `rustup target add aarch64-unknown-linux-gnu`.)
- [ ] **Step 8.5:** `cargo nextest run` → all pass (includes `deploy_assets` suite).
- [ ] **Step 8.6:** `cargo zigbuild --release --target aarch64-unknown-linux-gnu` → produces `target/aarch64-unknown-linux-gnu/release/bolt-v2`.
- [ ] **Step 8.7:** `just check-workspace` → exit 0.
- [ ] **Step 8.8:** Regression grep:

```bash
grep -rn 'file_level = "Debug"\|local_dir = "var/audit"\|output_dir = "var/raw"\|sweep_stale_logs\|bolt_v2::log_sweep' src/ tests/ config/live.local.example.toml
```
Expected: zero matches.

---

## Task 9: Push and wait for CI green on the exact head

- [ ] **Step 9.1:** `git push -u origin issue-215-root-volume-remediation`.
- [ ] **Step 9.2:** Watch the CI run with `gh run watch` and confirm the `gate` lane reports success for the exact head SHA. Record the run URL.

**Do not open a PR or request external review until `gate` is green on this exact head.** Per `AGENTS.md`: "Do not ask for external review until the exact PR head's CI is confirmed green."

---

## Self-Review

**1. Spec coverage (brief → tasks):**

- "separate data volume design" → Task 1 (mount + installer) + Task 4 (validator enforces absolute writes).
- "fixed systemd `WorkingDirectory`" → Task 1 (unit) + Task 2 (locked by test).
- "move write-heavy paths off root" → Task 3 + Task 4 + Task 6.
- "absolute or safely anchored paths" → Task 4.
- "cap root log growth" → Task 1 (journald drop-in) + Task 5 (NT file logs disabled) + Task 6 (sweep removal).
- "app file logs" → Task 5 + Task 6.
- "raw_capture" → Task 4 + Task 6.
- "audit local spool" → Task 4.
- "journald/syslog" → Task 1.
- "SSM output growth" → Task 1 (README runbook: `--output-s3-bucket-name`).

**Rule coverage:**
- no dual paths — one unit file, one validator rule per path, no backwards-compat shim.
- no TODOs — none introduced; supersede notices are explicit not promissory.
- no hand-wavy fixes — every step names files + exact edits.
- no external review until exact-head green — Task 9 gates PR opening on CI green.
- CLAUDE.md rule 8 (no bolt v1) — no v1 reads anywhere.
- CLAUDE.md rule 9 (one scope) — root-volume remediation only; host cutover deferred.

**2. Placeholder scan:** No `TBD` / `fill in` / "similar to Task N" remain.

**3. Type consistency:**
- `check_absolute_path` signature is uniform across all call sites.
- Error codes `not_absolute` (paths) and `file_logging_disabled` (file_level) are new and unique.
- `deploy/systemd/bolt-v2.service` referenced identically in Task 1, Task 2, and `deploy/install.sh`.

---

## Execution Handoff

Plan saved to `docs/superpowers/plans/2026-04-20-issue-215-root-volume-remediation.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review diffs between tasks.
2. **Inline Execution** — execute tasks in this session via `superpowers:executing-plans`, with checkpoints.

Which approach?
