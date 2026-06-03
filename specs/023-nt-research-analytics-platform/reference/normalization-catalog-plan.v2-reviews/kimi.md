Verdict: SHIP-WITH-CHANGES

**Per-finding table (class-level verdicts)**

| Finding | Verdict | Evidence |
|---|---|---|
| F1 | RESOLVED | §2.2-2.3: Three-tier matrix exhaustively maps every contract family against the pinned NT rev (`catalog.rs:4109-4146`, `config.rs:52-62`); `nt_target` column makes mis-assignment impossible. |
| F2 | RESOLVED | §4.3: `ConditionalCatalogWriter` mandates `PutMode::Create` (If-None-Match: *), content+transform-hash keys, and a concurrency-proof BLOCKER; §4.1 permanently bars NT's `write_to_parquet`. |
| F3 | RESOLVED | §6.1: Canonical four-family taxonomy derived from `contractType` + margin class at normalize time; binding TOML reconciled; fail-loud guard + acceptance test. |
| F4 | RESOLVED | §7.1: OKX `order_book_400` routed to `order_book_snapshots_fixed_depth` + `order_book_snapshot_deltas` with named derivation rule; explicit forbidden_claim against native `order_book_deltas`. |
| F5 | RESOLVED | §7.2: Single authoritative family (`order_book_snapshots_fixed_depth`) from accepted manifest; host corrected to `archive.pmxt.dev/Polymarket/v2`; binding + evidence matrix reconciled. |
| F6 | RESOLVED | §7.3: Deribit `get_index_price` forbidden from `index_prices.event_time`; `funding_history` identified as sole source; `event_time_source` class-level guard applied to all snapshot-only families. |
| F7 | RESOLVED | §6.2: Single three-valu[redacted_source_excerpt]aliased to[redacted_source_excerpt]with[redacted_source_excerpt]exhaustive[redacted_source_excerpt][redacted_source_excerpt]. |
| F8 | RESOLVED | §6.3:[redacted_source_excerpt](never[redacted_source_excerpt]);[redacted_source_excerpt]gate[redacted_source_excerpt][redacted_source_excerpt] |
| F9 | RESOLVED | §8: B[redacted_source_excerpt][redacted_source_excerpt] record; d[redacted_source_excerpt];[redacted_source_excerpt] |
| F10 | RESOLVED | §9: L[redacted_source_excerpt] gate; NT LIST amplification mitigation[redacted_source_excerpt][redacted_source_excerpt]; cost estimate blocks one-year run. |
| F11 | RESOLVED | §11-cred 0.3: Empiric[redacted_source_excerpt]scrubs[redacted_source_excerpt] delta attributes creds correctly. |
| F12 | RESOLVED | §12: S[redacted_source_excerpt]excluded from live binary; f[redacted_source_excerpt]guard. |
| F13 | RESOLVED | §7.4: Explicit[redacted_source_excerpt]("[redacted_source_excerpt]");[redacted_source_excerpt]deferred to[redacted_source_excerpt] |
| F14 | RESOLVED | §5.2: [redacted_source_excerpt][redacted_source_excerpt]phases with[redacted_source_excerpt];[redacted_source_excerpt]d on[redacted_source_excerpt]. |
| F15 | RESOLVED | §4.4: Explicit[redacted_source_excerpt] with exact object enumeration; pointer CAS ([redacted_source_excerpt]`);[redacted_source_excerpt]corded in snapshot, never[redacted_source_excerpt] |

**New findings**

- **NF-1 (BLOCKER): Python write path bypasses[redacted_source_excerpt].** §3 allows Python as an "optional [redacted_source_excerpt]" PyArrow/s3fs has no[redacted_source_excerpt]discipline; a notebook c[redacted_source_excerpt]ly overwrite or race with the Rust writer, voiding F2. The plan provides no FFI, subprocess wrapper, or service boundary to enforce the[redacted_source_excerpt]rule on the Python edge. **Fix:** Make Python read-only against the catalog, or provide a small Python-facing CLI/wrapper that invokes the Rust[redacted_source_excerpt].

- **NF-2 (HIGH):[redacted_source_excerpt]encoder reuse assumes separable NT internals.** §4.3 states it will "[redacted_source_excerpt][redacted_source_excerpt]." This assumes the encoding logic is publicly exposed and separable from the `ObjectStore::put` at[redacted_source_excerpt]. If that function is monolithic or internal, the plan requires forking[redacted_source_excerpt]—an unacknowledged implementation risk that could collapse the F2 resolution. **Fix:** Verify the API boundary before committing; if the encoder is not independently callable, budget for re-implementing the Parquet encoding with `arrow-rs` directly.

- **NF-3 (HIGH): Staged NT-format data lacks mechanical isolation fro[redacted_source_excerpt]** §5.3 permits building an "NT catalog" in staging before proof acceptance, while §5.2 rule 1 forbids projecting into an "[redacted_source_excerpt]" pre-acceptance. The document resolves this tension by saying staging is non-canonical, but there is no IAM, bucket, or path-layout barrier preventing[redacted_source_excerpt]`BacktestNode` from replaying the stage[redacted_source_excerpt]The gate is process-dependent, not mechanical. **Fix:** Stage pre-proof data under a non-NT path layout (e.g., `staged-raw/<family>/`), or enforce IAM so t[redacted_source_excerpt] runtime role cannot LIST th[redacted_source_excerpt].

- **NF-4 (MEDIUM): Runtim[redacted_source_excerpt]capability check is unspecified.** §4.3 asserts the writer checks that[redacted_source_excerpt]at construction, bu[redacted_source_excerpt]0.13.2 does not expose the resolved[redacted_source_excerpt]` configuration on the built `AmazonS3` store. The probe mechanism (e.g., dummy `Create`-and-delete to detect[redacted_source_excerpt]) is not documented. **Fix:** Specify the runtime probe and abort behavior.

- **NF-5 (MEDIUM): Synthetic and provider catalog collision risk.** Phase 0/3 s[redacted_source_excerpt]and Phase 5 provider data may share the same catalog root. Without a separate `catalog-root-synthetic/` URI, a[redacted_source_excerpt]query could commingle synthetic and provider rows if paths overlap. **Fix:** Enforce a distinct[redacted_source_excerpt] root URI in the capability-proof harness.

**Design-level concerns**

1. **[redacted_source_excerpt]NF-1)** is the most likely long-term erosion of write discipline. "Same catalog format" is not "same write discipline." The plan must either forbid Python writes or mediate them through the Rust writer.

2. **NT encoder API coupling (NF-2)**[redacted_source_excerpt]implementation risk. If the buffer interception fails, the entire no-overwrite architecture forks or collapses. A fallback encoding strategy should be documented now.

3. **[redacted_source_excerpt]gate is social, not physical (NF-3).** Because staged data is written in NT layout, a URI misconfiguration would silently replay un-proven bytes. Physical isolation (non-NT staging paths or IAM deny) would make the gate robust against human error.