# bolt-v2 Next Session: Root-Volume Remediation

> Historical handoff. Do not implement from the current dirty detached checkout.
> Start from a fresh clean worktree off `main`.

## Why This Exists

This is the handoff for the root solution to the April 20, 2026 production incident on
`bolt-v2-polymarket` (`i-08dee6aefe9a5b02c`).

The prior session completed the forensics. The next session should not re-run broad trial-and-error
investigation. It should consume the findings below and implement the root fix.

## Executive Summary

The production issue was **not primarily trading logic**, **not CPU/RAM**, and **not mainly audit**.

The real problem was:

1. The service had **no `WorkingDirectory`**.
2. Relative runtime write paths drifted onto the **root filesystem**.
3. The host accumulated **multiple large logging surfaces on root**:
   - app file logs in `/var/logs`
   - system logs in `/var/log`
   - retained SSM command outputs in `/var/lib/amazon/ssm`
4. The root volume was only **8 GiB**.
5. Root hit **100% full**, after which:
   - `cloud-init` threw `OSError: [Errno 28] No space left on device`
   - SSM `RunShellScript` started failing instantly with empty stdout/stderr
   - journal was truncated
   - the filesystem showed `Structure needs cleaning` on at least one root-level log file

Audit was a **real service-level fail-close problem**, but only a **small part of the disk problem**.

## Proven Disk Breakdown

Recovered from a read-only mounted clone of the affected root volume:

- `/var/logs`: `2.0 GiB`
- `/var/log`: `2.0 GiB`
- `/var/lib`: `1.6 GiB`
- `/opt`: `316 MiB`
- `/usr`: `1.6 GiB`
- `/var/audit`: `11 MiB`
- `/data`: `39 MiB`

Dominant file classes:

- `/var/logs/BOLT-ETH-001_*.log`: many ~15 MiB files, some much larger
  - largest observed: `131,545,269` bytes
  - second: `62,363,140` bytes
- `/var/log/syslog.1`: `963,625,459` bytes
- `/var/log/syslog`: `362,016,768` bytes
- journald archive: `725.6 MiB`
- `/var/lib/amazon/ssm`: `714 MiB`
  - driven by retained `stdout` and `stdoutConsole` artifacts from many SSM runs
  - largest command outputs were ~140 MiB and ~107 MiB, each duplicated
- `/var/lib/snapd/snaps`: `510 MiB`
- `/var/lib/apt/lists`: `288 MiB`

## What Not To Re-Investigate

These are already settled:

- `c7g.large` is not the core problem.
  - CPU/RAM are not the bottleneck.
  - The problem is storage layout and logging.
- Audit alone did not fill the disk.
  - At snapshot time `/var/audit` was only `11 MiB`.
- The box is not in an Auto Scaling Group.
- `34.248.143.2` is an Elastic IP and can be reassociated.
- The current host is operationally unhealthy after ENOSPC.

## Root Solution To Implement

The architectural fix is:

1. Add a **separate data volume** for bolt runtime writes.
2. Set a fixed **systemd `WorkingDirectory`**.
3. Move all write-heavy bolt paths onto the data volume.
4. Cap root-disk log growth.

Suggested target layout:

- root volume: OS only
- data volume mount: `/srv/bolt-v2`
- service working directory: `/srv/bolt-v2`
- runtime write paths under `/srv/bolt-v2/var/...`

At minimum move:

- audit spool
- app file logs
- raw capture
- any local runtime history

Also harden:

- journald limits (`SystemMaxUse`, `RuntimeMaxUse`)
- syslog/logrotate policy
- SSM orchestration cache hygiene or operational discipline

## Service / Host Facts

- broken host instance: `i-08dee6aefe9a5b02c`
- name: `bolt-v2-polymarket`
- EIP: `34.248.143.2`
- subnet: `subnet-2c4fd44b`
- AZ: `eu-west-1c`
- SG: `sg-08921a4b725682171`
- instance profile: `bolt-polymarket-exec-role`
- key pair: `bolt-polymarket-key`
- AMI: `ami-037d87f13f7e014c5`
- instance type: `c7g.large`
- root volume: `vol-02f5a5c9554f0265f`

Forensic resources still present:

- snapshot: `snap-0c5f98e69aa4ac0bd`
- helper instance: `i-0bd63c88cd82e3b35`
- clone volume: `vol-0d24ceef69cd27b33`

Do not delete them until the remediation session either:

- finishes the implementation and writes up the result, or
- explicitly decides they are no longer needed.

## Fresh-Session Process

1. Start from fresh clean `main` worktree.
2. Read `docs/postmortems/2026-04-20-root-volume-incident.md`.
3. Do not repeat broad live-host probing.
4. Use the postmortem findings as the fixed input set.
5. Implement the storage/layout/logging fix.
6. Only after implementation, decide whether to:
   - recover the current host in place, or
   - roll the corrected design onto a fresh instance

## Non-Goals For The Next Session

- Do not re-debate whether audit or strategy logic caused the disk fill.
- Do not treat compute resizing as the main answer.
- Do not keep dumping huge investigative output through SSM on production unless absolutely necessary.
- Do not work from the current detached dirty checkout.

