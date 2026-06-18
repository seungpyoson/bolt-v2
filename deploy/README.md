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

## Pre-arm gate (issue #768)

The full pre-arm verification runs in order; the service must not start until all pass:

1. `bolt-v2 ops generate-live-config --profile config/prod-btc-5m.toml --output /opt/bolt-v2/config/live.toml`
   — produce the runtime config from the tracked profile (fail-closed on schema/invariant errors).
2. `bolt-v2 ops verify-live-config --profile config/prod-btc-5m.toml --deployed /opt/bolt-v2/config/live.toml`
   — byte parity vs the regenerated config + independent schema load + strategy-file content match.
3. `bolt-v2 secrets resolve --config /opt/bolt-v2/config/live.toml` — confirm every SSM credential
   resolves without printing values (#768 step 3c).
4. `bolt-v2 ops prestart-check --config /opt/bolt-v2/config/live.toml` — loads the config through the
   exact deployed binary and checks storage/catalog readiness (catalog-prefix containment, non-symlink
   catalog dir, write probe, free space ≥ `min_free_bytes`). The systemd unit also runs this as
   `ExecStartPre`.

No-submit/readiness (#768 step 3d) is structural, not a `prestart-check` duty: the bot starts disarmed
and will not submit orders until explicitly armed via the operator arming gate
(`bolt-v2 provider-artifacts preflight-live-submit-arming` / `generate-live-submit-approval`). Data-client
readiness is exercised at live-node startup and can be probed with `bolt-v2 ops data-client-probe`.

Steps 1–2 are config identity (covered by `ops verify-live-config` and CI); steps 3–4 are the live
secret-resolution and exact-binary config-load/storage checks that cannot run offline.

The systemd unit **enforces** step 2 at every start: `ExecStartPre` runs `ops verify-live-config` (then
`ops prestart-check`) before `run`, so a hand-edited or stale `/opt/bolt-v2/config/live.toml` — including
one with loss rails disabled — fails service start instead of trading. `verify` also enforces a **release
anchor**: the on-box profile and its strategy files must be byte-identical to the reviewed copy baked into
the deployed binary at build time (`include_str!`), so a stale or hand-edited on-box profile that is
merely self-consistent with its own generated `live.toml` is rejected too. Because the binary is the
CI-built release artifact, this ties the deployed config to the PR-reviewed Git revision — deploy the
binary and config from the same reviewed release.

`deploy/install.sh` repairs the **entire** deployed config bundle — the tracked profile, the generated
`live.toml`, and every `config/strategies/*.toml` — to `root:bolt` with group-readable modes (config and
strategies dirs `0750`, TOML files `0640`). The service user runs `ops verify-live-config` /
`prestart-check` / `run`, so this keeps the bundle readable regardless of the umask under which it was
copied (a restrictive umask would otherwise leave root-copied files `0600 root:root` and fail start).

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
