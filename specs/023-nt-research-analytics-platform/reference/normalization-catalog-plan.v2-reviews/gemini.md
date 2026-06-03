Verdict: REQUEST_CHANGES

### Checklist Assessment
- **1. Verify exact base/head refs and commits before judging the diff:** PASS (Evaluated against provided Head commit 1d9b004a7c465129d42f29baec509c045f267050).
- **2. Review only the declared scope and list any scope gaps as NOT REVIEWED:** PASS (Confined strictly to `normalization-catalog-plan.v2.md`).
- **3. Evaluate correctness bugs, security risks, regressions, and missing tests:** FAIL (Critical correctness bug identified in the canonical read path / filename parsing).
- **4. Check known review comments or residual threads when the prompt includes them:** PASS (F1-F15 explicitly evaluated below).
- **5. Separate blocking findings from non-blocking concerns:** PASS.
- **6. Treat timeout, truncation, interruption, permission block, or shallow output as a failed review slot:** PASS.

### F1–F15 Findings Assessment
| Finding | Status | Evidence / Reasoning |
|---|---|---|
| **F1** | RESOLVED | §2 explicitly defines the 3-tier classification and strictly scopes the replay claim to Tier A (`NautilusDataType`). |
| **F2** | RESOLVED | §4.3 mandates `ConditionalCatalogWriter` with `PutMode::Create` and a concurrency proof, barring NT's non-atomic writer. |
| **F3** | RESOLVED | §6.1 collapses the vocabularies to a single source of truth, deriving the 4 families from `contractType` during normalization. |
| **F4** | RESOLVED | §7.1 removes OKX 400-level from native `order_book_deltas` and enforces it as a derived snapshot family with a `forbidden_claim`. |
| **F5** | RESOLVED | §7.2 anchors PMXT families strictly to the accepted manifest (`order_book_snapshots_fixed_depth`), dropping unproven `bars`. |
| **F6** | RESOLVED | §7.3 introduces a sweeping `event_time_source=none` fail-loud guard for Deribit index and all snapshot-only families. |
| **F7** | RESOLVED | §6.2 locks `write_mode` to 3 values and cleanly separates `staging_location` as an additive field. |
| **F8** | RESOLVED | §6.3 mints deterministic provisional `source_proof_ids` and binds `nt_instrument_id` population strictly to `accepted` mapping status. |
| **F9** | RESOLVED | §8 abandons the window-complete illusion, enforcing a best-effort gap record and a declared symbol-shape parser. |
| **F10** | RESOLVED | §9 grounds volumes in the coverage ledger and blocks the full run on a costed estimate (including requester-pays egress). |
| **F11** | RESOLVED | §11-cred designs a precise negative control that scrubs both `AWS_*` env vars and IMDS to empirically prove SSM-credential attribution. |
| **F12** | RESOLVED | §12 structurally isolates the cloud feature via a separate workspace/lockfile and a f[redacted_source_excerpt]` guard. |
| **F13** | RESOLVED | §7.4 defers the[redacted_source_excerpt]entirely and attaches a explicit `forbidden_claim`. |
| **F14** | RESOLVED | §10.1 (S7/S8) forces[redacted_source_excerpt]proof[redacted_source_excerpt]to pass *before* staging projection or provider replay occurs. |
| **F15** | RESOLVED | §4.4 replaces prefix repointing[redacted_source_excerpt][redacted_source_excerpt][redacted_source_excerpt] pointer CAS to avoid[redacted_source_excerpt]s. |

### Blocking Findings

**1. Contradiction between Canonical Read Path, NT Prefix Listing, and Custom Filenames**
*Evidence:* §4.3.3 mandates custom filenames (`[redacted_source_excerpt]) and §4.4.5 claims "[redacted_source_excerpt]". However, §4.3 also states "NT's [redacted_source_excerpt] is used only[redacted_source_excerpt]".
*Why it blocks:* NT's standard[redacted_source_excerpt]has no awareness of[redacted_source_excerpt]` pointers. It operates via naive prefix listing ([redacted_source_excerpt]) and parses interval bounds strict[redacted_source_excerpt]name format.
- **Crash/Skip:** NT's `filename_to_timestamps` will fail to parse the appended `__t-hash` format, causing the reader to either crash or yield an empty catalog.
- **Data Duplication:** Because[redacted_source_excerpt]writes distinct[redacted_source_excerpt]versions as distinct files *without deleting old ones*, the `nt-catalog/` prefix will accumulate superseded objects. NT's prefix list will load *both* the old and new files fo[redacted_source_excerpt]concurrently, breaking t[redacted_source_excerpt] data stream.
*Resolution:* You must either implement a custom `Catalog` trait reader for[redacted_source_excerpt]that actually resolves the[redacted_source_excerpt] pointers (bypassing [redacted_source_excerpt]` entirely), OR you must write strict NT filenames t[redacted_source_excerpt]prefix and implement a mechanism to explicitly delete superseded objects upon promotion.

### Non-Blocking Concerns / Design Questions

**1. Promotion Materialization Inefficiency (Copy vs Put)**
*Evidence:* §4.4.3 states "[redacted_source_excerpt][redacted_source_excerpt]nt-catalog/... URI with[redacted_source_excerpt]."
*Concern:* If "materialize" implies a standard stream read/write rather than a backend-native copy (like S3 `CopyObject`), promoting Polymarket's 267 GiB will incur massive egress/ingress overhead.[redacted_source_excerpt]lacks a unified cross-prefix atomic copy API; ensure the physical bytes can move efficiently without downloading them to the worker.

**2. Cross-Kind Promotion Atomicity**
*Evidence:* §13.6 leaves "[redacted_source_excerpt]" as an open decision.
*Concern:* `nt_catalog` and `normalized` outputs originating from the same projection run must remain strictly synchronous. If[redacted_source_excerpt]is chosen, a backtest executing mid-promotion could read `nt_catalog` state that is out of sync with the underlying `normalized` tables.

**3. Tier B Funding Path Gap**
*Evidence:* §2.5 correctly notes[redacted_source_excerpt] (not[redacted_source_excerpt]).
*Concern:* As noted in your residual questions, if strategy logic requires historical funding rates to compute accurate PnL, there is currently no wired path to inject them into the backtest engine because they cannot be streamed via[redacted_source_excerpt]