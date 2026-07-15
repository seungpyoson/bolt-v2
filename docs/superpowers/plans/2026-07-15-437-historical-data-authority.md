# Historical Data Acquisition Authority Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the owner-approved historical-data acquisition decision the sole live authority without changing runtime behavior.

**Architecture:** Add one focused decision under the existing cross-project reference layer, remove the superseded v3 plan, and reconcile only clauses that would let a mutable Artifact Index pointer or raw S3 prefix select backtest input. Preserve the general Artifact Index for result/derived-artifact discovery and preserve immutable historical evidence as provenance.

**Tech Stack:** Markdown, TOML, repository text/static verification, `just source-fence-static`.

## Global Constraints

- Scope is the named #437 historical-data authority slice; no Rust behavior, AWS resource, issue, or workflow mutation.
- `historical-data-acquisition-architecture.v1.md` is the only live historical acquisition/catalog-binding authority.
- Runtime backtest input is explicit manifest URI + digest; no mutable latest pointer and no raw S3 catalog path.
- General Artifact Index behavior for results and derived artifacts remains outside this slice.
- Cached objects are re-hashed exactly once per run at first use.
- Every future Binance coverage cell is `excluded_by_policy` with exact reason `owner choice: Binance is intentionally excluded despite breadth loss`.
- Immutable proof/run artifacts and the historical converter investigation are not rewritten.
- Documentation/policy completion requires targeted static checks and internal adversarial review.
- No local compile-heavy Rust command is permitted; use repository-approved non-compile checks.

---

### Task 1: Establish The Replacement Authority

**Files:**

- Create: `specs/023-nt-research-analytics-platform/reference/historical-data-acquisition-architecture.v1.md`
- Modify: `specs/023-nt-research-analytics-platform/reference/README.md`
- Modify: `specs/023-nt-research-analytics-platform/reference/normalization-catalog-plan.v3-phase0-primitive-proof.md`
- Delete: `specs/023-nt-research-analytics-platform/reference/normalization-catalog-plan.v3.md`

**Interfaces:**

- Consumes: approved decision and reviews at repository head `37e619b3fbd65fc041a05399ecf1750b8999567a`, NT pin `d636f17604cdbddc28ad40e0e15720e2d19bf860`.
- Produces: one precedence rule inherited by every numbered project.

- [ ] **Step 1: Verify the decision contains the two final review amendments**

Run:

```bash
rg -nF 'Cached objects are re-hashed exactly once per run at first use before reuse.' specs/023-nt-research-analytics-platform/reference/historical-data-acquisition-architecture.v1.md
rg -nF 'owner choice: Binance is intentionally excluded despite breadth loss' specs/023-nt-research-analytics-platform/reference/historical-data-acquisition-architecture.v1.md
```

Expected: one binding requirement for cache verification and one exact owner-policy reason.

- [ ] **Step 2: Register the new authority in the reference README**

Add this bullet under `Authoritative Inputs`:

```markdown
- `historical-data-acquisition-architecture.v1.md`: owner-approved source selection, immutable publication/read binding, NT replay prerequisites, data-family ownership, and AWS cost boundary for historical backtests. It supersedes `normalization-catalog-plan.v3.md`.
```

Add this precedence sentence after the existing derived-view paragraph:

```markdown
For historical acquisition and backtest-input identity, `historical-data-acquisition-architecture.v1.md` overrides older project text and immutable proof artifacts. General result and derived-artifact discovery remains governed by `contracts.md`.
```

- [ ] **Step 3: Remove the superseded v3 file**

Delete `reference/normalization-catalog-plan.v3.md`. Do not move or copy it elsewhere; git history is the archive.

Add a banner to `normalization-catalog-plan.v3-phase0-primitive-proof.md` that
labels it historical evidence, points to the replacement authority, and leaves
the recorded proof transcript unchanged.

- [ ] **Step 4: Verify authority uniqueness**

Run:

```bash
test ! -e specs/023-nt-research-analytics-platform/reference/normalization-catalog-plan.v3.md
rg -n 'historical-data-acquisition-architecture\.v1|normalization-catalog-plan\.v3' specs/023-nt-research-analytics-platform/reference/README.md
```

Expected: the old file is absent; the README names the new authority and its supersession once.

### Task 2: Remove Live V3 References And Record Binance Policy

**Files:**

- Modify: `specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md`
- Modify: `crates/backtesting-vertical-slice/tests/fixtures/s3_catalog_smoke.toml`
- Modify: `specs/023-nt-research-analytics-platform/reference/backfill-evidence-matrix.v1.toml`
- Modify: `specs/023-nt-research-analytics-platform/reference/backfill-table-contract.md`

**Interfaces:**

- Consumes: Task 1 authority path.
- Produces: no live citation to the removed plan and no interpretation of legacy Binance evidence as acquisition permission.

- [ ] **Step 1: Repoint the RA durability citation**

In `RA-001`, replace the v3 reference with:

```markdown
per ../reference/historical-data-acquisition-architecture.v1.md; publication and manifest-bound read behavior remain separate issue-owned implementation slices
```

Also replace `Read access stays NT (from_uri S3)` with:

```markdown
NT remains the reader, but production backtest input must come from the manifest-verified sealed local catalog view; raw `from_uri` S3 input is not accepted authority.
```

- [ ] **Step 2: Repoint the MinIO conformance fixture**

Set:

```toml
requirement_ref = "historical-data-acquisition-architecture.v1.md §Immutable Publication Protocol"
```

- [ ] **Step 3: Mark the evidence matrix non-authoritative for selection**

Add this comment before `contract_version`:

```toml
# Historical source-availability evidence only. This file does not authorize
# acquisition or replay. The owner-approved Binance policy is
# excluded_by_policy for every product/table-family cell with reason
# "owner choice: Binance is intentionally excluded despite breadth loss".
# The exhaustive replacement registry is an issue-owned #437 implementation
# slice governed by historical-data-acquisition-architecture.v1.md.
```

Do not rewrite the historical availability rows in this authority-only slice.

- [ ] **Step 4: Demote the legacy contract's acquisition clauses**

Keep `backfill-table-contract.md` authoritative for table/schema vocabulary,
but mark its evidence-state, product-family, matrix, and venue-note sections as
historical and non-authorizing for source selection. State that the new
architecture controls coverage and that all Binance cells remain
`excluded_by_policy` with the owner-approved reason until the exhaustive
registry lands. Register the same precedence boundary in `reference/README.md`.

- [ ] **Step 5: Verify no live v3 citation or second coverage authority remains**

Run:

```bash
! rg -nF 'normalization-catalog-plan.v3' specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md crates/backtesting-vertical-slice/tests/fixtures/s3_catalog_smoke.toml
rg -nF 'owner choice: Binance is intentionally excluded despite breadth loss' specs/023-nt-research-analytics-platform/reference/backfill-evidence-matrix.v1.toml
rg -nF 'historical, non-authorizing investigation context' specs/023-nt-research-analytics-platform/reference/backfill-table-contract.md
rg -nF 'specifically overrides the source-selection' specs/023-nt-research-analytics-platform/reference/README.md
```

Expected: no stale reference and one explicit policy warning.

### Task 3: Bind Historical Inputs Without Replacing The General Artifact Index

**Files:**

- Modify: `specs/023-nt-research-analytics-platform/reference/contracts.md`
- Modify: `specs/023-nt-research-analytics-platform/reference/evidence.md`
- Modify: `specs/023-nt-research-analytics-platform/1-backtesting-engine/spec.md`
- Modify: `specs/023-nt-research-analytics-platform/1-backtesting-engine/plan.md`

**Interfaces:**

- Consumes: Task 1 explicit manifest identity.
- Produces: a narrow exception that prevents mutable discovery metadata from becoming backtest-input authority.

- [ ] **Step 1: Add the contract exception immediately after the Artifact Index rules**

Insert this subsection before `Result And Promotion Boundary`:

```markdown
### Historical Dataset Input Boundary

Artifact Index pointers and snapshots may advertise available datasets, but they never select bytes for a backtest. A run pins explicit dataset-manifest URI and SHA-256 values. The manifest binds normalized paths, byte lengths, content hashes, and S3 version IDs. Bolt verifies the selected objects and composes a sealed local catalog view; NT reads only that view. Raw S3 catalog paths, independently joined latest pointers, and validate-then-relist behavior are invalid production inputs. General Artifact Index discovery for results and derived artifacts is unchanged.
```

- [ ] **Step 2: Reconcile evidence E-038 without changing E-039 producer ownership**

Add this qualification immediately after the evidence table:

```markdown
### Historical Dataset-Input Qualification For E-038

For historical dataset input, discovery metadata is non-authoritative. Each run pins an explicit manifest URI and SHA-256, verifies the exact versioned object set, and consumes those bytes through a sealed local catalog view that NT reads. The generated latest pointer remains available for general artifact discovery, but it cannot select, substitute, or advance a backtest's input.
```

- [ ] **Step 3: Add the same boundary to the BTE spec and plan**

Add under `Artifact Index Policy` in the spec and after the Artifact Index paragraph in the plan:

```markdown
Historical backtest input is stricter than bulk discovery. A run selects explicit dataset-manifest URI and digest values, verifies the exact versioned objects, and passes NT a sealed local catalog view. The per-kind latest pointer cannot select, substitute, or advance run input, and a raw S3 catalog URI is not a production binding path.
```

- [ ] **Step 4: Verify the carveout is present and narrow**

Run:

```bash
rg -nF 'Historical Dataset Input Boundary' specs/023-nt-research-analytics-platform/reference/contracts.md
rg -nF 'sealed local catalog view' specs/023-nt-research-analytics-platform/reference/evidence.md specs/023-nt-research-analytics-platform/1-backtesting-engine/spec.md specs/023-nt-research-analytics-platform/1-backtesting-engine/plan.md
rg -nF 'artifact-index/v1/pointers/kind=<artifact_kind>/latest.json' specs/023-nt-research-analytics-platform/reference/contracts.md
```

Expected: explicit historical input binding exists while the general pointer contract still exists for other discovery.

### Task 4: Self-Review And Evidence

**Files:**

- Review every changed file from Tasks 1-3.

**Interfaces:**

- Consumes: completed authority reconciliation.
- Produces: reviewable documentation/policy evidence; no runtime claim.

- [ ] **Step 1: Scan for placeholders, ambiguity, and stale authority**

Run:

```bash
! rg -n '\bTO[D]O\b|\bTB[D]\b|fix[ ]later' specs/023-nt-research-analytics-platform/reference/historical-data-acquisition-architecture.v1.md docs/superpowers/plans/2026-07-15-437-historical-data-authority.md
! rg -nF 'normalization-catalog-plan.v3.md' specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md crates/backtesting-vertical-slice/tests/fixtures/s3_catalog_smoke.toml
git diff --check
```

Expected: no placeholders, no live v3 citation, and no whitespace errors.

- [ ] **Step 2: Run repository-approved static evidence**

Run:

```bash
just source-fence-static
```

Expected: exit 0 with every governed fence and self-test reporting `OK`.

- [ ] **Step 3: Run an internal adversarial documentation review**

The review prompt must ask whether the diff creates a second authority, overreaches into the general Artifact Index, misstates cache/Binance decisions, assigns work outside live issue scopes, rewrites immutable evidence, or claims unimplemented runtime behavior. Resolve every substantive finding before publication.

- [ ] **Step 4: Commit the authority slice**

Run:

```bash
git add docs/superpowers/plans/2026-07-15-437-historical-data-authority.md specs/023-nt-research-analytics-platform/reference/historical-data-acquisition-architecture.v1.md specs/023-nt-research-analytics-platform/reference/README.md specs/023-nt-research-analytics-platform/reference/normalization-catalog-plan.v3.md specs/023-nt-research-analytics-platform/reference/normalization-catalog-plan.v3-phase0-primitive-proof.md specs/023-nt-research-analytics-platform/reference/backfill-evidence-matrix.v1.toml specs/023-nt-research-analytics-platform/reference/backfill-table-contract.md specs/023-nt-research-analytics-platform/reference/contracts.md specs/023-nt-research-analytics-platform/reference/evidence.md specs/023-nt-research-analytics-platform/1-backtesting-engine/spec.md specs/023-nt-research-analytics-platform/1-backtesting-engine/plan.md specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md crates/backtesting-vertical-slice/tests/fixtures/s3_catalog_smoke.toml
git commit -m "docs: adopt historical data acquisition authority"
```

Expected: one docs/policy commit scoped to the named #437 slice.

## Execution Handoff

After this authority slice is reviewed and merged, start the named #437 immutable-publication slice from fresh authoritative `main`, then the separate manifest-read tracer. #563 remains conversion-state work only. Do not implement the publication tracer, read tracer, NT fork, coverage registry, or non-NT family contract in this branch.
