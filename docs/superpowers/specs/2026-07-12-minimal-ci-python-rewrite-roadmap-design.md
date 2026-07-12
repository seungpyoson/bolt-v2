# Minimal CI Python Rewrite Roadmap Design

## Decision and status

The program will reduce automatic CI and merge governance to the smallest system that still proves a named merge or deploy safety property. Its plain-English goal is that CI proves real safety from one owner and ordinary configuration changes do not require Python or Python-test edits. LOC is secondary.

This is a sequencing and governance roadmap, not implementation authority, a mega-PR plan, or permission to delete controls. Each subsystem needs a named issue, reviewed exact file and cutover set, evidence, and an atomic owner transition.

Issue #1016 is first because it proposes protected-base semantic authority for the central verifier. Only Program-A-before-precursor and atomic legacy deletion ordering are already approved. Its temporary exact-number admission lock, temporary Mergify Merge Protections/Freeze ceremony, pre-precursor final ruleset state, promotion/disablement, closed canary, corrected dormant-base and two-context design, and control plane remain proposed and blocked on separate approval, disposable live proof, owner review, and external review.

## Current authority and supported baseline

The authoritative planning base is main `9f3b13f4c6ae937be69cfb9c44fae409d268ef30`. [`docs/ci/1016-program-ledger.md`](../../ci/1016-program-ledger.md) is the current-only program board; older ledger copies are commit-qualified evidence.

Only these inventory totals are independently supported at this baseline:

- all `scripts/*.py`: 113 files and 143,819 lines;
- central #1016 cluster: 26,667 lines;
- provenance pair: 9,746 lines;
- clean-merged pair: 10,707 lines;
- storage pair: 7,827 lines; and
- AI-review pair: 4,742 lines.

The merge-governance estimates of 11,116 primary and 11,811 including readiness, Rust estimate of 14,387, conditional audited-domain sum of 58,525, conditional residual of 58,627 (approximately 58.6k, calculated as `143,819 - 26,667 - 58,525`), and shared transport estimate of roughly 300 are provisional. The sum and residual are both conditional until one exact non-overlapping path manifest reconciles ownership and overlap; they may guide planning but cannot support acceptance or deletion.

## Historical change-amplification evidence

Six distinct behavioral PRs were examined: #1151, #1186, #1297, #1290, #1173, and #1309. The five non-relocation PRs total 4,461 raw textual changed lines across 26 unique paths. Relocation PR #1309 is separate: 33,131 raw textual changed lines over 22 paths because no reproducible checked-in move map exists. The union across all six is 44 paths.

The nominal aggregate is #1151 + #1186 + #1297: 46 direct operational-value churn lines, 309 raw changed lines, 19 summed path touches, and 16 unique paths. Additions plus deletions are counted, so these are not semantic-fact counts. Those PRs are not counted again in another grand total.

PR #1290 changed 2,094 raw textual lines over 7 paths; its verifier/test subtotal is 566. PR #1173 changed 2,058 over 12 paths; its verifier/test subtotal is 1,264, or 61.42%. Positive comparators remain separate: #1146 changed 10 lines in one file and #1292 changed 27 lines over two files.

Exact SHAs, commands, raw-count rules, and supported subtotals are recorded in [`docs/ci/1016-receipts/9f3b13f/change-amplification-baseline.md`](../../ci/1016-receipts/9f3b13f/change-amplification-baseline.md). Historical churn is diagnostic evidence, not an acceptance gate.

## Safety burden and dispositions

Every existing control is retained unchanged unless evidence supports a reviewed disposition. Missing evidence never authorizes deletion, de-automation, weakening, or re-homing.

`evidence_pending` means unchanged temporary retention with an owner, issue, missing-proof statement, and review deadline. Its expiry preserves the control and keeps the decision blocked. If the control would enter a frozen corpus, unresolved evidence blocks that cutover. Missing proof rejects a proposed new required control.

Before any domain freeze, its exact-SHA disposition inventory compares risk, semantic and native owners, independent mutation or exact-identity evidence, tier, frequency, cost, and proposed disposition. Only independently proven retained, re-homed, or native rules enter that domain's corpus. Unresolved rows stay outside and block the applicable cutover.

## Change classes and accounting

The hard semantic requirements are:

| Class | Required shape |
| --- | --- |
| A — ordinary value | One lifecycle authority; no manual mirror or Python/test/manual-digest edit |
| B — existing generic semantics | No new branch, regex, copied fixture, or test method |
| C — new semantic invariant | One semantic owner plus independent clean and falsifying proof |
| D — topology | One topology authority; no mirrored literal, copied workflow, or manual digest |
| E — advisory/operator/reporting | Cannot authorize a required merge or deploy |

Classification precedence is E when non-authoritative; otherwise D > C > B > A. Pure moves, refactors, documentation, and provenance renewal receive no A–E amplification ratio.

One receipt covers one semantic intent, lifecycle owner, and rollback boundary. Every hunk belongs to one receipt; shared infrastructure is its own receipt. Representation amplification is manually maintained fact-to-representation edges touched divided by canonical semantic facts changed. Candidate-added helper fields do not inflate the denominator. Raw files/lines and derived outputs are reported separately.

Each symbol/span record has functional role (`authority`, `implementation`, `evidence`, `provenance`, `docs`) and representation origin (`unique`, `derived`, `duplicated`). Independent evidence has a separate oracle owner and kills a mutation; calling a representation a test or digest does not make it independent.

All ordinary numerical line, file, ratio, percentage, and rolling-median figures are provisional review signals until a reproducible calibration set exists. They are not gates, acceptance requirements, or automatic exception machinery. The explicit exception is the owner-selected 13,333-line #1016 ceiling: it is secondary one-time cutover governance, not empirical proof, a runtime fence, or an ordinary-PR fence.

The ceiling uses the atomic design's protected-base full-path/all-language attribution contract: launcher seams, runner/protocol, facade, parsers, registry, adapters, bootstrap, measurement, focused tests, and generated executable code are counted, while validated non-executable corpus and manifest exclusions are reported separately. Corpus and manifest data may contain governed inputs and expected evidence, never executable rule selection, applicability, or verdict logic; only typed materializer/runner evidence paths consume them, enforced by import/dataflow fences and hidden-policy mutations. Hidden policy in data, generated output, retained owners, or another language is counted and rejected.

## One authority for #1016 memberships

#1016 unifies only its central-verifier suite registration, focused cheap-lane membership currently consumed through two TOML locations, and central-verifier workflow fingerprint inputs. One lifecycle-owned declarative registry is the human authority.

Consumers read it directly where possible. Any unavoidable standalone artifact is deterministic, explicitly non-authoritative, and provenance-bound to generator, input, command, and digest. Zero-diff regeneration proves derivation. Tests validate schema and derivation instead of copying exact sets.

Maintenance proof must show a member change as one human authority edit with no Python, test, or manual-digest edits, while reporting deterministic derived-byte churn separately. This roadmap does not claim a global registry for unrelated Rust-verification membership; that would require its own issue-owned topology change.

## Deterministic planning inventory

A future separately reviewed docs/governance slice must generate an exact-SHA inventory that covers every tracked `scripts/*.py` path exactly once. For each path it records blob ID, digest, bytes, lines, domain, semantic owner, file-set owner, operational tiers, execution modes, control IDs, disposition, issue, dependencies, proofs, and representation-axis symbol/span records.

Generation must fail on missing, duplicate, or overlapping path ownership and publish command, input SHA, output digest, and reconciliation totals. It runs at program phase boundaries. It is not an always-on runtime CI fence or an ordinary-PR maintenance obligation.

## Machine-readable subsystem DAG

Before implementation, the program requires a reviewed machine-readable DAG. Each node records issue, exact file set, semantic and file-set owners, risk, tier, inputs, entrypoints, outputs, checks, dependencies, blockers, cutover and deletion sets, evidence, freeze surface, and a design-specific maximum review surface.

Only `must_land_before` edges must be acyclic. Shared-file serialization and design-parallel relationships are separately typed so they cannot be mistaken for implementation authority. A node without a named issue and reviewed exact file/cutover set is `BLOCKED` or `UNASSIGNED`; unresolved placeholders are not used.

## Portfolio and sequencing

| Program node | Current evidence | State and authority condition |
| --- | ---: | --- |
| #1016 central verifier | 26,667 supported lines | `BLOCKED`; follow dormant precursor, promotion/disablement, final authority establishment, closed canary/freeze, and atomic activation |
| Shared GitHub transport | roughly 300 provisional lines | `UNASSIGNED`; one transport-only caller-migration slice after exact inventory |
| CI provenance | 9,746 supported lines | `UNASSIGNED`; needs named issue and reviewed exact cutover set |
| Merge governance | 11,116/11,811 provisional | `UNASSIGNED`; needs reconciled manifest and named issue |
| Rust verification | 14,387 provisional | `UNASSIGNED`; needs exact manifest and named issue |
| Clean merged artifacts | 10,707 supported lines | `UNASSIGNED`; needs named issue and reviewed exact cutover set |
| Storage audit | 7,827 supported lines | `UNASSIGNED`; needs named issue and reviewed exact cutover set |
| AI review | 4,742 supported lines | `UNASSIGNED`; needs named issue and reviewed exact cutover set |

The shared GitHub transport node is one explicitly named caller-migration slice: it atomically migrates all duplicated transport callers and deletes duplicate clients without moving domain verdicts. It may cross caller files because that is its declared slice, but it cannot become a broad domain rewrite.

No later domain has implementation authority until its node has a named issue, exact file/cutover/deletion set, dependency resolution, and review. Design work may proceed only where the ledger and DAG explicitly allow it.

## Per-subsystem sequence

1. Regenerate the exact inventory, callers, controls, timings, RSS, cost, and representation records from authoritative main.
2. Adjudicate every control; unresolved evidence retains existing behavior and blocks the applicable corpus or cutover.
3. Establish a named issue, exact file set, owners, DAG node, and reviewed design.
4. Choose the smallest risk-proportionate atomic transition for that issue. Existing trusted or native owners judge a one-head implementation-and-deletion cutover unless the change could authorize its own merge or deploy result.
5. For #1016, reserve the exact precursor PR number and first land one separately reviewed temporary `.mergify.yml` admission lock under current legacy authority. It has exactly one queue matching that number, batch size one, one parallel check, branch-protection injection disabled, four explicit legacy checks, mandatory native review, and no alternate route.
   Before precursor merge, disposable proof covers `exempt` injection, exact-number admission, injection-disabled behavior, hidden routes, self-change reset, preexisting proof invalidation, mixed batches, merge-time Freeze re-evaluation, exclusions, dequeue/no-running-batch state, wrong publisher, native/direct blocking, Freeze under exempt, identity, latency, and API/quorum/audit. Operators establish an indefinite Freeze, atomically put ruleset 14763242 in final gate-only-replacement state, change Mergify 10562 from `always` to `exempt`, reserve the activation PR, add its inert second exclusion, and terminally re-query all state. The precursor stages the dormant replacement and manifest and atomically replaces the admission lock with final hotfix/default mappings. The temporary lock, legacy checks, and native review judge it; trusted emits nothing. After merge, no enforcement mutation occurs: promotion, tombstone, and an internal, non-publishing closed canary run against final state. The canary may compute allow/deny, but it cannot create or satisfy a merge-visible context and none of its records or artifacts can become activation authority. Success permits the reserved activation to queue alone as the literal first subsequent covered change; trusted remains absent until terminal tuple validation succeeds for its exact proof head. After exact protected-main proof, Freeze and temporary Merge Protections reporting/binding are removed; final trusted ruleset authority and Mergify `exempt` remain. Failure after precursor has no recovery PR: Freeze remains and recovery requires a new program plus explicit owner/external operational decision. This remains pending approval and is not a universal ceremony for later domains.
6. For advisory or operator-only tools, use the smallest direct reviewed change; they never inherit merge-authority ceremony merely because #1016 needed it.
7. Record exact-head evidence, review state, merge SHA, and before/after receipts in the current ledger.

Every transition remains one named issue, exact file set, and DAG node; there is no same-event old/new comparison, head-selected corpus, compatibility adapter, cleanup PR, or cross-domain mega PR.

For sequencing terminology, Program-A issue-owned deletions already landed before the precursor. The precursor fixes the exact legacy deletion manifest, but the legacy central-verifier implementation is actually deleted only in the atomic activation.

## Acceptance and rejection

Program progress requires one semantic owner per retained rule, lifecycle configuration as the only scalar authority, minimal independent mutations, provenance-bound derivation, exact caller closure, fail-closed required authority, and atomic deletion of superseded owners.

The following are not progress: moving or splitting code without reducing ownership, generated hidden policy, copied workflows/configuration, manual membership or digest mirrors, advisory contexts satisfying merge authority, deletion based on missing evidence, compatibility layers, permanent measurement fences, deletion exemptions, or LOC reduction with unchanged ordinary-change touch surface.

## Current blockers

Program B is blocked on issue-body/atomic-ruling reconciliation; owner/external approval of the proposed hinge and corrected dormant-base/two-context design; separate App/control-plane and temporary Merge Protections/Freeze authorization and budget; disposable live proof of exact-number admission, injection and `exempt` behavior, hidden routes, identity, exclusions, self-change reset, proof invalidation, mixed batches, merge-time re-evaluation, dequeue/no-running-batch state, native/direct blocking, latency, and API/quorum/audit; exact-SHA regeneration; the admission-lock PR; pre-precursor terminal final state; the single precursor merge under legacy checks and native review; promotion and irreversible bootstrap disablement without later enforcement mutation; successful closed canary and exact freeze evidence; and atomic cutover as the first subsequent covered Git/enforcement-surface change. Before precursor merge, abort uses the separately reviewed operator procedure. After precursor merge or canary/activation failure, Freeze stays active and there is no recovery PR; proceeding requires explicit acceptance that the repository may remain unable to merge and any recovery is a new separately authorized program. Later subsystem rows are unauthorized planning nodes, not queued implementations.

## Spec self-review status

This correction pass checked that current state is separated from historical evidence; unsupported aggregates are marked provisional; missing evidence preserves controls; membership authority is bounded to #1016; and later domains remain unauthorized. It does not claim that inventory, DAG, control-plane design, owner approval, external review, precursor evidence, freeze evidence, or implementation readiness exists.
