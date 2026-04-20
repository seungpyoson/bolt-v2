# Issue 223: Monitoring And Alerting Contract For Replacement-Host Cutover

Date: `2026-04-20`

## Goal

Define the minimum operator-visible signals required to approve a rebuilt `bolt-v2` host during
launch validation and after cutover.

This is a **cutover-critical monitoring contract**, not a full observability redesign.

## Scope boundary

- This contract is for the live `bolt-v2` replacement-host decision path from `#221`.
- It assumes the `#215` baseline is already present:
  - `WorkingDirectory=/srv/bolt-v2`
  - `User=bolt`
  - data volume mounted at `/srv/bolt-v2`
  - journald capped with `SystemMaxUse=500M`
  - runtime write paths anchored under `/srv/bolt-v2/var/...`
- It does **not** define raw-capture retention or audit S3 lifecycle policy from `#219`.
- It does **not** require new trading logic or a broad metrics platform rollout.

## Contract principle

For `#223`, “monitoring and alerting” means:

- each required signal has a concrete observation surface today
- each required signal has a healthy condition
- each required signal has a fail condition that blocks approval or triggers rollback/escalation

This issue does **not** require every signal to be backed by a fully automated CloudWatch alarm
before a rebuilt host can be approved. A staffed operator watch is acceptable as long as the signal
is concrete and the fail condition is explicit.

## Required signal families

Any rebuilt host must provide enough visibility for operators to judge these signal families:

1. control-plane access
2. storage and mount health
3. service health and restart behavior
4. config and secret resolution
5. reference health and staleness
6. selector health
7. feed disconnect / reconnect storm visibility
8. strategy readiness / non-readiness
9. audit spool health and backlog

## Existing observation surfaces

These are the current minimum surfaces the contract relies on:

- `systemctl` / `systemctl show`
- `journalctl -u bolt-v2`
- `findmnt`, `df`, `du`, `namei`
- `aws ssm describe-instance-information`
- `aws ssm start-session`
- `aws ssm send-command`
- `bolt-v2 secrets check`
- `bolt-v2 secrets resolve`
- audit JSONL under `/srv/bolt-v2/var/audit`
- file logs under `/srv/bolt-v2/var/logs`

## Signal contract

### 1. Control-plane access

Observation surfaces:

- `aws ssm describe-instance-information`
- `aws ssm start-session`
- `aws ssm send-command`

Healthy condition:

- instance reports `PingStatus = Online`
- Session Manager opens successfully
- `RunShellScript` returns usable stdout/stderr for a read-only smoke command

Fail condition:

- host is online but `RunShellScript` is broken, empty, or unusable
- Session Manager cannot open a shell

Why it matters:

- the old damaged host remained `SSM Online` while `RunShellScript` was effectively dead
- replacement approval requires proving that the control path actually works, not just that the
  instance is registered

### 2. Storage and mount health

Observation surfaces:

- `findmnt / /srv/bolt-v2`
- `df -h / /srv/bolt-v2`
- `journalctl --disk-usage`
- `du -sh /srv/bolt-v2/var/logs /srv/bolt-v2/var/audit /srv/bolt-v2/var/raw`

Healthy condition:

- `/srv/bolt-v2` exists as a separate mounted data volume
- root remains OS-like during the watch window and does not show unexpected growth
- runtime writes land under `/srv/bolt-v2/var/...`
- journald remains within the configured cap of `500M`

Fail condition:

- `/srv/bolt-v2` is missing or not mounted
- runtime files appear on root-side write surfaces instead of the data volume
- root usage trends upward in the same class as the original incident
- journald exceeds the configured cap or shows unexpected growth

### 3. Service health and restart behavior

Observation surfaces:

- `systemctl status bolt-v2`
- `systemctl show bolt-v2 -p ActiveState -p SubState -p ExecMainStatus -p NRestarts -p FragmentPath`
- `journalctl -u bolt-v2 -n 200 -f`

Healthy condition:

- `ActiveState=active`
- `SubState=running`
- `ExecMainStatus=0`
- `NRestarts` stays stable during launch validation
- journal shows normal startup progression rather than restart churn

Fail condition:

- service never becomes `active/running`
- service exits non-zero
- restarts increment during launch validation
- journal shows a restart loop or fail-closed shutdown

### 4. Config and secret resolution

Observation surfaces:

- `bolt-v2 secrets check --config /opt/bolt-v2/config/live.toml`
- `bolt-v2 secrets resolve --config /opt/bolt-v2/config/live.toml`
- `namei -l /opt/bolt-v2/config/live.toml`

Healthy condition:

- secret-config completeness passes
- secret resolution through SSM succeeds
- config remains readable by the service user

Fail condition:

- any secret path is missing, unreadable, or fails SSM resolution
- config file permissions are incompatible with the service user

### 5. Reference health and staleness

Observation surfaces:

- latest `ReferenceSnapshot` audit records in `/srv/bolt-v2/var/audit/...`
- `journalctl -u bolt-v2`

Current runtime thresholds from the operator lane:

- reference venue `stale_after_ms = 1000`
- reference venue `disable_after_ms = 5000`
- `reference.publish_topic = "platform.reference.default"`

Healthy condition:

- fresh `ReferenceSnapshot` records are appearing
- `confidence > 0`
- required venues are not persistently `stale=true`
- required venues are not persistently `health=Disabled`
- reasons such as `no reference update received yet` or `auto-disabled after ...` do not persist

Fail condition:

- reference snapshots stop appearing
- confidence stays zero
- one or more required venues stay stale or disabled long enough to prevent healthy fused reference
  operation

### 6. Selector health

Observation surfaces:

- `SelectorDecision` audit records
- `EligibilityReject` audit records
- `journalctl -u bolt-v2`

Current runtime thresholds from the operator lane:

- selector poll interval `1000ms`
- candidate load timeout `30s`
- ruleset selector tag `ethereum`
- ruleset selector prefix `eth-updown-5m`

Healthy condition:

- selector decisions are being emitted
- selector reaches `Active` or expected `Freeze` with a concrete market/instrument
- `EligibilityReject` records are explainable and do not eliminate all candidates indefinitely

Fail condition:

- selector remains in `Idle` unexpectedly
- selector emits only rejects and never reaches an actionable market
- selector discovery fails closed or produces no usable market set

### 7. Feed disconnect / reconnect storm visibility

Observation surfaces:

- `journalctl -u bolt-v2`
- especially Chainlink reconnect/error logs

Current runtime threshold from the operator lane:

- `reference.chainlink.ws_reconnect_alert_threshold = 5`

Healthy condition:

- connection noise is transient
- no sustained reconnect storm is present
- no repeated client/session-failure errors dominate the watch window

Fail condition:

- repeated logs such as
  - `failed to connect Chainlink Data Streams websocket`
  - `Chainlink Data Streams session failed`
  - `Chainlink Data Streams hit 5 consecutive connection failure(s); continuing reconnect loop`
  continue without recovery

This is the minimum current reconnect-storm signal. A rebuilt host is not production-equivalent if
operators cannot see when reference feeds are failing repeatedly.

### 8. Strategy readiness / non-readiness

Observation surfaces:

- `journalctl -u bolt-v2 | rg 'eth_chainlink_taker (entry|exit) evaluation'`

Current runtime thresholds from the operator lane:

- `warmup_tick_count = 50`
- `vol_min_observations = 20`
- `forced_flat_stale_chainlink_ms = 1500`
- `lead_jitter_max_ms = 250`

Healthy condition:

- after warmup, entry/exit evaluation logs show the strategy in `phase=Active`
- the strategy is not persistently blocked by warmup, stale reference data, non-ready volatility,
  or forced-flat conditions
- no sustained `submission_blocked_reason` prevents the lane from becoming operationally ready

Fail condition:

- the lane never gets past warmup/readiness gates
- `gate_blocked_by` or `pricing_blocked_by` remain persistently non-empty for readiness-critical
  reasons
- forced-flat reasons remain active unexpectedly

This signal matters because a clean process start is not enough. Approval requires a trading-ready
lane, not merely a booted binary.

### 9. Audit spool health and backlog

Observation surfaces:

- `du -sb /srv/bolt-v2/var/audit`
- `/srv/bolt-v2/var/audit/.sequence-watermark`
- `journalctl -u bolt-v2 | rg 'platform audit task failed|max_local_backlog_bytes exceeded'`

Current runtime threshold from the operator lane:

- `audit.max_local_backlog_bytes = 10485760`

Healthy condition:

- audit files are being written
- `.sequence-watermark` advances over time
- retained local backlog stays below the configured cap
- no audit-task failure is emitted to the journal

Fail condition:

- backlog exceeds the configured limit
- `.sequence-watermark` stalls unexpectedly
- journal emits
  - `platform audit task failed`
  - `max_local_backlog_bytes exceeded: ...`

The audit task is part of the fail-closed runtime path, so this is a cutover-critical signal rather
than an optional one.

## Phase-specific watch contract

### Phase 1: Pre-start validation

Before starting `bolt-v2`, operators must verify:

- SSM works through both session and command execution paths
- `/srv/bolt-v2` is mounted and writable by the service user
- rendered config and SSM secret resolution succeed
- root and data-volume headroom are visible
- journald usage is visible

If any of those are unknown, cutover is blocked.

### Phase 2: Startup watch

From service start until the lane becomes trading-ready, operators must watch:

- `systemctl show/status`
- `journalctl -u bolt-v2`
- audit records under `/srv/bolt-v2/var/audit`

The startup watch is not complete until operators have seen:

- service stable in `active/running`
- fresh `ReferenceSnapshot` audit records
- a concrete `SelectorDecision`
- strategy evaluation logs showing warmup progression and then a ready/active state

### Phase 3: Post-cutover staffed watch

After traffic or the EIP moves to the rebuilt host, operators must continue a staffed watch long
enough to observe:

- stable service state with no restart churn
- stable root/data-volume behavior
- continuing reference snapshots and selector decisions
- no reconnect storm
- strategy remains ready rather than falling back into persistent blocked or forced-flat states
- audit watermark continues advancing and backlog stays under cap

If any fail condition appears during this watch, the replacement host is not approved as a
production-equivalent lane.

## Minimum operator commands

These are the minimum commands the contract assumes operators can run:

```bash
aws ssm describe-instance-information --region <region> --filters Key=InstanceIds,Values=<instance-id>
aws ssm start-session --region <region> --target <instance-id>
aws ssm send-command --region <region> --instance-ids <instance-id> --document-name AWS-RunShellScript ...

findmnt / /srv/bolt-v2
df -h / /srv/bolt-v2
journalctl --disk-usage

systemctl status bolt-v2
systemctl show bolt-v2 -p ActiveState -p SubState -p ExecMainStatus -p NRestarts
journalctl -u bolt-v2 -n 200 -f

/opt/bolt-v2/bolt-v2 secrets check --config /opt/bolt-v2/config/live.toml
/opt/bolt-v2/bolt-v2 secrets resolve --config /opt/bolt-v2/config/live.toml

du -sb /srv/bolt-v2/var/audit
ls -l /srv/bolt-v2/var/audit/.sequence-watermark
rg 'eth_chainlink_taker (entry|exit) evaluation' /srv/bolt-v2/var/logs /var/log/syslog* 2>/dev/null || true
rg 'platform audit task failed|max_local_backlog_bytes exceeded|Chainlink Data Streams hit' /var/log/syslog* /srv/bolt-v2/var/logs 2>/dev/null || true
```

Operators may choose better wrappers, but the contract is not satisfied unless these underlying
surfaces remain available.

## Relationship to `#221`

This document supplies the missing operational-parity contract from `#221`.

`#221` can now use this artifact directly for the operational-parity portion of replacement-host
approval:

- control-plane visibility
- service/log visibility
- reference/selector/strategy readiness visibility
- audit backlog visibility
- explicit fail conditions during launch validation and after cutover

## Follow-up issues

No additional implementation issue was opened from this pass.

Reason:

- every required cutover-critical signal already has at least one concrete observation surface
  today
- the minimum contract can be satisfied by staffed operator observation using existing systemd,
  journal, SSM, filesystem, and audit/log surfaces

If the team later wants automated CloudWatch alarms or richer dashboards, that can be tracked as a
separate improvement, but it is not required to close `#223`.
