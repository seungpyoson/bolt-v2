# Deploy Notes

`deploy/install.sh` provisions the data volume mount at `/srv/bolt-v2`, creates the runtime
directories under `/srv/bolt-v2/var`, installs `deploy/systemd/bolt-v2.service`, and installs the
minimal journald cap drop-in.

Recommended sequence:

1. Copy the prebuilt binary to `/opt/bolt-v2/bolt-v2` with mode `0755`; do not use the EC2
   instance as a Rust build/cache host for the live service.
2. Copy the tracked production overlay, its base template, and the strategy files to the instance:
   `/opt/bolt-v2/config/profiles/prod-btc-5m.overlay.toml`, `/opt/bolt-v2/config/root.toml`, and
   `/opt/bolt-v2/config/strategies/`. The overlay's `base = "../root.toml"` resolves relative to the
   overlay, and both the generated runtime config and the overlay reference strategy files by relative
   path, so the base and `strategies/` must sit under `/opt/bolt-v2/config/` alongside the deployed config.
3. Generate the runtime config by composing the overlay onto its base — never hand-edit `live.toml`
   (issue #768):
   `/opt/bolt-v2/bolt-v2 ops generate-live-config --profile /opt/bolt-v2/config/profiles/prod-btc-5m.overlay.toml --output /opt/bolt-v2/config/live.toml`
4. Verify it re-composes from the approved overlay+base and still loads against the deployed binary
   before any start (byte parity + independent schema load — catches stale-key drift that hash
   parity alone does not):
   `/opt/bolt-v2/bolt-v2 ops verify-live-config --profile /opt/bolt-v2/config/profiles/prod-btc-5m.overlay.toml --deployed /opt/bolt-v2/config/live.toml`
5. Keep the config readable by the service user, for example `root:bolt` with mode `0640`.
6. Run `sudo BOLT_DATA_DEVICE=/dev/<data-volume-device> ./deploy/install.sh`.
7. Enable and start the service after the binary and config are in place.

## Pre-arm gate (issue #768)

The full pre-arm verification runs in order; the service must not start until all pass:

1. `bolt-v2 ops generate-live-config --profile config/profiles/prod-btc-5m.overlay.toml --output /opt/bolt-v2/config/live.toml`
   — compose the overlay onto its base into the runtime config (fail-closed on TOML/schema/composition/invariant errors).
2. `bolt-v2 ops verify-live-config --profile config/profiles/prod-btc-5m.overlay.toml --deployed /opt/bolt-v2/config/live.toml`
   — byte parity vs the re-composed config + independent schema load + strategy-file content match.
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
one with loss rails disabled — fails service start instead of trading. The deployed config is bound to the
PR-reviewed Git revision **procedurally**, not by a binary-embedded anchor: `verify` re-composes the
runtime config from the on-box overlay+base, requires the deployed bytes to be byte-identical to that
re-composition, and independently loads them against the deployed binary's schema. No profile is baked
into the binary (a binary-embedded `include_str!` anchor does not scale to a multi-strategy/venue fleet and
is superseded by arming bound to the deployed config checksum plus a signed release manifest — #768
follow-up). Deploy the binary, the overlay, the base `root.toml`, and the strategy files from the same
reviewed release so the on-box bytes are exactly what CI loaded.

`deploy/install.sh` repairs the **entire** deployed config bundle — the tracked overlay, its base
`root.toml`, the generated `live.toml`, and every `config/strategies/*.toml` — to `root:bolt` with
group-readable modes (config and strategies dirs `0750`, TOML files `0640`). The service user runs
`ops verify-live-config` / `prestart-check` / `run`, so this keeps the bundle readable regardless of the
umask under which it was copied (a restrictive umask would otherwise leave root-copied files
`0600 root:root` and fail start).

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
