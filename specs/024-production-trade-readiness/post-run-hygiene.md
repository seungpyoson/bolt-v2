# T045 Post-Run Hygiene

> Closeout sequence and where T045 sits in it: see [`closeout-runbook.md`](closeout-runbook.md) Step 7.

Status: pending T044.

T045 cannot be completed until the T044 tiny-capital canary runs and produces the post-run artifact paths bound in the verified operator packet. This file records the exact post-run hygiene contract that must be satisfied before T045 can be checked off.

## Required Proof

The post-run hygiene proof must be written to the operator-evidence `post_run_hygiene_path` used for the canary packet. Current preflight path:

- `/private/tmp/bolt-v2-t044-preflight-4302d249/post-run-hygiene.json`

The proof must satisfy the current `phase8_assert_post_run_hygiene_proof` contract:

- `record_kind`: `post_run_hygiene`
- `run_id`: matches the T044 canary run id.
- `strategy_instance_id_hash`: matches the approved canary strategy instance id hash.
- `client_order_id_hash`: matches the canary client order id hash.
- `venue_order_id_hash`: matches the canary venue order id hash.
- `raw_secret_residue_absent`: `true`.
- `scanned_artifact_hashes`: non-empty list of sha256 hashes for scanned artifacts/logs.
- `retention_purge_path_hash`: sha256 hash of the retained/purged artifact path record.

## Required Hygiene Pass

After T044 completes, run an artifact/log scan over the T044 output directory and any configured post-run report paths. The scan must prove:

- No API keys, private keys, passphrases, approval ids, non-redacted account balances, or raw secret material are present in retained artifacts/logs.
- Retained artifacts are the minimum set needed for final-packet verification and issue/PR evidence.
- Any purge decision is recorded by hash/path-hash, not by printing secret-bearing paths or contents.

## Current Non-Live State

No T045 scan has been run because T044 has not executed. No post-run hygiene proof exists yet, and T045 remains open.
