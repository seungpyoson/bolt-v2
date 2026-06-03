Verdict: SHIP-WITH-CHANGES

The design is structurally sound. All 15 prior findings are resolved at the class level (not just patched). Four new findings surface below; none block the design, but one (NF-2) must be settled before Phase 0 code, and one (NF-4) is a latent correctness gap that could bite under real concurrency.

---

## Per-Finding Resolution Table

| Finding | Verdict | Evidence |
|---|---|---|
| F1 | RESOLVED | §2 three-tier matrix + `nt_target` column makes mis-assignment structurally impossible. 9-member `NautilusDataType` exhaustively cited (`config.rs:52-62`). `FundingRateUpdate` confirmed absent from `Data` enum (`mod.rs:100-112`). |
| F2 | RESOLVED | §4.3 `ConditionalCatalogWriter` replaces NT's head-then-put with `PutMode::Create` (If-None-Match: *). Content+transform-hash keying eliminates interval collision. Phase-0 BLOCKER concurrency proof gates all writes. |
| F3 | RESOLVED | §6.1 four-family derivation from `contractType` at normalize-time; binding TOML `acquisition_group` rename removes `*_or_delivery` spelling; fail-loud guard on absent contractType; acceptance test ships with Phase 5. |
| F4 | RESOLVED | §7.1 OKX 400-level → `order_book_snapshots_fixed_depth` + named-derivation deltas; forbidden_claim enforced by S7 gate (proof keyed[redacted_source_excerpt] stays `pending`). |
| F5 | RESOLVED |[redacted_source_excerpt]family from[redacted_source_excerpt]corrected to[redacted_source_excerpt]TOML and[redacted_source_excerpt]econciled; unproven families moved[redacted_source_excerpt]. |
| F6 | RESOLVED | §7.3[redacted_source_excerpt]applied as a class fix to ALL[redacted_source_excerpt] (Deribit `index`/[redacted_source_excerpt]/[redacted_source_excerpt]L `meta`/contexts). Fail-loud, not skip. |
| F7 | RESOLVED | §6.2 one 3[redacted_source_excerpt][redacted_source_excerpt] aliased to[redacted_source_excerpt]exhaustive grep-verified[redacted_source_excerpt] |
| F8 | RESOLVED |[redacted_source_excerpt]`sp:<ver>:<venue>/<pf>/<tf>:v0-pending` scheme — [redacted_source_excerpt]contract triple[redacted_source_excerpt]bound to[redacted_source_excerpt]only. |
| F9 | RESOLVED |[redacted_source_excerpt][redacted_source_excerpt]/[redacted_source_excerpt]/[redacted_source_excerpt]; d[redacted_source_excerpt][redacted_source_excerpt]); unresolved →[redacted_source_excerpt](never dropped);[redacted_source_excerpt] |
| F10 | RESOLVED |[redacted_source_excerpt]; O(D²) LIST amplification identified and mitigated via[redacted_source_excerpt]` +[redacted_source_excerpt][redacted_source_excerpt][redacted_source_excerpt]d (NT drops[redacted_source_excerpt]); cost gates one-year run. |
| F11 | RESOLVED | §11-cred two-part control: feature gate + SSM-attribution delta (scrub env+IMDS; fail without SSM, succeed with SSM). IMDS-[redacted_source_excerpt]honestly flagged as o[redacted_source_excerpt]. |
| F12 | RESOLVED |[redacted_source_excerpt] Correctly identifies that[redacted_source_excerpt]makes in-graph isolation impossible. |
| F13 | RESOLVED | §7.4 explicit[redacted_source_excerpt][redacted_source_excerpt];[redacted_source_excerpt]correc[redacted_source_excerpt]_by_block` gated as separate future task. |
| F14 | RESOLVED |[redacted_source_excerpt]oof-[redacted_source_excerpt]+[redacted_source_excerpt] +[redacted_source_excerpt]. §10 reorders proof (S7) before projection (S8) and before backtest (S9). No[redacted_source_excerpt]byte can reach BacktestNode in the automated pipeline. |
| F15 | RESOLVED | §4.4[redacted_source_excerpt](no prefix/glob enumeration) +[redacted_source_excerpt] for[redacted_source_excerpt]+ CAS pointer commit. Staging objects never mutated; commit_state flip only in[redacted_source_excerpt]. |

---

## New Findings

### NF-1 — Encoder determinism breaks idempotency claim (MEDIUM)

**Where:** §4.3 point 3: "[redacted_source_excerpt]"

**What:** The[redacted_source_excerpt]is SHA-256 of the *parquet bytes*.[redacted_source_excerpt]([redacted_source_excerpt]does not guarante[redacted_source_excerpt]output: row-group boundaries depend on batch sizing, parquet compression (e.g., Snappy/Gzip dictionary) may vary across invocations, and floating-point serialization can differ by platform. Two runs of the same logical data over the same transform can produce *different*[redacted_source_excerpt]values → different keys → both succeed on `Create` → duplicat[redacted_source_excerpt]s at distinct physical paths. Not a correctness bug (both are valid), but the "idempotent" claim is false, and storage cost projections in §9 underestimate object counts for re-runs.

**Fix:** Either (a) make[redacted_source_excerpt]cover logical content (e.g., hash sorted column values, not parquet bytes), or (b) acknowledge the key is *approximately* idempotent and add a cleanup/dedup step for stale variants under the same `[redacted_source_excerpt], interval, transform_hash)`. Add this to §9 cost model.

### NF-2 — Instruments write path ambiguity vs. [redacted_source_excerpt] mandate (HIGH)

**Where:** §2.1 ("[redacted_source_excerpt]") vs. §4.3 ("[redacted_source_excerpt]")

**What:** §2.1 says instruments use NT's[redacted_source_excerpt]lane. §4.3 says ALL[redacted_source_excerpt][redacted_source_excerpt]. These are contradictory unless the plan means: encode instruments via NT's encoder, then write via [redacted_source_excerpt] ([redacted_source_excerpt]instruments` function, which internally calls the non-atomic `put`). The plan doesn't explicitly state this. If instruments go through NT's nativ[redacted_source_excerpt], they inherit the TOCTOU race from F2.

**Fix:** Add one sentence to §4.3: "The instruments lane encodes via NT's instrument serializer bu[redacted_source_excerpt][redacted_source_excerpt] using[redacted_source_excerpt]fix and[redacted_source_excerpt], exactly like all other data objects. NT's[redacted_source_excerpt]function is never called for staged or[redacted_source_excerpt]." Also add `instruments` to[redacted_source_excerpt]guard — [redacted_source_excerpt] carry no event_time but are not time-series, so the guard should explicitly exempt them.

### NF-3 — NT encoder path may not be a public API (MEDIUM)

**Where:** §4.3 point 1: "[redacted_source_excerpt][redacted_source_excerpt]"

**What:** The plan assumes the [redacted_source_excerpt] (in a[redacted_source_excerpt]) can call into NT's encoder path. If [redacted_source_excerpt] or the buffer-to-parquet encoding is not a public export of th[redacted_source_excerpt] crate, the external writer cannot access it without either forking NT or vendoring the encoder code. The plan doesn't address this.

**Fix:** Phase 0 sub-task: verify [redacted_source_excerpt][redacted_source_excerpt]buffer-encoder) is `pub` in the pinned NT rev. If not, add a concrete plan (vendor the ~50 LOC encoder, or contribute a public API upstream, or implement independent parquet encoding that matches NT's read path).

### NF-4 — Promotion TOCTOU: orphan transition between p[redacted_source_excerpt]and[redacted_source_excerpt]LOW-MEDIUM)

**Where:** §4.4 points 1-4

**What:** The[redacted_source_excerpt] is constructed at time T₁ (rejecting orphans/superseded), but the[redacted_source_excerpt]happens at T₂ > T₁. A concurrent staging cleanup or supersede operation could mark an[redacted_source_excerpt]as `orphan` between T₁ and T₂. The package references exact staging URIs + content_hashes. If the staging object is *deleted* between T₁ and T₂, th[redacted_source_excerpt] fails loudly (can't read source). If the staging object is merely *re-labeled* orphan but its bytes are intact, th[redacted_source_excerpt] succeeds — the object's content is valid but its staging metadata says orphan.

Practical risk is low because the content_hash verification catches byte corruption, and staging cleanup shouldn't delet[redacted_source_excerpt]objects. But the plan's claim that "r[redacted_source_excerpt]" is a point-in-time check, not a transactional guarantee.

**Fix:** Add to §4.4: "[redacted_source_excerpt]records the content_hash of ever[redacted_source_excerpt]. Th[redacted_source_excerpt] step re-reads each staging object and verifies SHA-256 before writing. If any hash mismatches (indicating mutation or deletion), the entire promotion aborts. The staging cleanup policy MUST NOT delete objects referenced by any constructed-but-uncommitted[redacted_source_excerpt]."

---

## Design-Level Concerns

### 1. The t[redacted_source_excerpt]is correct *today* but creates a version-coupling landmine

The Tier A/B/C split depends o[redacted_source_excerpt] having exactly 9 members and[redacted_source_excerpt]being the only replay path. If NT adds[redacted_source_excerpt]to[redacted_source_excerpt]in a future rev, Tier B collapses into Tier A — which is *good*, but every[redacted_source_excerpt]`nt_target` label,[redacted_source_excerpt]about non-replayability, and Phase-6 funding-consumption workaround becomes stale. The plan should add a version-gating assertion: the projector's `Cargo.toml` pin[redacted_source_excerpt]to rev `6e059dc` and a CI[redacted_source_excerpt][redacted_source_excerpt] count == 9 (fail on upgrade until the tier matrix is re-verified). Without this, an NT dep-update silently invalidates the plan.

### 2. The [redacted_source_excerpt] is the plan's load-bearing wall, and it's entirely unproven

Every write discipline, every concurrency guarantee, and every idempotency claim flows through §4.3. The plan correctly gates it[redacted_source_excerpt], but the design has three dependencies that haven't been verified:[redacted_source_excerpt][redacted_source_excerpt]behavior under real S3 multipart uploads for files >5MB (the plan discusses single PUTs but not multipart + If-None-Match on `CompleteMultipartUpload`).
- The production bucket'[redacted_source_excerpt]` configuration — if `Disabled`[redacted_source_excerpt][redacted_source_excerpt][redacted_source_excerpt]. The plan lists this as a "r[redacted_source_excerpt]" (§13) but it should be a Phase-0 prerequisite, not a lingering question.
- T[redacted_source_excerpt] (§4.3 point 4) must use a store with identical[redacted_source_excerpt]semantics to production S3.[redacted_source_excerpt]`Create`[redacted_source_excerpt]. The plan mentions MinIO/R2 but doesn't specify how the proof store is provisioned or validated.

**Recommendation:** Elevate the[redacted_source_excerpt] check from "r[redacted_source_excerpt]" to Phase-0 prerequisite. Add a Phase-0 sub-task: verify[redacted_source_excerpt]works end-to-end on the actual production bucket (or a byte-identical staging bucket) before any other work.

### 3. The[redacted_source_excerpt]gate has a manual-trust surface that isn't bounded

§5.2 enforces that[redacted_source_excerpt].[redacted_source_excerpt]gates all [redacted_source_excerpt]. B[redacted_source_excerpt]n't specify *who*[redacted_source_excerpt]or *what* automated checks are sufficient. §13 lists "[redacted_source_excerpt]" as a r[redacted_source_excerpt]. This is a trust bottleneck: if acceptance is manual, one incorrect accept Poison the entire pipeline. If acceptance is automated, the[redacted_source_excerpt]set must be exhaustive — and the plan defines checks[redacted_source_excerpt]50-73` but doesn't audit whether those checks are sufficient to prevent, e.g., a schema-drift injection.

**Recommendation:** Before Phase 4, specify (a)[redacted_source_excerpt]authority (human sign-off vs. automated gate), (b) a tamper-evidence mechanism for[redacted_source_excerpt]records (e.g., the proof's SHA-256[redacted_source_excerpt]the[redacted_source_excerpt]of its own write), and (c) a revocation path if an[redacted_source_excerpt]later found invalid.

---

## Scope Gap

Only `specs/023-nt-[redacted_source_excerpt]-platform/reference/[redacted_source_excerpt].md` was in scope and reviewed. The referenced contract/schema/[redacted_source_excerpt]TOML files, venue scripts, and NT crate sources were used as context per the review instructions but were NOT independently audited for correctness of the plan's citations. The plan's correctness depends on those citations being accurate (e.g.[redacted_source_excerpt]09-4146` really does list[redacted_source_excerpt]not[redacted_source_excerpt];[redacted_source_excerpt]really has exactly 9 members at the pinned rev). A falsifying error in any citation would downgrade the corresponding finding to PARTIAL.