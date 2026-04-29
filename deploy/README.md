# Deploy Notes

`deploy/install.sh` provisions the data volume mount at `/srv/bolt-v2`, creates the runtime
directories under `/srv/bolt-v2/var`, installs `deploy/systemd/bolt-v2.service`, and installs the
minimal journald cap drop-in.

Recommended sequence:

1. Copy the binary to `/opt/bolt-v2/bolt-v2` with mode `0755`.
2. Copy the rendered runtime config to `/opt/bolt-v2/config/live.toml`.
3. Keep the config readable by the service user, for example `root:bolt` with mode `0640`.
4. Run `sudo BOLT_DATA_DEVICE=/dev/<data-volume-device> ./deploy/install.sh`.
5. Enable and start the service after the binary and config are in place.

If `/opt/bolt-v2/config/live.toml` already exists, `deploy/install.sh` repairs it to `root:bolt`
with mode `0640`.
