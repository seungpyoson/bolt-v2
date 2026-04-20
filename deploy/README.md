# bolt-v2 Deployment

Operator runbook for the bolt-v2 host. Every step is actionable; nothing here is aspirational.

---

## 1. Host Layout

**Root volume (OS only)**

| Path | Contents |
|---|---|
| `/opt/bolt-v2/bolt-v2` | Compiled binary |
| `/opt/bolt-v2/config/live.toml` | Rendered runtime config |
| `/etc/systemd/system/bolt-v2.service` | Systemd unit (installed by `install.sh`) |
| `/etc/systemd/journald.conf.d/bolt-v2.conf` | Journald cap drop-in (installed by `install.sh`) |

The root volume is **OS-only**. No runtime data, no logs, no writable bolt state lives here.

**Data volume (`/srv/bolt-v2`)**

| Path | Contents |
|---|---|
| `/srv/bolt-v2/var/raw/` | Raw market data written by the LiveNode |
| `/srv/bolt-v2/var/audit/` | Audit trail records |
| `/srv/bolt-v2/var/state/` | Persistent engine state |

The data volume is a separate EBS volume, labeled `bolt-v2-data`, mounted at `/srv/bolt-v2` via fstab with `nofail`. The bolt service's `WorkingDirectory` is `/srv/bolt-v2`; all relative-path writes land here, not on the root volume.

---

## 2. Provisioning Runbook

### 2.1 Launch the instance

```
AMI:              ami-037d87f13f7e014c5
Instance type:    c7g.large
Subnet:           subnet-2c4fd44b
Security group:   sg-08921a4b725682171
Instance profile: bolt-polymarket-exec-role
```

Add a **dedicated EBS data volume** at launch (recommended: 20 GiB gp3). Note the device name assigned by EC2 (typically `/dev/nvme1n1` on Nitro instances).

### 2.2 Upload the binary

```bash
scp -i <key> target/aarch64-unknown-linux-gnu/release/bolt-v2 ec2-user@<host>:/tmp/bolt-v2
ssh -i <key> ec2-user@<host> "sudo install -D -m 0755 /tmp/bolt-v2 /opt/bolt-v2/bolt-v2"
```

### 2.3 Upload the runtime config

```bash
scp -i <key> rendered/live.toml ec2-user@<host>:/tmp/live.toml
ssh -i <key> ec2-user@<host> "sudo install -D -m 0640 /tmp/live.toml /opt/bolt-v2/config/live.toml"
```

### 2.4 Upload the repo and run the installer

```bash
# Upload repo to a staging path on the host
rsync -av --exclude target/ . ec2-user@<host>:/tmp/bolt-v2-repo/

# Run the idempotent installer (replace /dev/nvme1n1 with the actual device)
ssh -i <key> ec2-user@<host> "sudo BOLT_DATA_DEVICE=/dev/nvme1n1 bash /tmp/bolt-v2-repo/deploy/install.sh"
```

The installer:
- Creates the `bolt` system user if absent
- Formats and mounts the data volume (idempotent)
- Writes the fstab entry (idempotent)
- Creates `/srv/bolt-v2/var/{raw,audit,state}` with correct ownership
- Installs the systemd unit and journald drop-in
- Enables `bolt-v2.service` (does not start it)

### 2.5 Start the service

Once the binary and config are confirmed in place:

```bash
ssh -i <key> ec2-user@<host> "sudo systemctl start bolt-v2"
```

---

## 3. Verification

Check the service is running and writing to the journal (not the root volume):

```bash
# Recent log output
journalctl -u bolt-v2 -n 200 --no-pager

# Data volume usage (should grow over time; root volume should not)
df -h /srv/bolt-v2

# Root volume usage (should stay flat)
df -h /
```

If the root volume is unexpectedly full, check:

```bash
journalctl --disk-usage
du -sh /var/lib/amazon/ssm/
```

---

## 4. Runtime Discipline — SSM Hygiene

Every `aws ssm send-command` must pass `--output-s3-bucket-name` and `--output-s3-key-prefix`.

Without these flags, SSM retains command stdout under `/var/lib/amazon/ssm` on the instance. This is how 714 MiB accumulated during the 2026-04-20 root-volume incident, filling the root volume and blocking the binary upload that would have restarted trading.

S3-backed output also survives host loss, which raw SSM instance storage does not.

```bash
# Correct — output goes to S3
aws ssm send-command \
  --instance-ids i-XXXXXXXXXXXXXXXXX \
  --document-name AWS-RunShellScript \
  --output-s3-bucket-name <your-bucket> \
  --output-s3-key-prefix ssm-output/ \
  --parameters commands=["journalctl -u bolt-v2 -n 100 --no-pager"]

# Wrong — output accumulates on the instance root volume
aws ssm send-command \
  --instance-ids i-XXXXXXXXXXXXXXXXX \
  --document-name AWS-RunShellScript \
  --parameters commands=["journalctl -u bolt-v2 -n 100 --no-pager"]
```

**On-call first checks:** `journalctl --disk-usage` and `df -h /`. Those two numbers tell you whether the problem is journald or SSM accumulation.

---

## 5. Why No File Logging

NT's kernel constructs its `FileWriterConfig` with `::default()`, which sets no output directory. With no directory, file logs land in the process working directory — `/srv/bolt-v2` — which is the data volume, not the root volume. However, this behaviour is undocumented and fragile.

The validator therefore **refuses any `logging.file_level` other than `"Off"`**. Use `stdout_level` for runtime log verbosity; logs are captured by journald via `StandardOutput=journal` and `StandardError=journal` in the service unit. Retrieve them with `journalctl -u bolt-v2`.

This keeps the audit trail on the data volume (journald on the OS volume, but capped to 500 MiB by the installed drop-in) and prevents any surprise root-volume fill from unbounded file logging.

---

## 6. Forensic Resources — Do Not Remove

The following resources from the 2026-04-20 incident are preserved for post-incident analysis. Do not terminate, delete, or detach until explicitly decided.

| Resource | ID | Notes |
|---|---|---|
| Broken host | `i-08dee6aefe9a5b02c` | Instance that filled root volume |
| Helper instance | `i-0bd63c88cd82e3b35` | Used for forensic access |
| EBS snapshot | `snap-0c5f98e69aa4ac0bd` | Snapshot of broken root volume |
| Clone volume | `vol-0d24ceef69cd27b33` | Cloned from snapshot for safe inspection |
