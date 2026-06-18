# Deploy Notes

`deploy/install.sh` provisions the data volume mount at `/srv/bolt-v2`, creates the runtime
directories under `/srv/bolt-v2/var`, installs `deploy/systemd/bolt-v2.service`, and installs the
minimal journald cap drop-in.

Recommended sequence:

1. Copy the prebuilt binary to `/opt/bolt-v2/bolt-v2` with mode `0755`; do not use the EC2
   instance as a Rust build/cache host for the live service.
2. Copy the tracked production profile and its strategy files to the instance, for example
   `/opt/bolt-v2/config/prod-btc-5m.toml` and `/opt/bolt-v2/config/strategies/`. The runtime config
   references its strategy files by relative path, so they must sit alongside it.
3. Generate the runtime config from the profile — never hand-edit `live.toml` (issue #768):
   `/opt/bolt-v2/bolt-v2 ops generate-live-config --profile /opt/bolt-v2/config/prod-btc-5m.toml --output /opt/bolt-v2/config/live.toml`
4. Verify it regenerates from the approved profile and still loads against the deployed binary
   before any start (byte parity + independent schema load — catches stale-key drift that hash
   parity alone does not):
   `/opt/bolt-v2/bolt-v2 ops verify-live-config --profile /opt/bolt-v2/config/prod-btc-5m.toml --deployed /opt/bolt-v2/config/live.toml`
5. Keep the config readable by the service user, for example `root:bolt` with mode `0640`.
6. Run `sudo BOLT_DATA_DEVICE=/dev/<data-volume-device> ./deploy/install.sh`.
7. Enable and start the service after the binary and config are in place.

If `/opt/bolt-v2/config/live.toml` already exists, `deploy/install.sh` repairs it to `root:bolt`
with mode `0640`.

The systemd unit refuses to start unless `/srv/bolt-v2` is mounted and the Rust prestart check
passes against `/opt/bolt-v2/config/live.toml`. That prestart check requires
`persistence.catalog_directory` to stay under the TOML-configured
`persistence.required_catalog_prefix` and requires the catalog filesystem to have at least the
TOML-configured `persistence.min_free_bytes` available.
For live EC2 operation, start Bolt through the systemd unit; direct `bolt-v2 run --config ...`
executes the same storage prestart check before constructing the live node.

Before a live run, inspect the instance storage state:

```bash
df -h / /srv/bolt-v2 /var/log /run
sudo journalctl --disk-usage
sudo du -sh /srv/bolt-v2/var/* 2>/dev/null
grep -n 'catalog_directory\|required_catalog_prefix\|min_free_bytes' /opt/bolt-v2/config/live.toml
```

The journald drop-in caps journald storage only. Runtime catalog, raw, audit, report, and decision
evidence retention still needs a Bolt runtime retention policy.
