# T044 Tiny-Capital Canary

Status: not executed.

T044 remains gated on renewed explicit operator approval because it is a live tiny-capital canary. The canary may submit at most one live order under the configured bounds:

- `max_live_order_count = 1`
- `max_notional_per_order = "1.00"`

## Current-Head Non-Live Preflight

Preflight head: `c6fe228c5aa4807d704bdae904f8695220b64dd5`

The previously configured ignored `config/live.local.toml` operator packet still binds reviewed code head `8b95eca9c2f410ff462954cff90c4734d01593cb`, so the stale packet correctly fails current-head pre-run verification:

- Command: `cargo run --locked --bin bolt-v2 -- operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t042-review-repair-final-refresh/operator-evidence-packet-8b95eca9.json --verification-stage pre-run`
- Result: failed closed with `[live_canary.operator_evidence].head_sha does not match build head_sha`.

A current-head packet was assembled and verified on a temporary config copy under `/private/tmp/bolt-v2-t044-preflight-c6fe228c` without mutating the ignored live TOML:

- Copied root config: `/private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml`
- Copied relative strategy config: `/private/tmp/bolt-v2-t044-preflight-c6fe228c/strategies/binary_oracle.local.toml`
- `operator-artifacts generate-base-static --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --output-dir /private/tmp/bolt-v2-t044-preflight-c6fe228c/base-static --strategy-instance-id bitcoin_updown_main`: passed.
- `operator-artifacts generate-operator-evidence-json`: wrote `/private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-c6fe228c.json`, sha256 `a47c59e5f49a00fa203360b6a2e7cb613363d3a35dc488a71f048fce6d7c35d1`.
- `operator-artifacts update-operator-evidence-toml --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --operator-evidence-json /private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-c6fe228c.json --max-operator-evidence-json-bytes 65536`: passed; temp root TOML sha256 `97264654371a1dd9467bc74ea42e58ac443b6579974809b1b032caf702a18012`.
- `operator-artifacts write-manifest-from-operator-evidence --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --output /private/tmp/bolt-v2-t044-preflight-c6fe228c/static-artifacts-manifest-c6fe228c.json`: passed; manifest sha256 `a4dc04d7f8dbd5a7d3210a7a5c8ea5dae78bb45227785047adef1ea1358c782d`.
- `operator-artifacts assemble-final --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --static-manifest /private/tmp/bolt-v2-t044-preflight-c6fe228c/static-artifacts-manifest-c6fe228c.json --operator-packet /private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-packet-c6fe228c.json`: passed.
  - approval envelope sha256: `ef690d8e1834b30f30fda3b9dc187ce9704db1a56a86e77a7cf1c60797f85201`
  - operator packet sha256: `32261a754f30967b84b66edad60cf710c3bcaac40d6e72e8400384dc59b4527d`
  - static manifest sha256: `a4dc04d7f8dbd5a7d3210a7a5c8ea5dae78bb45227785047adef1ea1358c782d`
- `operator-artifacts verify-final --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --operator-packet /private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-packet-c6fe228c.json --verification-stage pre-run`: passed and verified the hashes above.

Scope and side effects: this was non-live artifact generation and verification only. It did not run `bolt-v2 run`, submit/cancel orders, transfer funds, mutate on-chain state, mutate CLOB allowance/cache state, display secrets, or patch the ignored real `config/live.local.toml`. The temporary approval window used for preflight is not reusable for the live canary; the real ignored live TOML must be refreshed after explicit operator approval and immediately re-verified before T044 execution.
