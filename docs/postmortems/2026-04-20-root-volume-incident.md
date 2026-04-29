# Root Volume Incident Postmortem

Date: `2026-04-20`

Scope: production incident for `bolt-v2-polymarket` / `i-08dee6aefe9a5b02c`

This document is the detailed forensic log for the root-volume / host-control incident. It is
written so a future session can reproduce the reasoning path without repeating wide trial-and-error.

## Final Findings

### 1. Disk exhaustion was real and host-local

Evidence:

- EC2 console output showed:
  - `cloud-init ... OSError: [Errno 28] No space left on device`
- the cloned root volume mounted read-only at `100%` used:
  - `Filesystem      Size  Used Avail Use% Mounted on`
  - `/dev/nvme1n1p1  7.6G  7.6G     0 100% /mnt/forensic`

Conclusion:

- the host root filesystem filled completely
- this was not an AWS control-plane false positive

### 2. SSM control failure was a consequence of host damage, not IAM/network

Evidence:

- instance remained `PingStatus = Online`
- instance profile still had `AmazonSSMManagedInstanceCore`
- SG egress still allowed `443`
- EC2 reachability and EBS health were `ok`
- but every `AWS-RunShellScript` invocation failed immediately:
  - `ResponseCode = 1`
  - `ExecutionElapsedTime = PT0S`
  - empty stdout/stderr
- automation `AWSSupport-TroubleshootSessionManager` failed in the `getAgentDiagnosticsLinux` step
- the failing command was literally:
  - `/usr/bin/ssm-cli get-diagnostics`
- a successful command shortly before full failure already reported:
  - `Journal file .../system.journal is truncated, ignoring file.`
- forensic mount later showed one root-level file path returning:
  - `Structure needs cleaning`

Conclusion:

- the SSM control path broke because the host filesystem/log state was unhealthy after ENOSPC
- this was not primarily IAM, SG, or registration drift

### 3. Audit was a real fail-close service problem, but not the main disk consumer

Evidence:

- active spool path on host was `/var/audit`
- the service had no `WorkingDirectory`
- runtime config used relative paths like:
  - `local_dir = "var/audit"`
- active spool exceeded configured cap earlier in the incident
- audit backlog calculations showed:
  - `file_count 12`
  - `total_bytes 10485773`
- after archival and later recheck:
  - `/var/audit` was ~`6.9 MiB`
- forensic clone top-level breakdown showed:
  - `/mnt/forensic/var/audit`: `11 MiB`

Conclusion:

- audit caused service-level fail-close behavior
- but audit did **not** explain the disk filling to `100%`

### 4. The main disk consumers were logs and SSM artifacts on root

Exact read-only forensic breakdown:

- `/var/logs`: `2.0 GiB`
- `/var/log`: `2.0 GiB`
- `/var/lib`: `1.6 GiB`
- `/opt`: `316 MiB`
- `/usr`: `1.6 GiB`

Dominant files:

#### `/var/logs`

App file logs:

- many `BOLT-ETH-001_*.log` files around `15 MiB`
- large examples:
  - `131,545,269` bytes
  - `62,363,140` bytes
  - `43,646,376` bytes
  - many more between `14-36 MiB`

#### `/var/log`

System logs:

- `/var/log/syslog.1`: `963,625,459`
- `/var/log/syslog`: `362,016,768`
- journald archive total:
  - `Archived and active journals take up 725.6M in the file system.`

#### `/var/lib/amazon/ssm`

Retained SSM command output:

- directory total under `/var/lib/amazon/ssm`: `714 MiB`
- large duplicated files observed:
  - `139,704,101` bytes `stdout`
  - `139,704,101` bytes `stdoutConsole`
  - `107,089,139` bytes `stdout`
  - `107,089,139` bytes `stdoutConsole`
  - multiple other duplicated multi-MB stdout artifacts

Conclusion:

- the disk filled primarily because write-heavy logging stayed on the root volume
- audit was secondary

### 5. The root design bug was missing path anchoring

Evidence:

- unit file on host:
  - `ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml`
  - **no `WorkingDirectory`**
- current process cwd observed earlier:
  - `/`
- config contained relative runtime paths:
  - `[raw_capture] output_dir = "var/raw"`
  - `[audit] local_dir = "var/audit"`
- earlier investigation also found root-level bolt log files and later a large `/var/logs` tree

Conclusion:

- relative write paths drifted to the root filesystem
- root disk became the dumping ground for service runtime data

## Important Corrections To Earlier Hypotheses

### Incorrect / incomplete framing

- “audit is the disk problem”

Why it was incomplete:

- audit was only ~`11 MiB` at forensic capture
- the actual dominant consumers were file logs, syslog/journald, and SSM stdout artifacts

### Corrected framing

- audit is the fail-close service problem
- logging/storage layout on root is the disk problem

## Reproducible Investigation Path

This is the shortest evidence path to reach the final findings.

### A. Confirm host identity and lack of ASG attachment

```bash
aws ec2 describe-instances --region eu-west-1 --instance-ids i-08dee6aefe9a5b02c
aws autoscaling describe-auto-scaling-instances --region eu-west-1 --instance-ids i-08dee6aefe9a5b02c
aws ec2 describe-addresses --region eu-west-1 --public-ips 34.248.143.2
```

What this proved:

- standalone EC2 instance
- no Auto Scaling attachment
- EIP can be reassociated

### B. Confirm SSM failure mode is host-local, not IAM/network

```bash
aws ssm describe-instance-information --region eu-west-1 \
  --filters Key=InstanceIds,Values=i-08dee6aefe9a5b02c

aws iam list-attached-role-policies --role-name bolt-polymarket-exec-role
aws ec2 describe-security-groups --region eu-west-1 --group-ids sg-08921a4b725682171

aws ssm get-automation-execution --region eu-west-1 \
  --automation-execution-id 486e498f-ecdd-4abe-8d84-e80d0c19f9e5

aws ssm get-command-invocation --region eu-west-1 \
  --command-id 743be45c-f4ea-4364-980b-1b6570e5d3aa \
  --instance-id i-08dee6aefe9a5b02c
```

Key observations:

- `PingStatus = Online`
- IAM/SG looked fine
- `RunShellScript` failed with empty stdout/stderr and `PT0S`

### C. Confirm `ENOSPC` from console output

```bash
aws ec2 get-console-output --instance-id i-08dee6aefe9a5b02c --latest --region eu-west-1 --output text
```

Key observation:

- `cloud-init` emitted `OSError: [Errno 28] No space left on device`

### D. Recover earlier successful SSM probes for historical state

Useful command IDs:

- `3d925f13-aecb-42a4-8357-fda3122b8df2`
  - `check disk layout and headroom`
- `29effba8-2298-48bc-992b-2d2955a04178`
  - `inspect archived audit spool and disk`
- `4bd8a151-314c-4885-9840-c77d4f2aaad7`
  - `inspect bolt-v2 log file locations`
- `ad743ec6-0e10-4ee4-9919-40bd357fc334`
  - `inspect live config capture flags and mtimes`
- `a15256cb-197e-44fe-bfc8-f577b7fbfdd3`
  - `inspect deployed state v2`
- `a3bc6983-2756-4ab1-b240-1be35b30fc89`
  - last successful live monitor before full failure

Example:

```bash
aws ssm get-command-invocation --region eu-west-1 \
  --command-id 29effba8-2298-48bc-992b-2d2955a04178 \
  --instance-id i-08dee6aefe9a5b02c
```

These historical commands proved:

- audit was active in `/var/audit`
- host root was already `89%` used after some cleanup
- unit lacked `WorkingDirectory`
- current process cwd had been `/`
- one successful command reported truncated journal

### E. Fresh-helper baseline to separate “normal Ubuntu usage” from incident growth

Provision helper:

```bash
aws ec2 run-instances --region eu-west-1 \
  --image-id ami-037d87f13f7e014c5 \
  --instance-type t4g.small \
  --iam-instance-profile Name=bolt-polymarket-exec-role \
  --security-group-ids sg-08921a4b725682171 \
  --subnet-id subnet-2c4fd44b
```

Then run:

```bash
df -h /
du -xhd1 /
du -xhd1 /var
du -xhd1 /var/lib
snap list --all
```

Baseline from fresh helper:

- root used only `1.6 GiB`

This proved the broken host’s extra multi-GB usage was incident growth, not normal base OS size.

### F. Snapshot + read-only forensic mount

Create snapshot:

```bash
aws ec2 create-snapshot --region eu-west-1 --volume-id vol-02f5a5c9554f0265f
```

Create clone volume from snapshot, attach to helper, then mount read-only:

```bash
sudo mount -o ro,noload /dev/nvme1n1p1 /mnt/forensic
```

Then collect:

```bash
df -h /mnt/forensic
df -i /mnt/forensic
sudo du -xhd1 /mnt/forensic | sort -h
sudo du -xhd1 /mnt/forensic/var | sort -h
sudo du -xhd2 /mnt/forensic/var/log | sort -h
sudo du -xhd2 /mnt/forensic/var/lib | sort -h
sudo du -xhd3 /mnt/forensic/opt | sort -h
sudo find /mnt/forensic/var/log -xdev -type f -printf "%s %p\n" | sort -nr | head
sudo find /mnt/forensic/var/logs -xdev -type f -printf "%s %p\n" | sort -nr | head
sudo find /mnt/forensic/var/lib/amazon/ssm -xdev -type f -printf "%s %p\n" | sort -nr | head
sudo journalctl --directory=/mnt/forensic/var/log/journal --disk-usage
```

This is the step that produced the final exact breakdown.

## What Future Sessions Should Not Repeat

- Do not keep issuing large verbose SSM forensic probes directly to the production host.
  - Those probes themselves can enlarge `/var/lib/amazon/ssm`.
- Do not stop at “audit is the cause.”
  - That is only a partial explanation.
- Do not debate CPU/RAM.
  - This was a storage-layout incident.
- Do not work from the current dirty detached checkout for remediation code.

## Root Solution Direction

The root solution is **not** “bigger instance type.”

The root solution is:

1. dedicated data volume for bolt runtime writes
2. fixed service working directory
3. write-heavy paths moved off root
4. root log growth capped

Potential implementation shape:

- root disk: OS only
- data volume mount: `/srv/bolt-v2`
- systemd `WorkingDirectory=/srv/bolt-v2`
- runtime write paths under `/srv/bolt-v2/var/...`
- journald capped
- syslog/logrotate capped
- audit S3 destination corrected

## Forensic Resources

Still present at time of writing:

- broken host instance: `i-08dee6aefe9a5b02c`
- forensic snapshot: `snap-0c5f98e69aa4ac0bd`
- forensic helper instance: `i-0bd63c88cd82e3b35`
- forensic clone volume: `vol-0d24ceef69cd27b33`

Do not clean these up until the remediation session explicitly decides it no longer needs them.

