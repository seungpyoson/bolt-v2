# Plan v2 — Six-Model Adversarial Review Synthesis

`reviews_of`: `normalization-catalog-plan.v2.md` (content_hash sha256 `746db046564aaca9d0d56cc425e1d94cdce9886a9d065f3bd656e433e8a11d19`, 90,784 B)
`panel`: Codex, GLM, DeepSeek, Kimi (relay), Grok (relay), Gemini (relay) — each an independent single-doc adversarial challenge (F1–F15 RESOLVED/PARTIAL/NOT-RESOLVED + design challenge + missed-items).
`raw_reviews`: `normalization-catalog-plan.v2-reviews/{codex,glm,deepseek,kimi,grok,gemini}.md`
`NT verified at`: rev `6e059dcbb59ac1e582132fc431a581936c216c3c` (the bolt-v2 pin), checkout `USER_HOME_DIR/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6e059dc/`.
`status`: drives plan **v3**. Note the relay reviews contain `[redacted_source_excerpt]` tokens — the relay scrubs source-derived excerpts; finding structure is intact.

## 1. Verdict spread

| Reviewer | Verdict | Original 15 |
|---|---|---|
| GLM | SHIP-WITH-CHANGES | 15/15 RESOLVED |
| DeepSeek | SHIP-WITH-CHANGES | F2, F10, F15 PARTIAL; rest RESOLVED |
| Kimi | SHIP-WITH-CHANGES | 15/15 RESOLVED |
| Grok | REQUEST_CHANGES | F14, F15 PARTIAL; rest RESOLVED |
| Gemini | REQUEST_CHANGES | 15/15 RESOLVED |
| Codex | NO-SHIP | F1,F12 RESOLVED; F7,F14,F15 NOT-RESOLVED; rest PARTIAL |

**Unanimous:** the 15 v1→v2 findings are resolved at the design level. **No reviewer SHIPs unconditionally.** The split (NO-SHIP↔SHIP-WITH-CHANGES) is driven almost entirely by ONE convergent new blocker plus a question of plan-vs-implementation framing.

## 2. THE convergent blocker — catalog read-path incompatibility (B-1)

Raised by **Codex (critical), Gemini (blocker), DeepSeek (blocker), Grok (design concern), Kimi (NF-3)** — five of six independently, the sixth (GLM) adjacent via NF-2.

**Claim.** v2's write/promote design does not produce an NT-readable canonical view and can leak the wrong bytes into `BacktestNode`.

**Self-verification against NT `6e059dc` (I read the source, not the reviews):**
- `query_files` (`catalog.rs:1986`) and `list_parquet_files` (`:1234`) build `data/<type>/` via `make_path` and **naively recursive-`list` the object-store prefix** — every `.parquet` under it, zero pointer/snapshot awareness.
- `parse_filename_timestamps` (`:4626`) does `strip_suffix(".parquet")` → `split_once('_')` → ISO-parse each half. A v2 name `<start>_<end>__t-<hash>__c-<hash>.parquet` parses the second half as `<end>__t-…` → fails → returns `None`.
- `query_intersects_filename` (helper): on `None` it returns **`true`** (file included in *every* requested window). So a custom-named file does **not crash** (Gemini's "crash" mechanism is wrong) — it is **silently included regardless of the query's [start,end]**, and because the list is naive, **all superseded transform versions for an interval load together.** The real failure is worse than a crash: silent over-inclusion of superseded + out-of-window data. This violates fail-loud (CLAUDE.md rule 2).
- `timestamps_to_filename` (`:4175`) = `format!("{ts1}_{ts2}.parquet")` — the exact format the reader expects.

**Three distinct defects fold into B-1:**
1. **Filename format** (F2 regression): hash-suffixed names defeat NT's interval pruning and are over-included.
2. **Pointer never consulted** (F15 hole): NT lists the prefix directly; the artifact-index pointer/snapshot indirection is invisible to it. Canonical bytes are written (PutMode::Create) **before** the pointer CAS, so a failed/lost CAS leaves NT-readable orphans under the canonical prefix.
3. **Interim-staging NT catalog** (F14 hole, §5.3): staging is allowed to generate "normalized tables + an NT catalog" with a provisional `v0-pending` proof — a pending-proof, NT-layout, provider-byte path whose only guard is "humans won't point BacktestNode at it" (social, not mechanical — Kimi NF-3).

**Class fix for v3 (verified-feasible):**
- **(a) Immutable per-commit NT catalog roots.** Materialize the canonical NT catalog ONLY from a *committed* PromotionPackage, into a fresh immutable root keyed by snapshot id, **after** the pointer CAS. The hot pointer names the active root; NT is pointed at that root. A lost CAS leaves an unreferenced root → no orphan is ever readable. (Codex's "immutable NT-compatible snapshot roots.")
- **(b) NT-native filenames** in canonical roots: exact `timestamps_to_filename` (`<start>_<end>.parquet`), interval-disjoint, exactly one live file per interval. Content+transform-hash keying is **staging-only** (NT never reads staging).
- **(c) Staging is Tier-C-only and physically non-NT.** Interim staging never emits a Tier A/B NT-replayable catalog. Staged research data lives under a non-NT path layout (e.g. `staged-research/<family>/…`, never `data/<type>/`) so NT's `make_path`/`query_files` cannot even enumerate it. A manifest/catalog validator rejects `v0-pending`/non-accepted proofs under any Tier A/B NT prefix and before any `BacktestResultContract`.

## 3. Other valid findings (fold into v3)

| # | Severity | Finding | Source | v3 fix |
|---|---|---|---|---|
| R-2 | HIGH | Cross-kind promotion atomicity: `nt_catalog` + `normalized` from one run can desync mid-promote; a backtest mid-promotion reads inconsistent view | Grok, Gemini | One commit spans all kinds for a run; readers pin a committed snapshot SET, not per-kind pointers. Resolve open-decision §13.6. |
| R-3 | MED→HIGH | Idempotency claim false: `content_hash` over **parquet bytes** isn't deterministic (compression, row-group sizing, FP serialization) → re-runs make new keys → duplicate objects | GLM NF-1 | Key on **logical** content digest (sorted/normalized canonical rows), not parquet bytes. Update §9 cost model. |
| R-4 | HIGH | Instruments lane contradiction: §2.1 uses NT `write_instruments`; §4.3 says ALL writes go through ConditionalCatalogWriter. `write_instruments`→non-atomic `put` ⇒ inherits F2 TOCTOU | GLM NF-2 | Instruments lane also encode-then-conditional-write; never call NT `write_instruments` for staged/canonical; explicitly exempt instruments from the `event_time_source` guard (not time-series). |
| R-5 | HIGH | NT encoder reuse assumes a public/separable buffer-encoder (`write_batches_to_object_store` before the final `put`). If internal/monolithic, the external writer must fork/vendor | GLM NF-3, Kimi NF-2 | Phase-0 sub-task: verify the encoder boundary is `pub` at `6e059dc`. Fallback: vendor ~50 LOC or encode via `arrow-rs` to a byte-identical NT-readable parquet. |
| R-6 | MED | Conditional-put capability is unprovable from the built `AmazonS3` store; `S3ConditionalPut` config isn't exposed | Kimi NF-4, GLM design#2 | Specify a runtime probe (dummy `Create`+delete on a sentinel key) at writer construction; abort if not supported. Elevate "bucket supports conditional put" from open-decision to **Phase-0 prerequisite**. Cover multipart (>5 MB) If-None-Match. |
| R-7 | MED | Promotion materialization: 267 GiB via stream read/write = massive egress; `object_store` lacks a unified cross-prefix atomic copy | Gemini | Use backend-native server-side copy (S3 `CopyObject` via `object_store::copy`) for promotion; never download+reupload. Note in §9. |
| R-8 | HIGH | `write_mode` migration not atomic: live ledger heuristic (`backfill_coverage_ledger.py:286-293`) treats `write_mode ∉ {local_staging,dry_run}` as accepted — so migrating producers to `local_staging`+`staging_location` would *un-count* them, and live producers still emit `s3_staging` | Codex | §6.2 must ship producers + ledger logic + schema-validation test **atomically**; count only `local_staging`+`staging_location=s3_noncanonical` as S3-staging; reject unknown/missing modes. List as interdependent implementation tasks. |
| R-9 | MED | `:v0-pending` violates `source_proof_version` (positive integer) schema | Codex, GLM | The provisional `…:v0-pending` suffix is a **row-id** segment, NOT the `source_proof_version` field. A pending `SourceProofReport` is version `1`, `status=pending`. Decouple the two in §6.3; require the provisional id resolve to a real pending record. |
| R-10 | HIGH | Orphan acceptance path: `backfill_accept_staged_objects.py --from-s3-keys` (`:151-209`) can accept unmanifested orphan bytes with optional `source_proof_id` + sampled verify — undercuts F15's "orphans can't be trusted" premise | Codex | Distinct `recovered_orphan` state; require a resolvable **accepted** `source_proof_id`, full (not sampled) hash verify, complete provenance; never counted as accepted until reviewed. |
| R-11 | MED | Tier-A version coupling: tier matrix assumes `NautilusDataType` has exactly 9 members; an NT dep bump could silently invalidate it | GLM design#1 | Add a CI guard: assert the pinned-rev `NautilusDataType` member count == 9 (and the prefix set) and pin the projector to `6e059dc`; fail on drift until the matrix is re-verified. |
| R-12 | MED | Python write path = dual path: "optional Python convenience that writes the same format" has no SSM/conditional-write discipline (and violates bolt rule 2 NO DUAL PATHS) | Kimi NF-1, Grok | Make Python **read-only** against the catalog. Notebooks/Research-Analytics read; only the Rust ConditionalCatalogWriter writes. Removes the dual path entirely. |
| R-13 | LOW-MED | Promotion TOCTOU: PromotionPackage built at T1, written at T2; staging object could be re-labeled orphan or deleted between | GLM NF-4 | Re-verify each object's `content_hash` at write time and abort on mismatch; staging-cleanup policy MUST NOT delete objects referenced by any constructed-but-uncommitted package. |
| R-14 | MED | Synthetic vs provider catalog root collision in Phase-0/3 capability proofs | Kimi NF-5 | Enforce a distinct synthetic catalog-root URI; never commingle with provider roots. |
| R-15 | (framing) | Many Codex "PARTIAL"s are because v2 *describes* edits to bindings/evidence-matrix/ledger that aren't *applied* (e.g. `source-bindings:24,37` still `*_or_delivery`; `evidence-matrix:76,86,148` still list downgraded families) | Codex | v3 explicitly separates **design-decided** from **repo-edits-pending**, and lists the latter as atomic implementation tasks with file:line targets. The plan does not itself edit those files (that's implementation), but must own the task list. |
| R-16 | (open, keep) | Tier B funding (`funding_rate_update`) not BacktestNode-replayable; if a strategy needs historical funding for PnL there's no wired path | Gemini, plan §11.1 | Keep as open decision; note the custom-data/actor injection path as the candidate, decided at build time. |

## 4. Adjudication

- **B-1 is REAL and the fix is known and verified-feasible** → v3 must restructure §4.3/§4.4/§5.3 around immutable per-commit NT-native roots + Tier-C-only physically-isolated staging + a validator. This is the single largest v3 change.
- **R-2…R-14 are valid** and fold in as class-level fixes (not patches).
- **R-15** is a framing fix: v2 is a plan; reviewers who judged "is it fixed in the repo" (Codex) vs "is the design right" (others) diverge. v3 keeps the design bar but adds an explicit, atomic implementation-task ledger so nothing is lost.
- **R-16** stays an owner/build-time open decision.
- **Disproven / not adopted:** Gemini's "reader will *crash*" (NT returns `None` gracefully and over-includes — confirmed at `query_intersects_filename`); the net effect is silent wrong-data, which v3 fixes regardless.

## 4b. Build-feasibility verification (main session, self-checked at `6e059dc` / object_store 0.13.2)

Two findings raised feasibility doubts about the writer/promotion design. Both are confirmed **buildable as designed** — verified by reading source, not by trusting the reviews:

- **R-5 (NT encoder reuse) — resolved cleaner than v2 framed it.** `write_batches_to_object_store` (`crates/persistence/src/parquet.rs:170-200`) is `pub`, but the non-atomic `object_store.put` (`:197`, default `PutMode::Overwrite`) is the **last line of the same function** — encode and put are NOT separable, so it cannot be reused with a swapped put. However the encode (`:178-194`) is ~17 lines of **public `parquet`/`arrow-rs` API only** (`ArrowWriter::try_new` + `WriterProperties` SNAPPY default + `max_row_group_row_count(5000)`) — nothing NT-proprietary. v3 writer **replicates** that encode (byte-shape-compatible: SNAPPY, row-group 5000, same schema) and writes via `object_store.put_opts(path, buf, PutOptions{ mode: PutMode::Create, .. })`. No fork, no vendoring of NT internals.
- **R-7 (server-side copy) — confirmed present.** object_store 0.13.2 `ObjectStore` trait exposes `copy` (`lib.rs:1315`), **`copy_if_not_exists` (`:1324`)**, `copy_opts` (`:1111`), and `rename*`. So promotion uses backend-native server-side copy (S3 `CopyObject`) staging→canonical with **zero download/reupload egress**, and `copy_if_not_exists` additionally gives an **atomic create-only** copy at the canonical layer (If-None-Match on destination) — folding R-7 and the canonical create-only guarantee into one op.

Net: the §4.3 writer encodes via public arrow/parquet + `put_opts(Create)` for staging; §4.4 promotion uses `copy_if_not_exists` into the immutable per-commit canonical root. Both verified-feasible.

## 5. What remains owner-gated (unchanged)

Canonical writes stay gated (contract §292-309). v3 does not authorize them. Owner decisions still required: artifact_root URI, minimum instrument-universe completeness bar, one-year-run cost sign-off, IMDS-block mechanism on the build host, HL `node_fills` authority. These are not defects — they are the gate.
