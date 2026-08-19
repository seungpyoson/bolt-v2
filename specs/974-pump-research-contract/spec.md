# Feature Specification: Manipulated Pump Research Contract

**Feature Branch**: `974-pump-research-contract`
**Created**: 2026-08-20
**Status**: Draft
**Input**: User-approved v6 contract for reproducible, hard-evidence crypto pump-and-reversal research with zero current incremental spend, bounded storage, no selected granular-data provider, and externally reviewed safeguards against hindsight, leakage, unsupported manipulation claims, and dual research truth.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Discover Reproducible Episodes (Priority: P1)

As a researcher, I can define a bounded market frame, prove what instruments and time periods were observable, and detect retrospective pump-and-reversal episodes without claiming that price action proves manipulation.

**Why this priority**: The first useful outcome is a defensible event manifest. Granular data, onchain enrichment, and strategy work are premature until the observable event population and its coverage are reproducible.

**Independent Test**: Supply an admitted discovery dataset and frozen experiment definition, run discovery twice in fresh environments, and verify that both runs produce the same event-manifest content while every result remains explicitly conditional on the enumerated roster and measured coverage.

**Acceptance Scenarios**:

1. **Given** a target frame with incomplete historical inventory, **When** discovery completes, **Then** every claim is scoped to the enumerated roster and reports unknown or insufficiently covered units rather than silently excluding them.
2. **Given** admitted trade or bar observations, **When** a pump-and-reversal trigger completes, **Then** the output is labeled `episode_detected` only after the giveback window closes and is not labeled manipulation.
3. **Given** a delisted, rebranded, or migrated instrument, **When** it enters the roster, **Then** its venue instrument, token contract, economic asset, and time-bounded mappings remain distinguishable and are never joined solely by ticker.
4. **Given** zero or very few detected episodes, **When** the run finishes, **Then** it publishes the null or insufficient-evidence result without loosening the frozen criteria.

---

### User Story 2 - Obtain a Confirmatory Result Without Hindsight Leakage (Priority: P2)

As a research reviewer, I can verify that evaluation information was sealed before experiment choices, that every permitted disclosure was fixed in advance, and that the released result came from one non-adaptive analysis reproduced independently in a fresh environment.

**Why this priority**: A deterministic calculation is not credible if analysts can inspect evaluation outcomes, tune boundaries, or retry selectively before publication.

**Independent Test**: Exercise the full genesis, discovery, confirmatory, and release commitment sequence with synthetic evaluation data, including unauthorized access, stale timestamps, a changed log head, a retryable failure, a semantic mismatch, and a clean run; verify that only the clean run reaches confirmatory release.

**Acceptance Scenarios**:

1. **Given** prospective evaluation data, **When** any access occurs before the genesis commitment or outside its disclosure contract, **Then** the affected scope is deterministically downgraded to exploratory.
2. **Given** a permitted pre-confirmation aggregate release, **When** it is delivered, **Then** the release artifact and current custody head are independently timestamped first and the disclosure budget is updated.
3. **Given** discovery is complete, **When** the primary trigger is locked, **Then** the confirmatory commitment includes one primary cell, the complete analysis and comparison rules, a closed custody head, and no later adaptive change is permitted.
4. **Given** two sealed executions, **When** their semantic outputs differ or result-bearing material is exposed early, **Then** the run fails and no output is selected as the preferred result.
5. **Given** a successful semantic comparison, **When** the result is released, **Then** a final transactional checkpoint covers every execution, retry, artifact, access, comparison, and unsealing authorization.

---

### User Story 3 - Enrich Only Selected Windows Under Separate Authorization (Priority: P3)

As the research owner, I can evaluate the need for granular market or onchain evidence after episode discovery, authorize a bounded purchase or pilot separately, and prevent provider choice or selective coverage from determining the mechanism conclusion.

**Why this priority**: Historical L2/L3 and onchain storage is expensive. The workflow must quantify the small set of relevant windows first and must not turn an inexpensive discovery phase into an implicit provider commitment.

**Independent Test**: Starting from a frozen Stage-1 event/control manifest, verify that no quote, pilot, acceptance read, purchase, or mechanism-bearing data access occurs until the relevant hypothesis, sampling, provider-ranking, admission, selection, custody, and user-authorization gates are complete.

**Acceptance Scenarios**:

1. **Given** Stage 1 is incomplete, **When** granular enrichment is requested, **Then** the request is rejected without a provider call.
2. **Given** Stage 1 is complete but the user has not authorized spend, **When** a paid quote, query, pilot, or purchase is attempted, **Then** it is rejected.
3. **Given** several candidate datasets, **When** more than one passes admission, **Then** the precommitted ranking and tie-breaking rule selects the dataset mechanically before mechanism-bearing content is exposed.
4. **Given** only detected cases can be enriched, **When** results are reported, **Then** they are labeled case studies and cannot support prevalence, discrimination, or population claims.
5. **Given** a dataset without admitted market-by-order evidence and venue semantics, **When** a queue-position or L3 claim is attempted, **Then** the claim is rejected.

---

### User Story 4 - Audit Evidence and Claims Over Time (Priority: P4)

As an auditor, I can trace every published statement to admitted evidence, a bounded claim tier, experiment commitments, source status, and immutable lineage, including later corrections, revocation, or legal-status changes.

**Why this priority**: Hard evidence requires more than a persuasive report. Claims must remain reviewable after sources, licenses, labels, or integrity assessments change.

**Independent Test**: Trace a detected episode, a venue allegation, a mechanism-consistency claim, and a proven-manipulation claim through their evidence and lifecycle; then revoke one dataset and verify that dependent claims are invalidated without deleting history.

**Acceptance Scenarios**:

1. **Given** an observable episode without authoritative manipulation evidence, **When** it is published, **Then** its status is `episode_detected` and `not_proven`, never `non_manipulated` or `manipulation_proven`.
2. **Given** a venue sanction, **When** it is recorded, **Then** it receives the distinct `venue_sanctioned` tier and does not become proof.
3. **Given** a final court or regulatory finding or explicit admission, **When** the strongest tier is assigned, **Then** the claim is limited to the named actors, instruments, venues, and periods established by that authority.
4. **Given** a source becomes revoked, expired, or quarantined, **When** its state changes, **Then** dependent manifests, results, and claims are invalidated while their lineage remains available.

### Edge Cases

- The outer instrument roster is incomplete or cannot be proven complete.
- A symbol is reused, a token migrates contracts, or a rebrand overlaps venue histories.
- A current web page describes an old event but has no archival proof of historical availability.
- An event window crosses a discovery/evaluation boundary or target-frame endpoint.
- A source correction arrives after discovery, after confirmation, or after unblinding.
- Reported volume is inflated, duplicated across venues, or affected by a quote-asset dislocation.
- No suitable matched control exists, common support fails, or a control later becomes an episode.
- A disclosure query can reveal information through differencing despite using permitted fields.
- The custody log head changes while a commitment or release receipt is being created.
- A timestamp receipt is pending, invalid, or trusted under the wrong policy.
- A retry produces partial result-bearing logs or exceeds the committed attempt limit.
- A previous experiment has already unblinded an overlapping evaluation unit.
- A pilot or local cache exposed mechanism-bearing content before provider selection.
- A provider passes fidelity checks but its license forbids retained replay bytes or derived catalogs.
- Stage 2 can enrich cases but not controls, near-threshold episodes, or negative controls.
- A credential is shared, compromised, or rotated during a sealed run.
- A source is revoked after reports have already been produced.

## Requirements *(mandatory)*

### Scope and Authority

- **FR-001**: The system MUST treat one versioned Bolt TOML experiment definition as the sole authority for universe rules, partitions, triggers, controls, source admission, enrichment sampling, estimands, and claim promotion. It MUST name the role allowed to append versions, and every version—not only formal G/D/C/E/P commitments—MUST be hash-linked and registered.
- **FR-002**: NautilusTrader MUST own catalogs, replay, backtest execution, events, fills, snapshots, and reports. Bolt MAY compile a hash-linked immutable NautilusTrader run input but MUST NOT introduce another replay engine or allow replay configuration to redefine experiment meaning.
- **FR-003**: The system MUST treat token-screener outputs only as candidate observations; Bolt MUST admit, content-address, and register them before they can support an experiment claim.
- **FR-004**: Every canonical run MUST identify the experiment version, code version, schema version, replay-engine/catalog versions, dependency set, execution environment, numeric rules, and random seeds.
- **FR-005**: The current feature MUST NOT select a market-data or onchain provider, authorize a purchase, initiate a paid query, or implement a trading strategy.
- **FR-006**: The research workflow MUST remain separate from any predictive or trading experiment; production logic requires a separately approved specification.

### Target Frame, Universe, and Source Admission

- **FR-007**: Before source discovery, the experiment definition MUST freeze the target venues, market families, time interval, time-unit grain, outer-roster construction rule and vintage, inventory sources, reconciliation rules, status precedence, conflict reasons, and roster-completeness classification.
- **FR-008**: Every unit in enumerated roster R MUST receive exactly one of `eligible_observed`, `known_ineligible`, `known_insufficient_coverage`, or `existence_or_coverage_unknown`.
- **FR-009**: When roster completeness is not proven, every result MUST state that it covers enumerated roster R and MUST NOT generalize to the entire market family.
- **FR-010**: The system MUST represent venue instrument, token contract, and economic asset as distinct identities with time-bounded, evidence-backed mappings and explicit series-splice rules.
- **FR-011**: Universe and label facts MUST record valid/event time, original publication or first-known availability time, retrieval time, and every revision or retraction as append-only assertions.
- **FR-012**: Facts lacking archival availability evidence MUST be marked `retrieval_time_attested` and MUST NOT support contemporaneous-availability or predictive claims.
- **FR-013**: Free and official datasets MUST pass the same admission gates as paid datasets. Each source-register entry MUST identify the exact dataset/product/version; provenance and upstream source; query and fields; coverage and gap semantics; query, download, caching, post-termination retention, derived-data, collaboration, publication, and attribution rights; fidelity/completeness evidence; revision behavior; transformations; cost status; evidence hashes; reviewer; decision date; and expiry.
- **FR-014**: A canonical discovery source MUST permit retention of exact immutable inputs or a lossless compact normalized panel sufficient to reproduce detection; otherwise its results MUST remain non-reproducible and exploratory.
- **FR-015**: The admitted-source register MUST report measured delisting, rebrand, migration, and gap coverage without shrinking the target frame to a provider's exposed inventory.
- **FR-016**: Stage 0 MUST perform zero paid queries. A nominally free pilot MUST have confirmed zero-cost status and separate user authorization before execution.
- **FR-017**: Each canonical dataset MUST have an input-vintage cutoff and append-only version chain. D MUST freeze exactly one correction policy: ignore later revisions for the experiment, incorporate them through a deterministic D-frozen sealed process, or create a new experiment version. A correction affecting evaluation after unblinding MUST invalidate confirmatory use of that evaluation information.

### Independent Timestamping and Genesis Commitment

- **FR-018**: Every formal commitment MUST use canonical bytes, a declared hash algorithm, an archived verification record, and a successfully verified independent timestamp before the associated gate is crossed.
- **FR-019**: Pending or invalid timestamps MUST fail closed. A hosted repository event MAY corroborate ordering but MUST NOT be the sole timestamp authority.
- **FR-020**: Before any byte or metadata access that could enter a prospective evaluation range, Genesis Commitment G MUST freeze a conservative provisional evaluation superset E0.
- **FR-021**: G MUST freeze the authorized roles and separated credentials for ingestion, disclosure execution, canonical evaluation, verification replay, custody, experiment decisions, and governance approval.
- **FR-022**: The custodian MUST be distinct from analysis authors, experiment decision-makers, and the governance approver; failed-run diagnostic unsealing MUST require dual control.
- **FR-023**: G MUST freeze ingestion operations, access-event schema, canonicalization, fail-closed consequences, timestamp trust policy, custody and registry anchoring intervals, and a deterministic contamination-propagation rule.
- **FR-024**: When a narrower contamination scope cannot be proven from committed records, the system MUST treat all potentially reachable E0 information and dependent versions as affected.
- **FR-025**: G MUST freeze a noninteractive pre-confirmation disclosure program, including exact tables, groupings, filters, release schedule/count, suppression, rounding, censoring, boundary flags, cross-query/version accounting, and deterministic program identity.
- **FR-026**: G MUST freeze a deterministic E0 narrowing rule or a finite set of candidate ranges with deterministic selection and tie-breaking; the final evaluation range MUST be contained in E0.
- **FR-027**: If E0 contains event time preceding G, every role able to influence later commitments MUST inventory prior caches, notebooks, exports, provider access, experiments, recipients, and machine copies; retained artifacts MUST be quarantined or destroyed, and disclosed exposure MUST remain exploratory.
- **FR-028**: G MUST precede any Stage-0 ingestion or coverage computation touching E0. E0 coverage and attrition releases MUST run only through the committed custodian automation.

### Disclosure and Custody Ledger

- **FR-029**: Every disclosure release MUST record the governing program, input vintage, grouping/filter parameters, output hash, suppression/censoring/rounding, cumulative disclosure state, recipients, sequence, status, and time.
- **FR-030**: Every disclosure delivery MUST execute as a transactional checkpoint satisfying FR-033: its ledger entry and current custody head MUST be locked, independently timestamped, compare-and-swap verified unchanged, and released with a single-use authorization. Unanchored, changed-head, or adaptive delivery MUST downgrade the deterministically affected scope to exploratory.
- **FR-031**: Once the committed disclosure budget is exhausted, no further pre-confirmation release MAY occur.
- **FR-032**: All evaluation and disclosure access MUST form an append-only hash chain whose heads are independently timestamped within the G-frozen maximum interval.
- **FR-033**: Each transactional checkpoint MUST quiesce relevant credentials or acquire an equivalent lock, capture and timestamp the current head, verify by compare-and-swap that the head remains unchanged, and consume a single-use authorization. A changed head MUST restart the checkpoint.
- **FR-034**: A shared credential, prohibited access, missing chain interval, invalid anchor, exceeded custody or registry interval, omitted tail, uncommitted release, or unproven contamination scope MUST fail closed under G's propagation rule. Intermediate registry heads MUST be independently anchored within G's committed registry interval.
- **FR-035**: A credential-compromise or rotation event MUST be appended, governance-approved, and followed by a fresh anchored head; compromise before rotation MUST make the affected scope exploratory.

### Discovery and Confirmation Commitments

- **FR-036**: Before discovery execution, Commitment D MUST freeze the final target frame, roster and source vintages, identity/conflict rules, coverage thresholds, complete-window partition rules, purge/censor policy, trigger candidates, primary estimand, unit of analysis, hypothesis families, dependence/multiplicity policy, controls, enrichment-strata functions, null-result rule, seeds, and version identities.
- **FR-037**: D MUST use G's deterministic narrowing rule, include every prior disclosure receipt and the selected-range feasibility record, and close its custody head transactionally before D is timestamped.
- **FR-038**: Partition membership MUST include the full span of every required computation. The purge span MUST cover trigger warm-up, baseline eligibility, matching-feature lookback, control windows, pump window, and giveback.
- **FR-039**: Boundary candidates MUST receive explicit left- or right-censored status and MUST appear in attrition reporting.
- **FR-040**: Evaluation information MUST NOT revise discovery-period labels, corrections, deduplication, controls, rules, or parameters. During the sealed operation, the committed program MAY construct evaluation labels, deduplication, and controls only from D/C-authorized sealed fields and clocks; control identities MUST freeze before contamination is calculated, and no evaluation-derived intermediate MAY be released until both executions finish.
- **FR-041**: After discovery and before evaluation access, Commitment C MUST freeze exactly one primary trigger cell or an aggregation rule already frozen in D, the primary estimand, unchanged boundaries, final analysis, conformance checks, output comparator, normalization/exclusion rules, numeric tolerances, and retry state machine.
- **FR-042**: C MUST reference a transactionally closed and independently timestamped custody head covering all access and disclosure from genesis. Any later access MUST require a new closing head and a new C.
- **FR-043**: The sealed evaluation operation MUST contain a canonical execution and a credential- and environment-independent deterministic verification replay against the same committed inputs.
- **FR-044**: The verification replay MUST be reported as a reproducibility check, not as an independent analytical implementation; separate conformance fixtures or reference checks MUST test analytical invariants.
- **FR-045**: Conformance evidence MUST cover partitions, purge boundaries, observation construction, trigger clocks, deduplication, matching, multiplicity, comparison behavior, and an independently derived primary-estimand check where practical.
- **FR-046**: The retry state machine MUST distinguish terminal and retryable failures, cap attempts, require identical committed inputs for retries, quarantine partial outputs, record every attempt, and make mismatch or human result exposure terminal.
- **FR-047**: The vocabulary, maximum count, and disclosure content of any minimal failure codes visible to analysts MUST be frozen and included in disclosure accounting.
- **FR-048**: Analysts MUST receive successful results only after both executions finish, compare equal under the committed comparator, and a transactional release checkpoint independently timestamps every execution, retry, replay, result artifact, access, comparison, and unsealing authorization.
- **FR-049**: Every semantic input and output MUST be content-addressed; volatile metadata MUST be normalized or excluded according to the committed comparator.
- **FR-050**: Any change to rules, data, program, comparator, normalization, tolerance, or fix after C MUST create a new experiment version and MUST forfeit confirmatory use of the original evaluation information.
- **FR-051**: Unblinded evaluation units MUST NOT be reused for later confirmation unless they belong to a closed sequential family frozen before first exposure with predetermined adaptation and error allocation; otherwise later versions require untouched, nonoverlapping evaluation information.
- **FR-052**: Every confirmatory report MUST enumerate prior overlapping experiment versions and attempts and MUST apply the frozen multiplicity or claim-tier consequence.

### Retrospective Detector and Controls

- **FR-053**: `episode_detected` MUST be a retrospective label available only after giveback completion and MUST record event anchor, feature cutoff, trigger completion, label availability, and any later decision time separately.
- **FR-054**: The authoritative experiment definition MUST specify exact observation construction, clocks, quote normalization, return and reported-volume definitions, baseline rules, minimum coverage, missing-data behavior, interruptions, trigger windows, warm-up, overlap, cooldown, ties, deduplication, and episode identity.
- **FR-055**: The canonical detector MUST require abnormal return, abnormal reported volume, and subsequent giveback; reports MUST disclose that reported volume may itself be manipulated.
- **FR-056**: Robustness variants MUST remain diagnostics and MUST NOT replace the locked primary trigger after evaluation.
- **FR-057**: Results MUST say `all detected episodes within target frame F, admitted universe U, and coverage C` and MUST NOT claim complete detection of manipulation.
- **FR-058**: Each treated anchor's controls MUST come from the same-time eligible risk set, receive a pseudo-anchor, and use only information available before the committed cutoff.
- **FR-059**: Control matching MUST freeze feature definitions, regimes, algorithm, distance, calipers, randomness, reuse, relaxation, balance, missingness, common-support failure, contamination reporting, and unmatched handling.
- **FR-060**: Future control outcomes MUST NOT affect control eligibility; unmatchable episodes MUST remain reported and MUST NOT be force-matched.

### Selective Enrichment and Separate Authorization

- **FR-061**: Stage 2 MUST begin only after the sealed Stage-1 operation and primary report are complete and MUST NOT amend the primary Stage-1 result.
- **FR-062**: Enrichment strata MUST be deterministic functions frozen in D over detected episodes, matched controls, a fixed near-threshold band, and negative or exogenous-event controls.
- **FR-063**: Before solicitation, quotes, provider coverage, pilots, acceptance reads, or granular access, Commitment E MUST freeze each mechanism claim's predictions, ordering, tests, minimum evidence, falsifiers, competing explanations, missing-field behavior, estimands, multiplicity, and scope.
- **FR-064**: E MUST also freeze candidate eligibility, evidence-packet requirements, acceptance windows/fields, content-neutral acceptance metrics, deterministic provider/dataset ranking and ties, multi-source policy, and prior pilot/exposure handling.
- **FR-065**: E MUST freeze the actual pre-provider enrichment draw, including census/sample choice, strata, sizes, inclusion probabilities, randomization, weighting, substitutions, unavailable coverage, and supported estimand.
- **FR-066**: Provider attrition MUST be measured against the pre-provider draw. Case-only enrichment MUST be limited to named case-study claims.
- **FR-067**: Any quote, pilot, query, or purchase—whether nominally free or paid—MUST require a separate explicit user authorization after Stage 1 reports counts, windows, coverage, storage, rights, maximum accepted cost, and minimum usable coverage.
- **FR-068**: Every exact dataset/product/version MUST pass an E-frozen evidence packet covering query, download, caching, and post-termination retention rights; internal replay, derived-artifact, collaboration, publication, attribution, and upstream rights; exact fields, dates, and venues; corrections, completeness, and gap semantics; timestamp, sequence, snapshot/reset, disconnect, and raw-payload behavior; checksum and replay invariants; quoted units, minimum commitments, overages, expiry, and storage impact. Each gate MUST record its reviewer, evidence hash, decision date, and expiry.
- **FR-069**: Acceptance testing MUST use discovery/negative-control windows or custody automation that releases only E-certified content-neutral integrity metrics.
- **FR-070**: After admission and before mechanism-bearing access, Commitment P MUST mechanically record the selected dataset and fields under E's ranking and freeze query windows, fusion, disagreement, precedence, exclusion, and prior-exposure classification.
- **FR-071**: P MUST NOT legalize earlier access. Selection after mechanism-bearing exposure MUST keep the analysis exploratory.
- **FR-072**: Stage-2 acceptance reads and mechanism-bearing access MUST use an anchored custody chain with transactional closing heads before E, before P, and before confirmatory mechanism-result release.
- **FR-073**: Enrichment storage MUST be limited to committed event, control, near-threshold, and negative windows plus fixed warm-up; admitted bytes and derived replay artifacts MUST be content-addressed.
- **FR-074**: Exact queue-position or L3 claims MUST be rejected without admitted market-by-order evidence and venue semantics.

### Claims, Reporting, and Lifecycle

- **FR-075**: Every published statement MUST be an atomic claim scoped by instrument, venue, period, evidence version, tier, predicted observations, minimum evidence, identity/timing certainty, competing explanations, and falsifiers.
- **FR-076**: `mechanism_consistent_with` MUST require admitted granular evidence matching E-frozen predictions and surviving named falsifiers; it MUST NOT be presented as causal proof.
- **FR-077**: Third-party allegations MUST use `manipulation_alleged`; final venue sanctions MUST use `venue_sanctioned` and MUST NOT count as proof.
- **FR-078**: `manipulation_proven` MUST be limited to final court/regulatory adjudication or explicit admission and MUST remain scoped to what that authority establishes.
- **FR-079**: Any exception to the strongest tier MUST create a new registry version, include conflict-of-interest analysis, and be approved by a role distinct from the claim author.
- **FR-080**: Absence of authoritative evidence MUST mean `not_proven`, never `non_manipulated`; authoritative-case evaluation MUST be positive-unlabeled and MUST NOT estimate population recall from unlabeled episodes.
- **FR-081**: Reports MUST disclose effect sizes, uncertainty, balance, missingness, attrition, clustering, survivorship, alternatives, sensitivity cells, controls, and all null or small-sample limitations. Uncertainty methods MUST respect dependence across tokens, venues, regimes/calendar periods, common-market shocks, and overlapping windows, and reports MUST distinguish temporal, unseen-instrument, and unseen-venue generalization.
- **FR-082**: Confirmatory mechanism reports MUST disclose that E was authored with knowledge of Stage-1 identities and the primary result but without granular mechanism-bearing content; predictions derivable from Stage-1-visible fields MUST have a capped claim tier.
- **FR-083**: Datasets MUST support `active`, `quarantined`, `revoked`, and `expired` states with append-only authorized transitions.
- **FR-084**: Integrity or rights changes MUST invalidate dependent manifests, replay artifacts, reports, and claims without deleting lineage; re-admission MUST require a complete fresh evidence packet and review.

### Scope Boundaries

- No provider is selected or preferred by this feature.
- No paid query, quote, pilot, purchase, or historical backfill is authorized.
- No Surf, Fable, CoinAPI, Tardis, Allium, Arkham, or other provider output becomes source of truth by default.
- No universal multi-year L2/L3/onchain archive is built.
- No market manipulation is inferred solely from price action, reported volume, wallet labels, AI synthesis, or microstructure patterns.
- No predictive signal, backtested trading strategy, execution policy, or production deployment is implemented.
- Concrete providers, venues, dates, thresholds, partitions, costs, identities, and custody services remain later user-approved values.

### Key Entities

- **Experiment Definition**: The single authoritative version of target frame, rules, commitments, estimands, controls, provider gates, and claim limits.
- **Target Frame**: The declared venue, market-family, and time scope whose negative space is measured.
- **Enumerated Roster**: The vintage-specific inventory of venue-instrument-time units and their coverage statuses.
- **Identity Mapping**: Time-bounded relationships among venue instruments, token contracts, and economic assets.
- **Source Register Entry**: Dataset-specific provenance, rights, fidelity, lineage, coverage, cost, review, and lifecycle evidence.
- **Custody Ledger**: The hash-chained record of sealed data access, disclosures, executions, and unsealing decisions.
- **Commitment G**: The prospective evaluation superset, custody, disclosure, timestamp, contamination, and narrowing contract.
- **Commitment D**: The discovery design, final partitions, detector grid, controls, and validation contract.
- **Commitment C**: The locked confirmatory cell, program, comparator, retry behavior, and closed custody state.
- **Commitment E**: The mechanism hypotheses, enrichment sample, candidate-provider rules, and acceptance contract.
- **Commitment P**: The mechanical post-admission dataset selection and mechanism-query contract.
- **Episode Manifest**: The reproducible output containing detected episodes, controls, coverage, missingness, inclusion/exclusion, and lineage.
- **Atomic Claim**: A bounded statement with tier, scope, supporting evidence, alternatives, and falsifiers.
- **Evidence Artifact**: Immutable or losslessly normalized retained input or output identified by content.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The current phase performs exactly zero incremental paid-provider queries and incurs exactly zero new provider spend.
- **SC-002**: One hundred percent of units in enumerated roster R receive a declared status, and all reports publish denominators and attrition.
- **SC-003**: One hundred percent of canonical source inputs have an active admission record, retainable reproducibility artifact, lineage, evidence hash, review date, and expiry.
- **SC-004**: Every canonical episode is reproducible from retained admitted inputs and receives the same semantic identifier and manifest content in two fresh-environment executions.
- **SC-005**: Every confirmatory evaluation has verified G, D, and C commitments, complete timestamp receipts, no unanchored custody tail, and zero prohibited pre-release accesses.
- **SC-006**: All boundary-crossing or insufficient-window candidates are censored and reported; none is silently assigned to discovery or evaluation.
- **SC-007**: Exactly one primary confirmatory cell or one D-frozen aggregation rule is executed per evaluation information set; no unblinded unit is reused outside a precommitted sequential family.
- **SC-008**: One hundred percent of control matches use same-time risk sets and pre-anchor information; all unmatched and failed-balance cases remain visible.
- **SC-009**: Every Stage-2 quote, pilot, query, or purchase has a distinct recorded user authorization; without it, provider calls remain zero.
- **SC-010**: One hundred percent of mechanism-bearing datasets are selected mechanically under E/P and pass dataset-specific rights, fidelity, replayability, lineage, and cost gates before content access.
- **SC-011**: Stored granular history is limited to authorized event/control/near-threshold/negative windows plus committed warm-up; no universal multi-year granular archive is created.
- **SC-012**: Zero queue-position or L3 claims are published without admitted market-by-order evidence and venue semantics.
- **SC-013**: One hundred percent of published claims carry a tier and evidence scope; zero observable episodes are automatically labeled manipulation.
- **SC-014**: Every source quarantine, revocation, or expiry identifies and invalidates all dependent active claims while preserving their audit trail.
- **SC-015**: Null, small-sample, leakage-failed, mismatch, and insufficient-evidence outcomes remain publishable and cannot be converted to confirmatory success by changing frozen rules.

## Assumptions

- The user will approve concrete venues, dates, thresholds, costs, role assignments, timestamp services, and custody mechanisms in later experiment versions.
- Existing Surf, CoinAPI, and Tardis pilot observations remain non-admission evidence; they do not authorize provider selection or canonical replay.
- Historical data already seen by analysts may force affected ranges to exploratory status unless a valid prior commitment or exposure inventory supports stronger treatment.
- Delisted-instrument coverage, reported-volume quality, authoritative labels, and regime generalization will remain imperfect and must stay explicit limitations.
- Independent timestamps and custody logs make ordering and tampering more auditable but cannot prevent collusion, omitted pre-genesis knowledge, or all side channels.
- Existing NautilusTrader research/backtest surfaces remain the execution and replay foundation; this feature defines research meaning and evidence gates rather than replacing those capabilities.
- Existing research evidence files under `docs/research/manipulated-pump-*` remain supporting observational records, not canonical provider admission.
