# #1016 D3 trusted authority control-plane proposal

Related authority and sequencing:

- [GitHub issue #1016](https://github.com/seungpyoson/bolt-v2/issues/1016)
- [Atomic verifier replacement design](2026-07-12-issue-1016-atomic-ci-verifier-rewrite-design.md)
- [Program ledger](../../ci/1016-program-ledger.md)

## Status and decision boundary

This is a design proposal only. It does not authorize implementation, installation of a GitHub App, repository or Mergify mutation, a rehearsal, or any check publication. Hosting-provider selection is deliberately deferred.

The selected architecture is a minimal external GitHub App service with a durable append-only state primitive. The service authenticates evidence, invokes the exact protected-base semantic engine in an isolated launcher, enforces protocol and state transitions, and alone publishes the App-qualified check. It contains no repository policy and cannot turn a candidate-produced verdict into authority without independently validating the complete protected-base proof.

D4 must derive numeric retry, time, and abort budgets from rehearsal. D5 remains an explicit owner decision; this design does not accept the post-precursor unbounded terminal-tail outage risk.

## Considered approaches

### 1. Candidate GitHub Actions publishes the required check

This reuses existing infrastructure and is inexpensive, but it cannot establish the required publisher boundary. Current required checks bind to the shared Actions App, candidate-controlled workflow execution can influence the publishing path, and Actions artifacts are neither an append-only authority ledger nor rollback-resistant state. Rejected.

### 2. External App service plus an ordinary mutable database

This supplies a distinct publisher identity and could implement the happy path with few components. It does not close snapshot rollback, old-service restoration, bootstrap reissuance, proof reuse, or audit deletion/equivocation. A nonce does not repair those trust failures. Rejected.

### 3. External App service plus one append-only, rollback-resistant state primitive

Selected. One service owns webhook intake, protected-state observation, launcher orchestration, validation, and check publication. One durable state primitive supplies conditional append, a monotonic sequence, immutable hash-chained records, unique proof-use keys, and an irreversible bootstrap tombstone. A separate immutable artifact bucket is used only for large evidence referenced by digest; it is not a verdict store. This is the smallest design that provides a distinct publisher, protected-base semantic execution, replay prevention, irreversible bootstrap closure, and reconstructable authority under GitHub `exempt`.

The provider may realize the state primitive with a managed ledger or a transactional append log plus immutable/WORM anchoring, but it must satisfy the interface and rollback drills below. A mutable SQL table with application-enforced `UPDATE` restrictions is not equivalent.

## Trust boundary and minimal components

1. **GitHub:** protected refs and objects, App/installation identity, rulesets, review enforcement, Mergify proof-head objects, webhook delivery identifiers, and Checks API.
2. **Mergify:** protected configuration, queue/batch construction, proof-head creation, `exempt` execution, Freeze and Merge Protections during the ceremony.
3. **Authority service and publisher runtime:** a small external GitHub App service. It accepts events and approvals, observes GitHub/Mergify state, drives the launcher, validates policy-agnostic protocol structure, appends state, and publishes checks. It has no repository-rule definitions, parsers, corpus selection, applicability logic, or success exceptions. The deployed binary/runtime and its App signing boundary are one catastrophic TCB: compromise can publish directly to GitHub outside the ledger. The ledger detects but cannot cryptographically prevent that direct credential misuse.
4. **Sandboxed launcher:** fetches exact Git objects and the attested protected-base bundle, supplies candidate documents as inert inputs, enforces resource and output limits, and returns the engine's canonical protocol document plus execution metadata. It cannot authenticate to the Checks API or mutate control-plane state.
5. **Artifact store:** immutable, content-addressed storage for attested launcher/engine bundles and larger evidence. All authority records contain their digests. Retention and deletion policy are selected and live-proven before reliance.
6. **Append-only state:** the honesty root for ordered attempts, approvals, proof consumption, protected-state epochs, key events, terminal re-query receipts, publication receipts, and the bootstrap tombstone. It exposes conditional append and rollback detection, not arbitrary record update/delete.
7. **Keys:** the GitHub App installation credential, an audit-record signing key, builder/attestation verification roots, and hardware-backed operator approval credentials.

No queue worker, mirror database, second publisher, fallback status context, policy service, or permanent ceremony controller is added. Horizontal replicas of the authority service are one logical component and coordinate only through the append-only state. Operational mitigations for the catastrophic publisher boundary are a minimal separately reviewed publisher build, source-to-artifact attestation, immutable deployment identity, no interactive shell, least-privilege workload identity, tightly restricted ingress/egress, short-lived installation tokens, deployment admission tied to the active artifact epoch, independent check-versus-ledger monitoring, and rehearsed credential revocation. These reduce probability and detection time; they do not turn runtime compromise into a fail-closed event.

## Principals, roles, permissions, and quorum

Named identities and backups must be recorded before live proof; role names below are not substitutes for that inventory.

| Principal | Capability | Explicit denial |
| --- | --- | --- |
| GitHub App installation | Repository metadata and contents read; pull request, checks, ruleset state, and installation identity read; Checks write only for its two owned contexts | No contents, refs, issues, PR, ruleset, Actions, administration, or Mergify write |
| Authority runtime | Use installation credential; append records; request sandbox runs; publish after state-machine authorization | Cannot approve epochs, promotion, ceremony mutations, or its own key changes |
| Launcher | Read exact Git objects and approved artifacts; write one bounded result to the authority service | No GitHub/Mergify write credentials, state-store credential, App key, approval keys, or network except allowlisted object/artifact reads |
| Builder | Produce source-to-artifact attestation for reviewed source | Cannot promote, approve, publish, or mutate GitHub/Mergify |
| Control approvers | Hardware-backed signatures over exact approval objects | No direct publisher credential or unilateral mutation authority |
| Ceremony operators | Execute separately approved ruleset/Mergify/Freeze operations | Cannot make those operations authoritative without quorum approval and terminal re-query |
| Audit reader/incident owner | Read and reconstruct retained records | Cannot append authority or publish checks |

Control-plane epoch creation, artifact promotion, bootstrap issuance, tombstone transition, App/key rotation, and every ceremony control mutation require **two distinct approvals from a registered three-person owner set**. At least one approver must be independent of the operator who executes the mutation; the service/runtime, builder, and executing operator cannot count as both approvals. Steady-state proof evaluation and check publication are automated under the already-approved active epoch and do not require a new human quorum per PR. Native code-owner approval remains independently mandatory for merges but is not a semantic verdict or control-plane quorum signature.

An approval is a signed canonical object containing schema version, operation, repository ID, protected-state epoch digest, exact proposed mutation or artifact digest, purpose, expiry, cancellation epoch, and a unique approval nonce. Approvals are single-use, operation-specific, and invalid on expiry, revocation, conflict, epoch movement, digest mismatch, or signer/key movement. Cancellation is an append-only record and wins before execution. Conflicting approvals terminally block that operation pending a newly approved epoch; they are never reconciled by the publisher.

## Canonical authority record

Records use UTF-8 JSON canonicalized with RFC 8785. Unknown, duplicate, missing, null-substituted, or non-canonical fields fail. The service signs the canonical bytes with a purpose-bound audit key; the tuple digest is `SHA-256(canonical_authority_tuple)`. Compact references never replace the explicit tuple in retained audit evidence.

The signed authority tuple contains:

- schema and protocol versions;
- immutable GitHub repository node ID, owner/name observation, protected ref name, and protected ref SHA;
- GitHub App integration ID, installation ID, installation owner ID, owned context, and purpose (`bootstrap-promotion`, `canary`, `activation`, `steady-state`, `terminal-requery`, or `operator-mutation`);
- ruleset ID, API-observed version/etag where available, canonical payload digest, and required App-qualified check binding;
- protected `.mergify.yml` blob SHA/digest, approved Mergify epoch, integration identity, bypass mode, queue/routing digest, Freeze identity/state/exclusions, and Merge Protections state;
- launcher source revision, immutable artifact digest, builder identity and attestation digest; engine bundle digest and promoted version;
- `policy_base_sha`, its tree SHA, protected-base manifest/corpus digest, and protected-base engine version;
- exact proof-head SHA, proof ref, ordered parent SHAs, tree SHA, event kind and GitHub delivery ID;
- ordered constituent PR numbers, head SHAs, base SHAs, merge-base identities, and expected batch identity;
- activation manifest digest, staged version, staged-byte/tree digest, and expected protected-main result when ceremony-purpose;
- control-plane epoch, invocation nonce, attempt number and lineage root, prior ledger sequence/hash, tombstone generation/value, and relevant key versions;
- engine terminal classification and ordered finding digest;
- terminal re-query receipt digest, completion time from the control plane's trusted clock, and record sequence.

The terminal re-query receipt repeats the live identities needed for publication rather than merely referring to an earlier observation. It binds the exact protected ref/object ancestry, ruleset, Mergify config and epoch, Freeze/exclusions, queue/batch/constituents, App installation, artifacts, manifest, tombstone, proof head, and outstanding conflicting/duplicate attempt census immediately before publication.

## Protected-base and proof-head data flow

1. A GitHub/Mergify event is treated as an untrusted trigger. The service records its delivery ID, then independently queries the installation, repository, protected ref, proof head, parents/tree, constituents, reviews, ruleset, checks, and current control state.
2. The service selects no semantic rules. From the exact protected `policy_base_sha`, it resolves an already promoted and source-attested engine/manifest bundle. Vn therefore judges the candidate or staged Vn+1.
3. It constructs the complete tuple, allocates a fresh nonce by conditional append, and rejects duplicate delivery, proof-use, active attempt, purpose mismatch, ordinary PR head, unauthorized constituent, or moved state.
4. The launcher receives only the immutable bundle, exact Git objects, tuple digest, and nonce. Candidate code is never executed as the semantic engine. Network, time, memory, process, filesystem, output count, and protocol channel are bounded.
5. The service validates the engine's one canonical JSON result using a closed, policy-agnostic protocol: exact tuple/nonce/bundle/manifest commitment, supported schema, terminal enum, canonical field and finding representation, protocol-defined uniqueness and ordering, and internal digest consistency. The protected-base engine alone owns rule IDs, rule ownership, applicability, semantic completeness, and whether findings are omitted or present. The publisher never enumerates rule IDs, recomputes expected findings, or reads repository policy to reinterpret the result. Crash, timeout, signal, extra output, malformed data, unknown field/classification, neutral, skipped, or structurally incomplete output is non-authorizing.
6. For a prospective success, the service performs the complete terminal re-query. Any identity or epoch movement invalidates the attempt. It conditionally creates one `PUBLICATION_RESERVED` record under the nonce-independent authority-domain key `(repository_node_id, protected_ref, context/purpose, authority_epoch, policy_base_version, policy_base_sha, proof_head_sha, manifest_digest)` and records the attempt nonce beneath it. That key permits at most one terminal authorizing record and one publication reservation. A regenerated nonce never creates a new authority domain. A regenerated proof head is a new domain only after the old domain is durably marked `SUPERSEDED_NONPUBLISHABLE` and immutable-state/budget rules authorize regeneration.
7. After the reservation commits, the service performs a second exact terminal re-query and compare-and-appends `READY_TO_PUBLISH`; movement marks the reservation non-publishable. A single elected publisher instance then makes at most one non-retried Checks API create call with the authority-domain digest as `external_id`. If the response is lost or ambiguous, the reservation remains `PUBLISHING_UNCERTAIN`: no instance may call create again. Reconciliation repeatedly queries exact SHA/context/App check runs. A uniquely matching `external_id` is bound to the reservation even if it becomes visible after an arbitrary delay, then follows the normal post-publication validation path. Duplicate or mismatched runs are terminal security findings but do not make the proof head safe to abandon. Absence after any observation window is inconclusive: `PUBLISHING_UNCERTAIN` may not transition to terminal abandonment, supersession, or replacement while that exact proof head remains merge-eligible.
8. Before an uncertain domain can be abandoned or a regenerated proof head can receive a publication domain, the service must append durable, independently re-queried proof that the old exact proof head was dequeued and invalidated or superseded, is not the current queue/batch head, and cannot merge under current Mergify, Freeze, ruleset, base, and constituent state. It must re-query for a delayed matching check both before and after invalidation. If a delayed check appears after invalidation, it is recorded and must remain merge-ineligible by the same stale-proof invariant; it never reactivates the old domain. If inability to merge cannot be proven, the domain remains uncertain indefinitely and all replacement publication is prohibited. D4 exhaustion does not override this safety hold.
9. On observed API success, the service immediately re-queries the full protected/control/queue state, the check, and the exact proof head, then appends `PUBLISHED_VALIDATED` plus the publication receipt. Publication is safe only because Mergify and the protected ruleset must, as a mandatory disposable-live-proof invariant, re-evaluate the exact current proof head/base/constituents and reject or regenerate a stale proof head after protected-base, queue, constituent, configuration, Freeze, ruleset, or bypass movement. The tuple also binds the expected post-merge main object and ancestry. If that invariant cannot be demonstrated at every cut point—before reservation, before API call, during arbitrarily delayed create/read visibility, after a lost response, after check creation, and immediately before merge—the architecture is not implementable and no ceremony may begin. An already-created success is never assumed retractable; its safety depends on this exact-head invalidation property and on GitHub/Mergify/ruleset administrators remaining inside the TCB.
10. After GitHub reports the merge, the service admits the new protected-main SHA only if it exactly equals the tuple's expected result and descends from the authorized base/proof lineage. It appends that transition and marks every other unpublished domain based on the old main `SUPERSEDED_NONPUBLISHABLE`. Any discrepancy is terminal and cannot be repaired by updating a check.

Feedback uses `trusted-ci-verifier-feedback`, is visibly advisory, and can never be referenced by the ruleset or converted into authority. The authoritative `trusted-ci-verifier` context is accepted only from the fixed App integration and installation IDs proven at installation.

## State machine and replay closure

The append-only state has these phases:

`UNINSTALLED -> INSTALLED_NONAUTHORITATIVE -> EPOCH_APPROVED -> BOOTSTRAP_ISSUED -> PROMOTED -> TOMBSTONED -> CANARY_TERMINAL -> ACTIVATION_TERMINAL -> STEADY_STATE`

There is no backward transition. `TERMINAL_ABANDONED` may be entered from any active ceremony attempt; after precursor it preserves Freeze and evidence and exposes no recovery or publication transition.

- **Epoch:** Every protected control/artifact/key set has a monotonically increasing generation and digest. Movement creates a new proposed epoch and invalidates approvals and attempts; it cannot silently amend an epoch.
- **Nonce:** Generated by the service after a conditional attempt append. It is unique and purpose-bound, but is not the honesty root.
- **Authority domain and attempt lineage:** The create-only uniqueness key is `(repository_node_id, protected_ref, context/purpose, authority_epoch, policy_base_version, policy_base_sha, proof_head_sha, manifest_digest)` and deliberately excludes nonce. It admits at most one authorizing terminal record and one publication reservation. Nonces identify attempts beneath the domain for freshness and audit only. A fresh nonce can resume or replace a pre-publication infrastructure attempt under the same domain within D4 limits, but cannot reopen a terminal domain or create another success.
- **Tombstone:** Promotion completion requires quorum-authorized append of one repository/bootstrap tombstone with a strictly higher monotonic generation. Once observed, all service versions and state restorations must reject bootstrap issuance and acceptance. The state primitive must detect a restored snapshot or stale replica whose signed checkpoint/sequence predates the externally durable tombstone anchor. Ambiguity or unavailable quorum fails closed.
- **Old binary defense and limit:** Each service/launcher version is attested and admitted by the active deployment epoch; deployment admission and workload identity reject an old artifact, and ledger-aware code rejects stale checkpoints, keys, and tombstones. These controls do not prevent a compromised publisher runtime or a party able to use its App signing interface from bypassing the ledger and calling GitHub directly. That is an explicit catastrophic residual risk monitored by comparing every App-owned check to a signed publication receipt and mitigated operationally as described above.
- **Create-only publication:** A failed, partial, feedback, canary, retry-noise, or prior-purpose record has no transition to authority. A successful retry is a wholly new terminal record for its exact regenerated proof head.

Backups may restore artifacts and append records for disaster investigation, but may not lower the observed monotonic checkpoint. Recovery that cannot prove continuity through the last durable signed checkpoint leaves publication disabled. Key or state loss has no availability fallback.

## Ceremony and steady-state publication rules

Before the precursor, the App is non-authoritative and publishes no `trusted-ci-verifier`. A bootstrap envelope authorizes exact-byte verification and promotion only; the launcher path for that purpose has no Checks API capability. After promotion the tombstone is written before the closed canary. Canary records are a separate purpose/domain, remain internal, and cannot create or update either App context.

During the ceremony, `trusted-ci-verifier` may be created only for the reserved activation's exact Mergify proof head after a clean canary, unchanged final controls, complete engine success, terminal re-query, durable proof consumption, and exact constituent validation. Ordinary PR heads, precursor heads, merge-queue noise, unexpected batches, and other proof heads receive no authoritative check.

After activation is on protected `main`, terminal main proof succeeds, and temporary controls are removed, the state enters `STEADY_STATE`. The App publishes authority only for exact Mergify proof heads under the active protected-base authority epoch. The current protected-base Vn judges proposed Vn+1; a candidate cannot activate its own semantics. Ordinary PR heads remain non-authorizing.

Steady-state main advancement is a separate monotonic transition, not an authority-epoch change. An ordinary proof may advance `main` from base B to expected descendant R under the same Vn engine/manifest/corpus identities only when the protected-base engine and protected manifest—not candidate metadata—classify the change as outside every protected semantic-authority surface. After exact merge observation, R becomes the next admissible policy-base repository SHA while the semantic authority version remains Vn; every other old-B publication domain becomes non-publishable, and the live Mergify proof must show that old-B proof heads cannot merge after advancement. A governed Vn+1 change to engine, manifest, corpus, semantic registry, authority protocol, launcher trust boundary, ruleset/Mergify authority configuration, App installation, key epoch, or other enumerated authority surface requires the separately governed staging/activation path. Out-of-band, unauthorized, non-descendant, ambiguous, or unexpected protected-ref movement is terminal; it never becomes a normal base roll-forward.

Only explicit engine `ALLOW` plus complete terminal validation maps to GitHub conclusion `success`. `DENY` maps to failure with stable finding IDs. Every other outcome results in absence of success; where useful, a failure/feedback check may be created under its own terminal record, but it can never later be updated to success.

## Keys and artifact lifecycle

- The App installation credential and audit signing key reside in non-exportable managed key custody or an equivalently reviewed signing boundary; plaintext keys are unavailable to launcher, builders, operators, logs, and artifacts. Hosting selection must prove GitHub JWT compatibility without weakening this property. Non-exportability prevents extraction, not a compromised authorized runtime from requesting signatures or installation tokens; the publisher runtime and signing boundary therefore remain catastrophic TCB.
- Operator approvals use individually registered hardware-backed credentials. Shared accounts and bearer approval tokens are forbidden.
- Builder attestations bind source commit, clean source tree, build recipe/image digest, dependency lock digests, builder identity, artifact digest, and output manifest. Promotion requires quorum approval of that exact attestation and independent digest verification.
- Each key has a stable key ID, purpose, owner, creation/activation/expiry, algorithm, and status in the ledger. Every signature records the key version.
- Rotation is a monotonic epoch transition: the old key authorizes the new verification key under quorum, outstanding attempts are invalidated, and only one App installation/publisher epoch may publish. Verification overlap for historical audit does not create dual publication authority.
- Revocation is append-only and immediately blocks new append/publication under that key. Compromise, loss, ambiguous custody, or failed revocation propagation fails closed. Emergency action may revoke and stop publication; it cannot approve a result, restore bootstrap, bypass Freeze, or create alternate authority.
- Rotation, revocation, key-loss, old-key, wrong-purpose, and compromised-builder drills are required before reliance and periodically thereafter. Cadence, retention, and recovery objectives remain owner-approved deployment decisions.

## Durable audit and `exempt` compensation

GitHub `exempt` removes GitHub-side bypass audit entries, so the control plane retains enough signed evidence to reconstruct every attempted and completed emission:

- event/delivery intake and duplicate disposition;
- protected-state observations and complete signed terminal re-query;
- approvals, cancellations, mutations, epochs, keys, attestations, promotions, and tombstone;
- attempt lineage, nonce, launcher artifact/input/output digests, resource/terminal classification, findings digest, and proof consumption;
- Checks API request digest, response identity, subsequent exact App/context/SHA re-read, and publication receipt;
- cleanup and periodic signed control-state checkpoints.

Records are hash-chained, sequenced, signed, immutable, and access-logged. Large evidence is content-addressed in immutable storage and referenced by digest. Missing sequence, divergent chain, invalid signature, deletion, stale checkpoint, or retention failure disables publication and alerts the incident owner. Sensitive API tokens and candidate secrets are never recorded; schemas allow only identifiers, digests, bounded findings, and redacted error classes. Audit reconstruction is evidence, not a second verdict engine: it cannot reinterpret semantic findings or satisfy a check.

The retention duration, access roster, cost ceiling, and incident reconstruction objective are explicit pre-reliance owner decisions. The live proof must reconstruct at least one allow, deny, retry, wrong-publisher, moved-state, tombstoned-bootstrap, and publication-fault attempt end to end.

## Retry and terminal behavior

The machine-readable retry allowlist is limited to stale API observation, API/network timeout, cancelled runner, blocked extra queue entry, and proof-head regeneration for the same exact activation/base/configuration tuple. A retry is allowed only after re-query proves every protected identity unchanged, with a fresh nonce and attempt record beneath the same authority domain, and while later D4 owner-approved count/time budgets remain. No earlier engine result, terminal receipt, check, or artifact becomes the retry's authority.

Webhook and re-request behavior is closed:

- A duplicate delivery ID returns the previously recorded disposition and causes no state transition.
- `check_run.rerequested` or `check_suite.rerequested` for an App-owned context never reopens a terminal domain and never creates another check. If the domain is pre-terminal, it only triggers read-only reconciliation/resumption of the existing reservation; if terminal, the service acknowledges it as `IGNORED_TERMINAL` and records the event.
- An operator/API retry after a timeout likewise reconciles the existing domain. While `PUBLISHING_UNCERTAIN`, it may query but never repeat the create call. Neither elapsed time nor D4 budget exhaustion permits abandonment or replacement until durable exact-head dequeue/invalidation proof establishes that a delayed success cannot merge.
- A new Mergify proof-head SHA creates a new domain only when the prior domain is `SUPERSEDED_NONPUBLISHABLE`, all immutable identities remain unchanged, and D4 retry rules allow it. Same-SHA reruns remain the same domain.
- Disposable live proof must establish which same-name run GitHub rulesets and Mergify consume and demonstrate that duplicate/same-name ambiguity fails closed. The design does not rely on newest-run selection for safety.

Merits deny; App/installation, artifact, tuple, ancestry, constituent, approval, key, tombstone, or epoch mismatch; protected-state movement; malformed/unknown/incomplete protocol; monotonic-state ambiguity; unauthorized queue entry; or retry-budget exhaustion is terminal. Service dependency outage, state/key unavailability, API ambiguity, and publication uncertainty fail closed; they do not invoke a fallback publisher or semantic path. Publication uncertainty is reconciled by exact App/context/SHA re-read and the proof-use record, never by blindly retrying success creation.

Before precursor, terminal or over-threshold failure follows the separately reviewed abort/restore procedure. After precursor, terminal abandonment keeps Freeze active, preserves evidence, and exposes no recovery PR or alternate authority. Incident escalation may notify owners and seal evidence only. D5 acceptance remains required before entering that state space.

## Service API boundaries

The implementation exposes the smallest necessary authenticated interfaces:

- `POST /github/webhook`: validates GitHub signature and delivery uniqueness; trigger only, never authority evidence by itself.
- `POST /approvals`: accepts canonical hardware-signed approval/cancellation objects for an exact epoch/operation.
- `POST /operations/{id}/execute`: available only to named ceremony operators after quorum; executes the exact approved control-plane operation and appends observations. It does not accept arbitrary API payloads.
- internal `CreateAttempt(tuple_digest, purpose)` and `CompleteAttempt(result, terminal_receipt)`: mutually authenticated launcher/service calls with conditional ledger transitions.
- read-only `GET /audit/{record-or-attempt}` for authorized reviewers and incident reconstruction.

There is no general-purpose GitHub proxy, arbitrary command endpoint, direct verdict endpoint, success override, record update/delete API, bootstrap reset API, or operator endpoint that writes a check. The sole publisher is an internal state-machine transition after durable terminal commit.

## Required evidence before implementation reliance

1. Threat model and trust-boundary review showing candidate bytes cannot select base, engine, corpus, applicability, verdict, publisher, or success conversion.
2. Canonicalization/schema mutation suite for every missing, extra, duplicate, wrong-type, wrong-version, reordered, stale, and substituted tuple field.
3. Launcher isolation tests: candidate executable payload, symlink/path escape, network attempt, fork/resource exhaustion, timeout, crash, signal, extra output, malformed UTF-8/JSON, and artifact substitution all fail closed.
4. Quorum tests for positive, missing, stale, revoked, expired, wrong-operation, wrong-digest, duplicate-signer, operator-self-approval, and conflicting approvals.
5. Concurrency and replay tests: duplicate webhooks, racing replicas, check reruns, regenerated proof heads, proof-use collision, stale replica, restored snapshot, lost response, partial append, and old binary/key cannot republish. Integration fault injection must delay Checks API create completion and read visibility beyond every configured observation window, then prove that no retry/replacement occurs while the old proof head is eligible, a delayed check is adopted when it appears, and abandonment becomes possible only after durable dequeue/invalidation plus before-and-after delayed-check reconciliation.
6. Artifact/build proof: reviewed source maps to exact launcher/engine bytes; wrong builder, recipe, dependency, manifest, digest, or partial promotion fails.
7. Key drills: rotation, revocation, compromise, loss, verification-only historical overlap, and installation identity change.
8. Audit drills: tamper, deletion, chain fork, missing blob, retention expiry, redaction leakage, and complete incident reconstruction under `exempt`.
9. Disposable live matrix for exact App-qualified check binding, same-name wrong publisher, check rerequest/suite rerequest/duplicate delivery, Checks API lost-response and duplicate-run faults, permissions, proof-head and constituent identity, GitHub/Mergify/Freeze/exclusion/injection behavior, queue races, direct/native blocking, exact post-publication re-query, stale-proof invalidation, and merge-time re-evaluation at every publication cut point. Inability to prove stale success ineligible is an implementation blocker.
10. Semantic-boundary mutation proof: repository rules, corpus, rule IDs, ownership, or applicability change only the protected-base engine/manifest artifact and require no authority-service code or configuration change; the publisher validates only canonical structure, cryptographic commitments, and protected-base facts.
11. D4 rehearsal measures latency, successful-path duration, dependency faults, retries, resource/capacity use, and operational/storage/API cost. It proposes—but D3 does not choose—the count/time budget and pre-precursor abort threshold.
12. Independent adversarial security, minimality, availability, and permanent-debt review before production implementation or any live control mutation. That review must explicitly accept or reject the catastrophic publisher-runtime/App-signing residual risk; append-only audit is detection and reconstruction, not prevention of direct credential misuse.

## Explicit non-goals and cleanup

This design does not implement or select a hosting provider; choose numeric D4 budgets; accept D5; define repository policy, corpus disposition, parsers, rule IDs, or runtime CI behavior; authorize a recovery PR; add old/new comparison, a compatibility adapter, a fallback scanner, dual verifier, bare-name check, candidate-head policy, or emergency success route; or broaden #1016 into unrelated Python cleanup.

Ceremony-only approval objects, bootstrap issuer, promotion endpoint, canary runner mode, mutation executor permissions, and temporary-control observation schedules are disabled and cryptographically unreachable after the terminal activation/cleanup record. Freeze, Merge Protections bindings/reporting, exact-number admission lock, exclusions, and other temporary repository/Mergify controls are removed and their absence is included in the final signed cleanup receipt. Bootstrap and historical canary records remain immutable audit evidence, but no executable bootstrap acceptance path remains.

Permanent components are limited to the external App authority service, sandbox launcher, promoted protected-base bundles, least-privilege keys, append-only replay/tombstone/audit state, immutable evidence storage, and read-only incident reconstruction. A post-cutover deletion review must prove that every other ceremony route, credential, permission, scheduler, and temporary artifact has been removed without deleting the permanent tombstone or proof-use history.
