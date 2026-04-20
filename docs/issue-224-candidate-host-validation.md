# Issue 224: Candidate Host Validation

Date: `2026-04-20`

## Decision for this candidate

**No. This rebuilt host is not yet an approved production-equivalent replacement environment.**

The candidate host proves that the merged `#215` remediation baseline can be provisioned cleanly on
a fresh EC2 instance, but the runtime did **not** reach trading-ready startup. The concrete blocker
found in this run is now tracked in `#225`.

## Candidate identity

- Candidate instance: `i-0b969ff05b7b47811`
- Region / AZ: `eu-west-1` / `eu-west-1c`
- Instance type: `c7g.large`
- AMI: `ami-037d87f13f7e014c5`
- Private IP: `172.31.4.199`
- IAM instance profile: `bolt-polymarket-exec-role`
- Security group: `sg-08921a4b725682171`
- Data volume: `vol-04146bd9128af2efd`
- Data volume device: `/dev/nvme1n1`

This candidate was launched as a validation-only host for `#224`. No EIP reassociation or
production cutover was performed.

## Deploy payload identity

Artifacts uploaded for this run:

- S3 prefix: `s3://bolt-deploy-artifacts/manual/issue-224/20260420T121518Z`
- ARM64 binary SHA256:
  `84138da64666787d34e513938e770bb970d227743cd8710088ea43508d0813e5`
- Forensic rendered config SHA256:
  `332eab4b01c11a60d9384a04ee2dd6fbc5a1d01fb3e9d72be856db0ca6d6d326`
- Candidate rendered config SHA256:
  `e826fdc9a24025bfde59af1067be931224aca0d9bb2e62520dd0cf90ea0e9bf3`

Config basis:

- Started from the exact rendered `live.toml` recovered from the forensic clone
- preserved the real operator/runtime lane settings
- changed only the `#215` write paths:
  - `raw_capture.output_dir = "/srv/bolt-v2/var/raw"`
  - `audit.local_dir = "/srv/bolt-v2/var/audit"`

## What happened

### 1. Candidate creation and control-plane access

Validated and then launched:

- dry-run for instance launch succeeded
- dry-run for data-volume creation succeeded
- instance came up `SSM Online`
- data volume attached successfully
- smoke `RunShellScript` on the candidate succeeded

Result:

- control-plane access was healthy on the candidate host

### 2. First install attempt failed for a non-host reason

The first install attempt failed before provisioning because the presigned S3 URLs were minted
against `eu-west-1`, while the `bolt-deploy-artifacts` bucket actually lives in `us-east-2`.

Observed on-host:

- downloaded files were XML `PermanentRedirect` responses instead of the payload

Action taken:

- regenerated presigned URLs for the bucket’s actual region (`us-east-2`)
- retried using the same uploaded payload

This was a deployment-transport mistake in the validation flow, not a host-parity issue.

### 3. Second install attempt proved the `#215` baseline

The corrected install/start path succeeded far enough to prove the new host can carry the merged
`#215` baseline:

- data volume was formatted and mounted at `/srv/bolt-v2`
- `deploy/install.sh` completed
- journald restarted with the configured cap
- `bolt-v2.service` was installed and enabled
- config readability was repaired to `root:bolt 0640`
- service user/runtime directories were created under `/srv/bolt-v2/var`

Observed state after provisioning:

- service unit:
  - `UnitFileState=enabled`
  - `ActiveState=active`
  - `SubState=running`
  - `ExecMainStatus=0`
  - `NRestarts=0`
- mount:
  - `/srv/bolt-v2 -> /dev/nvme1n1 ext4 rw,relatime`
- headroom:
  - root: `7.6G size`, `2.2G used`, `29%`
  - data volume: `20G size`, `2.7M used`, `1%`
- config permissions:
  - `/opt/bolt-v2/config/live.toml` -> `root:bolt 0640`
- runtime dirs:
  - `/srv/bolt-v2/var`
  - `/srv/bolt-v2/var/audit`
  - `/srv/bolt-v2/var/logs`
  - `/srv/bolt-v2/var/raw`
  all owned by `bolt:bolt`
- journald usage:
  - `8.0M`

This is the strongest positive result from the run:

- the host/storage/service baseline from `#215` is reproducible on a fresh candidate

### 4. Secret resolution required one additional host prerequisite

The fresh host did not initially have an `aws` binary available, so:

- `bolt-v2 secrets resolve --config /opt/bolt-v2/config/live.toml`
  first failed with:
  - `SecretError("Failed to run aws ssm get-parameter ... No such file or directory")`

Action taken:

- installed `awscli` on the candidate host

After that:

- `bolt-v2 secrets resolve` succeeded

Implication:

- a working `aws` CLI on-host is a real runtime prerequisite for the current secret-resolution path

### 5. Runtime did not reach trading-ready startup

After the candidate host was provisioned and the service started, the runtime did **not** reach the
healthy startup condition required by `#221`.

Observed sequence:

- the process built and entered the event loop
- most data clients connected successfully
- Polymarket execution connected successfully
- Binance loaded spot instruments successfully
- then Binance failed its WebSocket connect with:
  - `HTTP error: 400 Bad Request`
- the startup gate later emitted:
  - `Timed out (60s) waiting for engines to connect`
  - `Not starting trader: engine client(s) not connected`

The captured connection table on the candidate host showed:

- `HYPERLIQUID` data = `true`
- `KRAKEN` data = `true`
- `BYBIT` data = `true`
- `BINANCE` data = `false`
- `POLYMARKET` data = `true`
- `OKX` data = `true`
- `DERIBIT` data = `true`
- `CHAINLINK` data = `true`
- `POLYMARKET` execution = `true`

That concrete Binance failure is now tracked in `#225`.

## Audit and selector observations

Audit surfaces on the candidate host were partially working:

- `.sequence-watermark` existed and advanced to `1`
- one audit JSONL file existed:
  - `/srv/bolt-v2/var/audit/date=2026-04-20/part-00000000000000000000.jsonl`
- audit record kind counts:
  - `selector_decision: 90`

Selector result:

- the selector was not idle
- it repeatedly emitted `state = active`
- selected market `2022635`
- selected a concrete Polymarket instrument ID

What did **not** appear:

- no `ReferenceSnapshot` audit records
- no file logs under `/srv/bolt-v2/var/logs`
- no evidence that the trader actually started running strategy logic

Interpretation:

- the candidate host reached enough runtime state to mount, boot, resolve secrets, connect most
  clients, and run the selector
- but it did **not** satisfy the startup gate needed to start the trader, so it never reached the
  trading-ready state required for approval

## Parity result against `#221`

### Passed in this run

- control-plane access on the candidate host
- data-volume provisioning and mount
- service installation and enablement
- config readability and service-user directory ownership
- root/data-volume headroom and journald cap
- secret-config completeness
- secret resolution after installing `awscli`
- selector activity and audit file creation

### Failed in this run

- service startup did not translate into trader startup
- one required data client (`BINANCE`) remained disconnected
- the startup gate timed out after 60 seconds
- trader did not start
- strategy readiness was not reached
- reference-health evidence was incomplete
- no end-to-end trading-ready proof exists for this host

## Approval answer

For candidate host `i-0b969ff05b7b47811`, the answer is:

**No, do not approve this host yet as a production-equivalent replacement environment.**

Reason:

- host/platform parity is substantially improved and the `#215` baseline is proven
- but the runtime still fails a real startup-readiness condition on a fresh host
- that means the replacement environment is still not functionally equivalent for actual trading

## Follow-up

Opened from this run:

- `#225` — Binance spot websocket returns HTTP 400 on fresh live host and blocks trader startup

This is the concrete blocker that must be resolved or explicitly removed from startup-critical
requirements before a rebuilt host can be approved.
