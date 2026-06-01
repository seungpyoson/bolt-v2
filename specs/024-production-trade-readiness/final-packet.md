# T036/T047 Final Packet

Packet refresh build head: `48a9c0df7846c4d08cf7aa877d96cedb8043ee12`

This file records the latest committed packet refresh. A commit that updates this file necessarily changes `HEAD`; rerun `operator-artifacts verify-final` after the final evidence commit before treating the packet as exact-head review evidence.

Root TOML: `config/live.local.toml`

Operator evidence TOML sha256 after T047 exact-head refresh patch: `057170cf556295ff244c13c4327efa0adee445f777dd18a81a39f77f1dc794f3`

Artifact root: `/private/tmp/bolt-v2-t047-final-refresh`

## Source Inputs

| Artifact | Path | SHA-256 |
| --- | --- | --- |
| entry decision source | `/private/tmp/bolt-v2-t036-current-4b/entry-decision-source.json` | `e5d44bc6537c5c4e59e66a9db073c108e93f8229f28259a52316b55b3c377c84` |
| instrument source | `/private/tmp/bolt-v2-t036-current-4b/instrument-source.json` | `845cb4a9326e1a5f7cdd3018df0631cdfb56dabf6df0ea636112081af50122e5` |
| fee-rate source | `/private/tmp/bolt-v2-t036-current-4b/fee-rate-source.json` | `3c34ba73bcef23697f852d418ca57c68abc2b4fb1b66474cb836a0147c3f71f7` |
| decision evidence JSONL | `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/catalog/bolt-v3/decision-evidence/order-intents.jsonl` | `0ff50f02b21aec9355b85c229444914ba5b7db70351a0b7254300825791cf135` |

## Final Artifacts

| Artifact | Path | SHA-256 |
| --- | --- | --- |
| ssm manifest | `/private/tmp/bolt-v2-t047-final-refresh/base-static/ssm-manifest.json` | `76f2f3bac5242c454faabca9452a6cce3219b41252e6021b92b7031a20bb1704` |
| financial envelope | `/private/tmp/bolt-v2-t047-final-refresh/base-static/financial-envelope.json` | `bede2cbe91887660baaf540503ddb5da6fe44981713835cde83bc789de01af39` |
| approval nonce | `/private/tmp/bolt-v2-t047-final-refresh/base-static/approval-nonce.json` | `6227f65a62fa5c7dbf6a71d67e5a184a0c33aa85bc900dfb6f1c526da27c0805` |
| gate session | `/private/tmp/bolt-v2-t036-final-attempt-3/entry-readiness-gate-session.json` | `50d6f47d70c00f37095fb9a6a6754ad0af9416a376b067f2b89abc0fd05ff859` |
| strategy input | `/private/tmp/bolt-v2-t036-final-attempt-3/strategy-input.json` | `8d41468319af3517b945ded1a131f1ec60f94da64719dfa9b5d09cee035c0031` |
| pre-run state | `/private/tmp/bolt-v2-t036-final-attempt-3/pre-run-state.json` | `5b438d57b9ee16920a158a2e68140620460e31c595521771f2622cc7b12951a3` |
| abort plan | `/private/tmp/bolt-v2-t047-final-refresh/abort-plan.json` | `a53e32940bc515618aa0b87309f6477324921a31b30171744ebe7ec1bc94bd8f` |
| operator evidence JSON | `/private/tmp/bolt-v2-t047-final-refresh/operator-evidence-48a9c0df.json` | `5076027fcd27d43fe3f4a3ea708c5cac64b48435bc3e97562baf5e6b091ae31e` |
| static artifacts manifest | `/private/tmp/bolt-v2-t047-final-refresh/static-artifacts-manifest-48a9c0df.json` | `2c0ba198187487449a35dc69dd79e539596f0eeaf1c56ad8f8bd901525b0e0af` |
| approval envelope | `/private/tmp/bolt-v2-t047-final-refresh/approval-envelope-48a9c0df.json` | `94215f1e08fe7fb94dc00f0c7c064c7bd2f188f104051bfe52c6dc81e57fed01` |
| operator evidence packet | `/private/tmp/bolt-v2-t047-final-refresh/operator-evidence-packet-48a9c0df.json` | `e8e985b844c8628ab852606c9ad4d6a605110159bc0b62fb9d9e3d3b7e543e0b` |

## Verification

The previous `/private/tmp/bolt-v2-t036-final-attempt-3/operator-evidence-packet.json` is stale after T047: `operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t036-final-attempt-3/operator-evidence-packet.json --verification-stage pre-run` failed with `operator packet config_bundle_checksum does not match loaded config`.

`operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t047-final-refresh/operator-evidence-packet-48a9c0df.json --verification-stage pre-run` passed and verified the approval envelope, operator evidence packet, and static artifacts manifest hashes above.

No no-submit, tiny-capital canary, submit, cancel, transfer, or trade operation was run for this packet.
