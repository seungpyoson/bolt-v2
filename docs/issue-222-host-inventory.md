# Issue 222: Current Host Inventory For Replacement-Host Parity

Date: `2026-04-20`

Scope: capture the current/forensic host inventory needed to compare any replacement host against
the pre-remediation production lane.

## End-to-end capture path

This inventory was captured through the full chain below rather than by copying old notes:

1. Identified the current production host through AWS APIs:
   - instance `i-08dee6aefe9a5b02c`
   - name `bolt-v2-polymarket`
   - public IP / EIP `34.248.143.2`
   - root volume `vol-02f5a5c9554f0265f`
2. Confirmed the host is still `running` and `SSM Online`.
3. Confirmed the live `RunShellScript` path is still broken:
   - command `2063f14d-c088-4b56-9441-07005f4e2e04`
   - `Status = Failed`
   - `ResponseCode = 1`
   - empty stdout/stderr
4. Switched to the forensic helper path:
   - helper instance `i-0bd63c88cd82e3b35`
   - clone volume `vol-0d24ceef69cd27b33`
   - helper smoke command `0cddc1d1-0b1f-488a-af83-8c2f2bf54ead` succeeded
5. Mounted the cloned root volume read-only on the helper:
   - mount command `aea02410-51ce-40f0-bcf3-0571b934c58d`
   - mounted device `/dev/nvme1n1p1`
   - mountpoint `/mnt/forensic`
   - options `ro,relatime,norecovery`

That means the filesystem-level facts below come from the cloned root volume itself, not from
guesswork or from an edited branch.

## AWS identity inventory

- Instance ID: `i-08dee6aefe9a5b02c`
- Name: `bolt-v2-polymarket`
- State: `running`
- AMI: `ami-037d87f13f7e014c5`
- Instance type: `c7g.large`
- Key pair: `bolt-polymarket-key`
- Subnet: `subnet-2c4fd44b`
- AZ: `eu-west-1c`
- Private IP: `172.31.7.199`
- Public IP: `34.248.143.2`
- EIP allocation: `eipalloc-0bfd66b94102077e0`
- EIP association: `eipassoc-02f2e02c14cdee6d3`
- ENI: `eni-067dc25ad71bcfebd`
- Security group: `sg-08921a4b725682171` (`bolt-polymarket-exec`)
- VPC: `vpc-670f6d00`
- IAM instance profile:
  `arn:aws:iam::675819144420:instance-profile/bolt-polymarket-exec-role`
- Block devices on the live instance:
  - root only: `vol-02f5a5c9554f0265f`

Attached policies on `bolt-polymarket-exec-role` at capture time:

- `AmazonSSMManagedInstanceCore`
- `CloudWatchAgentServerPolicy`
- `bolt-deploy-read`
- `bolt-archive-write`
- `bolt-ticks-write`

## Service and unit inventory

Mounted unit file found at `/etc/systemd/system/bolt-v2.service` on the cloned root:

```ini
[Unit]
Description=Bolt v2 Trading Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml
Restart=on-failure
RestartSec=10
StartLimitBurst=3
StartLimitIntervalSec=300

[Install]
WantedBy=multi-user.target
```

Key parity-relevant facts:

- No `User=` directive
- No `Group=` directive
- No `WorkingDirectory=`
- `ExecStart` points at `/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml`
- Restart behavior differs from the `#215` baseline:
  - current host: `RestartSec=10`, `StartLimitBurst=3`, `StartLimitIntervalSec=300`
  - post-`#215` baseline: `RestartSec=5` and explicit runtime mount guard

Enabled-state evidence from the cloned root:

- `/etc/systemd/system/multi-user.target.wants/bolt-v2.service -> /etc/systemd/system/bolt-v2.service`

No `bolt-v2.service.d` drop-in directory was present on the cloned root at capture time.

## Journald inventory

Files found:

- `/etc/systemd/journald.conf`
- no journald drop-ins under `/etc/systemd/journald.conf.d/`

The cloned root shows the stock Ubuntu `journald.conf` with no active `SystemMaxUse` override.

Forensic journal usage:

- `Archived and active journals take up 725.6M in the file system.`

This is materially different from the post-`#215` baseline, which adds a journald drop-in with
`SystemMaxUse=500M`.

## Mount and fstab inventory

Mounted `/etc/fstab` from the cloned root:

```fstab
LABEL=cloudimg-rootfs	/	 ext4	discard,errors=remount-ro	0 1
LABEL=UEFI	/boot/efi	vfat	umask=0077	0 1
```

Key parity-relevant facts:

- No separate data-volume mount is configured
- No `/srv/bolt-v2` mount exists
- Live EC2 block-device mapping also shows only the root volume attached

This confirms the pre-remediation host was still a root-only filesystem layout.

## Users, groups, and permission inventory

On the cloned root:

- no `bolt` user entry was found in `/etc/passwd`
- no `bolt` group entry was found in `/etc/group`

Observed path ownership and modes:

- `/opt/bolt-v2`: `root:root` `0755`
- `/opt/bolt-v2/config`: `root:root` `0755`
- `/opt/bolt-v2/config/live.toml`: `root:root` `0644`
- `/var/audit`: `root:root` `0755`
- `/var/raw`: absent

This is materially different from the `#215` replacement baseline, which expects a dedicated
service user and config readability via `root:bolt` `0640`.

## Deployed binary and bolt-specific directories

Observed under `/opt/bolt-v2`:

- `bolt-v2`
- `stream_to_lake`
- `config/live.toml`
- `config/live.toml.bak.20260419T072507Z`
- `config/live.toml.bak.20260419T072551Z`
- multiple `backups/` directories
- multiple `staging/` directories

Binary metadata:

- path: `/opt/bolt-v2/bolt-v2`
- type: `ELF 64-bit LSB pie executable, ARM aarch64, stripped`
- `sha256`:
  `8484bb19838d07c116bfcda9d248025ba791c68551bbecefc7d5d6b527523651`

## Rendered runtime config inventory

Captured from the mounted `live.toml` on the cloned root:

- Node:
  - `name = "BOLT-V2-ETH-001"`
  - `trader_id = "BOLT-ETH-001"`
  - `environment = "Live"`
  - `load_state = false`
  - `save_state = false`
- Logging:
  - `stdout_level = "Info"`
  - `file_level = "Debug"`
- Write paths:
  - `[raw_capture] output_dir = "var/raw"`
  - `[audit] local_dir = "var/audit"`
- Polymarket runtime:
  - `subscribe_new_markets = true`
  - `update_instruments_interval_mins = 60`
  - `gamma_refresh_interval_secs = 5`
  - `gamma_event_fetch_max_concurrent = 8`
  - `ws_max_subscriptions = 200`
- Exec client:
  - `account_id = "POLYMARKET-001"`
  - `signature_type = 2`
  - funder address configured
- Secrets:
  - SSM parameter paths only
  - region `eu-west-1`
- Strategy:
  - `type = "eth_chainlink_taker"`
  - `warmup_tick_count = 50`
  - `book_impact_cap_bps = 15`
  - `vol_min_observations = 20`
  - `forced_flat_stale_chainlink_ms = 1500`
  - `lead_jitter_max_ms = 250`
  - `max_position_usdc = 25.0`
  - `worst_case_ev_min_bps = 50`
  - `exit_hysteresis_bps = 20`
  - `theta_decay_factor = 1.0`
- Reference:
  - `publish_topic = "platform.reference.default"`
  - Chainlink and Binance shared config present
  - seven ETH reference venues configured
  - venue staleness `1000ms`, disable window `5000ms`
- Ruleset:
  - `id = "ETHCHAINLINKTAKER"`
  - `resolution_basis = "chainlink_ethusd"`
  - `min_time_to_expiry_secs = 30`
  - `max_time_to_expiry_secs = 300`
  - `freeze_before_end_secs = 35`
  - `selector_poll_interval_ms = 1000`
  - `candidate_load_timeout_secs = 30`
  - selector tag `ethereum`
  - selector prefix `eth-updown-5m`

This ties the host inventory directly to the trading lane that actually ran on the damaged host.

## Installed packages, sysctl, and limits inventory

Package facts from the cloned root:

- package count: `615`
- key packages present:
  - `cloud-init 25.3-0ubuntu1~22.04.1`
  - `e2fsprogs 1.46.5-2ubuntu1.2`
  - `rsyslog 8.2112.0-2ubuntu2.2`
  - `snapd 2.73+ubuntu22.04.1`
  - `systemd 249.11-0ubuntu3.19`
  - `util-linux 2.37.2-4ubuntu3.5`
- `awscli` did not report as an installed Debian package via `dpkg-query`

Observed sysctl sources on the cloned root:

- `/etc/sysctl.conf`
- `/etc/sysctl.d/10-console-messages.conf`
- `/etc/sysctl.d/10-ipv6-privacy.conf`
- `/etc/sysctl.d/10-kernel-hardening.conf`
- `/etc/sysctl.d/10-magic-sysrq.conf`
- `/etc/sysctl.d/10-network-security.conf`
- `/etc/sysctl.d/10-ptrace.conf`
- `/etc/sysctl.d/10-zeropage.conf`
- `/etc/sysctl.d/50-cloudimg-settings.conf`
- `/etc/sysctl.d/99-cloudimg-ipv6.conf`
- `/etc/sysctl.d/99-sysctl.conf`

Notable non-commented sysctl entries present:

- `kernel.printk = 4 4 1 7`
- `kernel.kptr_restrict = 1`
- `kernel.sysrq = 176`
- `kernel.yama.ptrace_scope = 1`
- `net.ipv4.conf.default.rp_filter = 2`
- `net.ipv4.conf.all.rp_filter = 2`
- `net.ipv4.neigh.default.gc_thresh2 = 15360`
- `net.ipv4.neigh.default.gc_thresh3 = 16384`
- `net.netfilter.nf_conntrack_max = 1048576`
- `vm.mmap_min_addr = 32768`

Limits inventory:

- `/etc/security/limits.conf` present but left at stock commented defaults
- no custom files found under `/etc/security/limits.d/`
- `/etc/systemd/system.conf` shows only commented defaults, including:
  - `#DefaultLimitNOFILE=1024:524288`
- no bolt-specific `Limit*` entries were present in the mounted unit file

## Timers, cron, and auxiliary services inventory

Enabled services visible from symlinks under the cloned root include:

- `bolt-v2.service`
- `cron.service`
- `rsyslog.service`
- `ssh.service`
- `systemd-networkd.service`
- `systemd-resolved.service`
- `ufw.service`
- `unattended-upgrades.service`
- `snap.amazon-ssm-agent.amazon-ssm-agent.service`
- multiple snap mount units

Enabled timers include:

- `logrotate.timer`
- `apt-daily.timer`
- `apt-daily-upgrade.timer`
- `fstrim.timer`
- `e2scrub_all.timer`
- `snapd.snap-repair.timer`
- `ua-timer.timer`
- `update-notifier-download.timer`
- `update-notifier-motd.timer`

Cron files present:

- `/etc/cron.d/e2scrub_all`
- `/etc/cron.daily/apport`
- `/etc/cron.daily/apt-compat`
- `/etc/cron.daily/dpkg`
- `/etc/cron.daily/logrotate`
- `/etc/cron.daily/man-db`
- `/etc/cron.weekly/man-db`

No user crontabs were found under `/var/spool/cron*`.

## Write-surface inventory from the cloned root

Observed sizes:

- `/var/logs`: `2.0G`
- `/var/log`: `2.0G`
- `/var/lib/amazon/ssm`: `714M`
- `/var/audit`: `11M`
- `/data`: `39M`

Observed file counts:

- `/var/logs`: `300` files
- `/var/audit`: `13` files
- `/var/lib/amazon/ssm`: `1170` files

This confirms the pre-remediation runtime had large write surfaces on root and that SSM retained a
material number of artifacts locally.

## What this inventory proves for parity review

- The old production lane was not just “an EC2 instance with the same binary.”
- It had a specific end-to-end shape:
  - root-only mount layout
  - enabled `bolt-v2.service`
  - no `WorkingDirectory`
  - no dedicated service user
  - root-owned config and runtime paths
  - relative `raw_capture` and `audit` paths in the rendered runtime config
  - `file_level = "Debug"` plus a large `/var/logs` tree
  - active cron/timer/service background behavior from Ubuntu, rsyslog, snap, and SSM

Any replacement host claiming parity has to account for that entire path, not just copy the binary
and config.

## Remaining limits of this capture

- Because live `RunShellScript` is still broken on the production host, this artifact does **not**
  prove the host’s current in-memory effective state for:
  - `systemctl show` output
  - live process cwd
  - live effective limits at runtime
  - current active service state beyond what is inferable from symlinks, AWS state, and logs
- Interactive Session Manager command mode was not available from this workstation because the local
  plugin lacked the `InteractiveCommands` step.

Even with those limits, this inventory is strong enough to compare a rebuilt host against the
captured on-disk service/config/filesystem baseline and the AWS identity boundary of the damaged
production lane.
