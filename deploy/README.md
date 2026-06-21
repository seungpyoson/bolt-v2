# Deploy Notes

`deploy/install.sh` provisions the data volume mount at `/srv/bolt-v2`, creates the runtime
directories under `/srv/bolt-v2/var`, installs `deploy/systemd/bolt-v2.service`, and installs the
minimal journald cap drop-in.

The install paths live in exactly one place, `deploy/install-layout.env` (sourced by
`deploy/install.sh`). `deploy/systemd/bolt-v2.service` is a **generated** artifact rendered from
that layout and `deploy/systemd/bolt-v2.service.in` via `just generate-unit`; never hand-edit the
unit (drift is caught by source-fence).

Recommended sequence:

1. Copy the prebuilt binary to `/opt/bolt-v2/bolt-v2` with mode `0755`; do not use the EC2
   instance as a Rust build/cache host for the live service.
2. Copy the selected tracked production overlay, the base template, and the strategy files to the instance:
   `/opt/bolt-v2/config/profiles/<profile-id>.overlay.toml`, `/opt/bolt-v2/config/root.toml`, and
   `/opt/bolt-v2/config/strategies/`. The profile ID derives the overlay path under
   `/opt/bolt-v2/config/profiles/`; root and strategy paths derive from `/opt/bolt-v2/config`.
3. Export the selected tracked profile ID for the deploy shell:
   `BOLT_LIVE_PROFILE=<profile-id>`.
4. Generate the runtime config by composing the derived overlay onto root — never hand-edit `live.toml`
   (issue #768):
   `/opt/bolt-v2/bolt-v2 ops generate-live-config --profile "${BOLT_LIVE_PROFILE}" --config-root /opt/bolt-v2/config`
5. Verify it re-composes from the approved profile+root and still loads against the deployed binary
   before any start (byte parity + independent schema load — catches stale-key drift that hash
   parity alone does not):
   `/opt/bolt-v2/bolt-v2 ops verify-live-config --profile "${BOLT_LIVE_PROFILE}" --config-root /opt/bolt-v2/config`
6. Keep the config readable by the service user, for example `root:bolt` with mode `0640`.
7. Run `sudo BOLT_DATA_DEVICE=/dev/<data-volume-device> ./deploy/install.sh`.
8. Write `/etc/bolt-v2/live.env` with the same selected profile ID:
   `BOLT_LIVE_PROFILE=<profile-id>`. The systemd unit fails closed if this file is missing or the
   profile ID is empty, malformed, unknown, or cannot generate/verify `/opt/bolt-v2/config/live.toml`.
9. Enable and start the service after the binary and config are in place.

## Pre-arm gate (issue #768)

Generate the runtime config first, then start through the binary-owned launch lane.
The launch lane runs the start-time pre-arm verification in order and the service must not
enter the run loop until all pass:

1. `bolt-v2 ops generate-live-config --profile "${BOLT_LIVE_PROFILE}" --config-root /opt/bolt-v2/config`
   — compose the derived overlay onto root into the runtime config (fail-closed on TOML/schema/composition/invariant errors).
2. `bolt-v2 ops launch --profile "${BOLT_LIVE_PROFILE}" --config-root /opt/bolt-v2/config`
   — verifies config identity, checks secret configuration, performs the `secrets resolve` step to
   resolve every SSM credential without printing values, checks storage/catalog readiness
   (catalog-prefix containment, non-symlink catalog dir, write probe, free space ≥ `min_free_bytes`),
   proves reference-current-price health, then enters the live run loop.

Arming-gate/readiness (#768 step 3d) is structural, not a `prestart-check` duty: the bot starts disarmed
and will not submit orders until explicitly armed via the operator arming gate
(`bolt-v2 provider-artifacts preflight-live-submit-arming` / `generate-live-submit-approval`). Data-client
readiness is exercised at live-node startup and can be probed with `bolt-v2 ops data-client-probe`.

Config identity is covered by `ops verify-live-config`, CI, and the first stage of `ops launch`;
secret resolution, exact-binary config-load/storage checks, and reference health are live checks that
cannot run offline.

The systemd unit **enforces** the launch lane at every start: `ExecStart` runs `ops launch`, so a
hand-edited or stale `/opt/bolt-v2/config/live.toml` — including one with loss rails disabled — fails
service start instead of trading. The deployed config is bound to the PR-reviewed Git revision
**procedurally**, not by a binary-embedded anchor: `verify` re-composes the runtime config from the
on-box derived overlay+root, requires the deployed bytes to be byte-identical to that re-composition,
and independently loads them against the deployed binary's schema. No profile is baked into the binary
(a binary-embedded `include_str!` anchor does not scale to a multi-strategy/venue fleet and is
superseded by arming bound to the deployed config checksum plus a signed release manifest — #768
follow-up). Deploy the binary, the overlay, the base `root.toml`, and the strategy files from the same
reviewed release so the on-box bytes are exactly what CI loaded.

`deploy/install.sh` repairs the **entire** deployed config bundle — the tracked overlay, its base
`root.toml`, the generated `live.toml`, and every `config/strategies/*.toml` — to `root:bolt` with
group-readable modes (config, profiles, and strategies dirs `0750`, TOML files `0640`). The installer
rejects symlinked config bundle paths before repairing ownership or modes. The service user runs `ops launch`, so this keeps the bundle readable regardless of the
umask under which it was copied (a restrictive umask would otherwise leave root-copied files
`0600 root:root` and fail start).

The systemd unit refuses to start unless `ops launch` can verify the deployed config, resolve
secrets, pass the Rust prestart check against `/opt/bolt-v2/config/live.toml`, and observe the
configured reference-current-price sources. The prestart check requires `persistence.catalog_directory`
to stay under the TOML-configured `persistence.required_catalog_prefix` and requires the catalog
filesystem to have at least the TOML-configured `persistence.min_free_bytes` available.
For live EC2 operation, start Bolt through the systemd unit or `just live`; direct
`bolt-v2 run --config ...` is disabled for live arming: it refuses to start the node and redirects
operators to `bolt-v2 ops launch --profile <profile-id> --config-root <config-root>`.

## Supervised live checklist

Before any supervised live run, record one evidence packet that ties together:

1. The exact Git head, binary checksum, `BOLT_LIVE_PROFILE`, generated config checksum, and
   `ops verify-live-config` result.
2. `ops launch` stage logs from the deployed instance, including secret resolution, prestart-check, and reference-current-price-health.
3. The configured loss-governor and kill-switch caps from the generated `live.toml`, plus the
   operator-approved arming artifact checksum.
4. A rehearsed abort path: the operator who can remove submit authorization, stop the service, and
   preserve logs/reports.
5. Evidence retention paths for journald, runtime reports, decision evidence, raw capture, and catalog
   data.

Do not start the service for a supervised run until that packet exists and the operator explicitly
approves proceeding.

Before a live run, inspect the instance storage state:

```bash
df -h / /srv/bolt-v2 /var/log /run
sudo journalctl --disk-usage
sudo du -sh /srv/bolt-v2/var/* 2>/dev/null
grep -n 'catalog_directory\|required_catalog_prefix\|min_free_bytes' /opt/bolt-v2/config/live.toml
```

The journald drop-in caps journald storage only. Runtime catalog, raw, audit, report, and decision
evidence retention still needs a Bolt runtime retention policy.
