#!/usr/bin/env bash
set -euo pipefail

# Idempotent installer for the bolt-v2 host.
# Required env: BOLT_DATA_DEVICE — block device to back /srv/bolt-v2 (e.g. /dev/nvme1n1).

if [[ -z "${BOLT_DATA_DEVICE:-}" ]]; then
    echo "ERROR: BOLT_DATA_DEVICE is not set" >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> bolt-v2 installer — repo root: $REPO_ROOT"
echo "==> data device: $BOLT_DATA_DEVICE"

# --- Create bolt user if it does not exist ---
if ! id bolt &>/dev/null; then
    echo "==> Creating bolt system user"
    useradd --system --home-dir /srv/bolt-v2 --shell /usr/sbin/nologin bolt
else
    echo "==> bolt user already exists"
fi

# --- Format device if it has no filesystem ---
if ! blkid "$BOLT_DATA_DEVICE" &>/dev/null; then
    echo "==> No filesystem detected on $BOLT_DATA_DEVICE — formatting ext4"
    mkfs.ext4 -L bolt-v2-data "$BOLT_DATA_DEVICE"
else
    echo "==> Filesystem already present on $BOLT_DATA_DEVICE"
fi

# --- Create mount point ---
mkdir -p /srv/bolt-v2

# --- Add fstab entry if not already present ---
if ! grep -q 'LABEL=bolt-v2-data' /etc/fstab; then
    echo "==> Adding /etc/fstab entry"
    echo 'LABEL=bolt-v2-data /srv/bolt-v2 ext4 defaults,nofail,x-systemd.device-timeout=30s 0 2' >> /etc/fstab
else
    echo "==> fstab entry already present"
fi

# --- Mount if not already mounted ---
if ! mountpoint -q /srv/bolt-v2; then
    echo "==> Mounting /srv/bolt-v2"
    mount /srv/bolt-v2
else
    echo "==> /srv/bolt-v2 already mounted"
fi

# --- Create runtime directories and set ownership ---
echo "==> Creating runtime directories"
mkdir -p /srv/bolt-v2/var/{raw,audit,state}
chown -R bolt:bolt /srv/bolt-v2/var

# --- Install systemd unit ---
echo "==> Installing bolt-v2.service"
install -m 0644 "$REPO_ROOT/deploy/systemd/bolt-v2.service" /etc/systemd/system/bolt-v2.service

# --- Install journald drop-in ---
echo "==> Installing journald drop-in"
install -d /etc/systemd/journald.conf.d
install -m 0644 "$REPO_ROOT/deploy/systemd/journald-bolt-v2.conf" /etc/systemd/journald.conf.d/bolt-v2.conf

# --- Reload and enable ---
echo "==> Reloading systemd"
systemctl daemon-reload

echo "==> Restarting systemd-journald (applies journald cap)"
systemctl restart systemd-journald

echo "==> Enabling bolt-v2.service (does not start it)"
systemctl enable bolt-v2.service

echo ""
echo "Install complete. Start the service once the binary and config are in place:"
echo "  sudo systemctl start bolt-v2"
