# T038 Binance Operator Proof Packet

Date: 2026-05-21
Scope: T038 no-submit readiness blocker only.

This packet records the exact proof needed before rerunning T038 against
`config/live.local.toml`. It is not a readiness claim, not a canary approval,
and not production live-trading approval.

## Current Anchors

- Worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit`
- Current head: `ea846e657c2213bc8577430d00a994ab21f2c011`
- `origin/main`: `d55ccfefc316928a67a23cf076c8b7e584e011bd`
- PR #388: open, head `ea846e657c2213bc8577430d00a994ab21f2c011`, base `831368756bf5a7f8398944502dcce5fcc7c7952d`, merge state `CLEAN`
- Speckit tasks still open: T038 and T046 only
- T046 remains blocked until T038 produces a satisfied no-submit report accepted by the canary gate
- `origin/main` advanced after the 2026-05-20 end-to-end trace; this packet records the 2026-05-21 fetched state.

## Proven On Current Head

These current-head checks printed no secret values:

- `git fetch origin`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- `gh pr view 388 --repo seungpyoson/bolt-v2 --json state,headRefOid,baseRefOid,mergeCommit,url,mergeStateStatus,isDraft`
- `gh pr checks 388 --repo seungpyoson/bolt-v2`
- `rg -n "^- \[ \]" specs/001-thin-live-canary-path/tasks.md`
- `stat -f '%N %Sp %z' config/live.local.toml`
- `shasum -a 256 config/live.local.toml`
- `cargo run --quiet --bin bolt-v2 -- secrets check --config /Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml`
- `cargo run --quiet --bin bolt-v2 -- secrets resolve --config /Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml`
- `cargo test maps_binance_client_data_block_from_fixture -- --nocapture`
- `cargo test --test bolt_v3_adapter_mapping binance_data_client_config_plus_resolved_secrets_maps_to_nt_native_fields -- --nocapture`

Evidence:

- `config/live.local.toml` is mode `0600`, size `5024`, SHA-256 `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`.
- `secrets check` reported required fields for `binance_reference` and `polymarket_main`.
- `secrets resolve` reported both clients resolved successfully.
- `src/main.rs` resolves secrets through `SsmResolverSession::new()` and prints only client-level success.
- `src/bolt_v3_providers/binance.rs` resolves `api_secret_ssm_path`, validates the Ed25519 PKCS#8 shape accepted by the NT Binance adapter, then resolves `api_key_ssm_path`.
- `src/bolt_v3_providers/binance.rs` maps resolved Binance secrets into NT `BinanceDataClientConfig.api_key` and `.api_secret`, and maps configured HTTP/WebSocket URLs.
- The targeted adapter tests pass and assert configured Binance SBE endpoint plus resolved key/secret values are passed into NT config.

Conclusion: the repo-local config/SSM-to-NT mapping path is proven at current
head. This does not prove Binance accepts the configured key.

## Still Not Proven

T038 is still blocked. The last approved absolute-path no-submit attempt at
`c4f65cdc3f68f23668c8be37da7270df8bc4f167` failed `controlled_connect`,
skipped `reference_readiness`, and produced only blocker evidence. Binance SBE
rejected the WebSocket handshake with `Invalid X-MBX-APIKEY header`; the
read-only signed `/api/v3/account` probe returned HTTP `401` with Binance code
`-2015`.

Official Binance docs classify `-2015` as invalid API key, IP, or permissions.
Official Binance SBE docs say SBE market data requires an API key in
`X-MBX-APIKEY`, only Ed25519 keys are allowed, no extra public-market-data
permissions are needed, and IP whitelist restrictions still apply.
Sources: <https://raw.githubusercontent.com/binance/binance-spot-api-docs/master/errors.md>
and <https://raw.githubusercontent.com/binance/binance-spot-api-docs/master/sbe-market-data-streams.md>.

Therefore the remaining blocker set is:

- wrong configured SSM parameter target
- Binance API key and private key not paired
- Binance API key inactive, deleted, disabled, or wrong account
- IP whitelist does not allow this runner host
- account/environment mismatch
- Binance-side key state or permission issue

No production-code change is justified by current evidence.

## Required Operator Proof Before Next T038

Do not paste API keys, secret values, private keys, raw SSM paths, account
balances, or screenshots containing secrets into repo or chat.

Required non-secret proof:

1. Confirm the Binance API key in the operator console is an Ed25519 key for the intended production account/environment.
2. Compare the configured public-key fingerprint from the prior non-secret probe, `sha256=1d29db2eb2abf9f63afc99dd580125d83c9966a94e38d875f7adf0e5581c3df9`, against the public key attached to the Binance API key. Record only match/mismatch, not the public key.
3. Confirm the configured API key identifier in SSM points to that same Binance console key. Record only a non-secret hash/fingerprint or match/mismatch, not the raw key.
4. Confirm the key is active and not deleted, expired, disabled, or restricted to a different account.
5. Confirm the runner host's current outbound IP is allowed by the Binance API key whitelist, or that no whitelist is enabled. Record only allow/deny result and non-secret IP hash if needed.
6. Confirm account/environment matches `https://api.binance.com` and `wss://stream-sbe.binance.com/ws`.
7. If any mismatch is found, update SSM as one paired credential lifecycle change and rerun `secrets check` plus `secrets resolve` before any live no-submit attempt.

Only after this proof exists should T038 be rerun with an absolute config path
and a fresh exact-head report. T038 can be checked only if the report satisfies
`controlled_connect`, `reference_readiness`, `controlled_disconnect`,
`report_write`, and canary-gate acceptance. T046 stays unchecked until then.
