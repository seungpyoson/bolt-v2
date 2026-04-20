#!/usr/bin/env bash
set -euo pipefail

BOLT_USER="${BOLT_USER:-bolt}"
BOLT_GROUP="${BOLT_GROUP:-$BOLT_USER}"
BOLT_HOME="${BOLT_HOME:-/srv/bolt-v2}"
BOLT_INSTALL_ROOT="${BOLT_INSTALL_ROOT:-/opt/bolt-v2}"
BOLT_DATA_DEVICE="${BOLT_DATA_DEVICE:?set BOLT_DATA_DEVICE=/dev/<data-volume-device>}"
BOLT_DATA_FS_TYPE="${BOLT_DATA_FS_TYPE:-ext4}"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SYSTEMD_SRC_DIR="${SCRIPT_DIR}/systemd"
UNIT_DST="/etc/systemd/system/bolt-v2.service"
JOURNALD_DST="/etc/systemd/journald.conf.d/journald-bolt-v2.conf"

if [[ ${EUID} -ne 0 ]]; then
    echo "deploy/install.sh must run as root" >&2
    exit 1
fi

if ! id -u "${BOLT_USER}" >/dev/null 2>&1; then
    useradd --system --home-dir "${BOLT_HOME}" --shell /usr/sbin/nologin "${BOLT_USER}"
fi

if existing_fs_type="$(blkid -s TYPE -o value "${BOLT_DATA_DEVICE}" 2>/dev/null)"; then
    BOLT_DATA_FS_TYPE="${existing_fs_type}"
else
    mkfs -t "${BOLT_DATA_FS_TYPE}" "${BOLT_DATA_DEVICE}"
    udevadm settle
fi

mkdir -p "${BOLT_HOME}"

uuid="$(blkid -s UUID -o value "${BOLT_DATA_DEVICE}")"
if [[ -z "${uuid}" ]]; then
    echo "Failed to retrieve UUID for ${BOLT_DATA_DEVICE}" >&2
    exit 1
fi

fstab_line="UUID=${uuid} ${BOLT_HOME} ${BOLT_DATA_FS_TYPE} defaults,nofail 0 2"
if findmnt --fstab --target "${BOLT_HOME}" >/dev/null 2>&1; then
    existing_source="$(findmnt --fstab --target "${BOLT_HOME}" -no SOURCE | head -n1)"
    existing_fstype="$(findmnt --fstab --target "${BOLT_HOME}" -no FSTYPE | head -n1)"
    if [[ "${existing_source}" != "UUID=${uuid}" || "${existing_fstype}" != "${BOLT_DATA_FS_TYPE}" ]]; then
        echo "Existing /etc/fstab entry for ${BOLT_HOME} does not match ${BOLT_DATA_DEVICE}" >&2
        exit 1
    fi
else
    printf '%s\n' "${fstab_line}" >> /etc/fstab
fi

if ! mountpoint -q "${BOLT_HOME}"; then
    mount "${BOLT_HOME}"
fi

chown "${BOLT_USER}:${BOLT_GROUP}" "${BOLT_HOME}"

install -d -o "${BOLT_USER}" -g "${BOLT_GROUP}" \
    "${BOLT_HOME}/var" \
    "${BOLT_HOME}/var/audit" \
    "${BOLT_HOME}/var/logs" \
    "${BOLT_HOME}/var/raw"
install -d -m 0755 "${BOLT_INSTALL_ROOT}"
install -d -o root -g "${BOLT_GROUP}" -m 0750 "${BOLT_INSTALL_ROOT}/config"

if [[ -f "${BOLT_INSTALL_ROOT}/config/live.toml" ]]; then
    chown root:"${BOLT_GROUP}" "${BOLT_INSTALL_ROOT}/config/live.toml"
    chmod 0640 "${BOLT_INSTALL_ROOT}/config/live.toml"
fi

install -d -m 0755 /etc/systemd/system /etc/systemd/journald.conf.d
install -m 0644 "${SYSTEMD_SRC_DIR}/bolt-v2.service" "${UNIT_DST}"
install -m 0644 "${SYSTEMD_SRC_DIR}/journald-bolt-v2.conf" "${JOURNALD_DST}"

systemctl daemon-reload
systemctl restart systemd-journald
