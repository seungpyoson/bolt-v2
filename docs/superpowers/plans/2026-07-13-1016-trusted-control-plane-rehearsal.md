# #1016 Trusted Control-Plane Rehearsal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build, run, measure, and completely dismantle a disposable rehearsal that proves the D3 GitHub App authority protocol against a private synthetic repository without implementing or changing production CI.

**Architecture:** A small Python harness owns canonical records, deterministic fixture results, scenario orchestration, evidence reconstruction, and cleanup verification. Live-only adapters use one authority GitHub App, one sandbox-operator GitHub App, one private disposable repository, Mergify, AWS Lambda, asymmetric AWS KMS signing, DynamoDB conditional append/state, and S3 Object Lock evidence. The operator App is installed only on the sandbox repository and can mutate rehearsal controls but is denied Checks write; an external read-only operator session verifies its eventual destruction from GitHub/AWS platform records.

**Tech Stack:** Python 3 standard library (`argparse`, `dataclasses`, `hashlib`, `hmac`, `http.client`, `json`, `sqlite3`, `tomllib`, `unittest`), `cryptography==48.0.1` with `cffi==2.0.0` and `pycparser==3.0` (versions verified in the approved local environment on 2026-07-13), GitHub REST/GraphQL APIs, Mergify, AWS Lambda/KMS/DynamoDB/S3 Object Lock, TOML, `just` cheap gates.

## Global Constraints

- This plan implements the disposable D4 rehearsal only; it does not implement the final verifier, precursor, activation, production publisher, production corpus, or production control changes.
- The fixture engine is policy-free: it selects only predeclared outcomes and never inspects candidate content to derive a verdict or `authority_surface_change`.
- Runtime repository, App, ruleset, Mergify, key, deployment, state, and evidence identifiers come from one untracked TOML instance document; no secret or runtime identifier is committed.
- The AWS deployment artifact uses a hash-locked dependency file and an immutable Lambda artifact digest. Offline receipt verification uses only exported KMS public keys and `cryptography`; it never requires AWS credentials or a live KMS call.
- The operator/fault principal and authority-service principal are distinct. The operator App is installed only on the exact sandbox repository, denied every production-denylist identity, and has no Checks write, authority state-writer, fixture-signing, or audit-signing capability. Its private key is a rehearsal-only SSM SecureString readable only by its exact execution role and is revoked/deleted during cleanup.
- The authority context is App-qualified; feedback, same-name Actions checks, commit statuses, other Apps, ordinary PR heads, and wrong installations never authorize.
- One logical authority service makes at most one Checks API create call per authority domain; ambiguous acceptance enters `PUBLISHING_UNCERTAIN` and cannot be replaced while the old head may merge.
- Local simulation cannot satisfy a live-matrix row involving App identity, rulesets, Mergify, Freeze, queue construction, bypass, invalidation, or merge-time re-evaluation.
- Evidence is append-only, hash-chained, signed, content-addressed, redacted, and reconstructable without mutable service state; reruns append new attempts and never overwrite history.
- No new human approval, quorum, hardware-key, backup-person, or emergency-success gate is introduced.
- No local compile-heavy Rust verification and no repeated full CI; use focused Python tests, `git diff --check`, `just ci-lint-workflow`, and `just source-fence-static` once at a coherent head.
- Any ambiguity in publisher qualification, stale-head ineligibility, lost-response reconciliation, replay closure, or cleanup is a D4 no-go, not a larger-budget candidate.

---

## File map and ownership

All executable rehearsal files live under `scripts/ci_rehearsal/` so the final cleanup PR can delete that directory, its tests, the one Just recipe, and the committed permission manifest as a single bounded slice. Tasks 1–7 have disjoint primary implementation files; only Task 8 integrates their public interfaces.

| Path | Responsibility | Owning task |
| --- | --- | --- |
| `ci/1016-rehearsal-resources.toml` | Committed permission/resource contract; contains no live IDs or secrets | 3 |
| `scripts/ci_rehearsal/cloudformation.yaml` | Reproducible AWS Lambda/KMS/DynamoDB/S3/SSM/IAM stack | 3 |
| `scripts/ci_rehearsal/provision.py` | Denylist-first preflight, GitHub App manifest flow, AWS deploy, and resource inventory | 3 |
| `scripts/ci_rehearsal/requirements.in` | Exact direct/transitive Python versions verified in the approved environment | 3 |
| `scripts/ci_rehearsal/requirements.lock` | Generated hash-locked wheels for the exact Lambda platform | 3 |
| `scripts/ci_rehearsal/model.py` | Closed enums and immutable protocol dataclasses | 1 |
| `scripts/ci_rehearsal/canonical.py` | Canonical JSON, digests, redaction, chain verification | 1 |
| `scripts/ci_rehearsal/config.py` | Load and validate the untracked instance TOML against the committed contract | 3 |
| `scripts/ci_rehearsal/fixture_engine.py` | Policy-free predeclared result signer | 2 |
| `scripts/ci_rehearsal/state.py` | Conditional append, uniqueness, reservation, tombstone and proof-use transitions | 4 |
| `scripts/ci_rehearsal/github.py` | Least-privilege GitHub observations and one-shot check creation | 5 |
| `scripts/ci_rehearsal/operator.py` | Separate sandbox-only control mutator; structurally incapable of check publication | 5 |
| `scripts/ci_rehearsal/faults.py` | Fault proxy for service-visible create/read delay, loss and duplication | 5 |
| `scripts/ci_rehearsal/evidence.py` | Content-addressed evidence writer and offline reconstruction | 6 |
| `scripts/ci_rehearsal/scenarios.py` | H1–H10, ceremony, abort and fault scenario definitions | 7 |
| `scripts/ci_rehearsal/service.py` | Ephemeral webhook ingress and trigger-only delivery deduplication | 8 |
| `scripts/ci_rehearsal/driver.py` | CLI orchestration only; no policy decisions | 8 |
| `scripts/ci_rehearsal/cleanup.py` | Resource teardown plan and post-cleanup negative probes | 9 |
| `scripts/ci_rehearsal/report.py` | Raw distributions and owner-facing D4/D5 report | 10 |
| `scripts/test_ci_rehearsal_*.py` | Focused unit/contract tests corresponding to each owner | 1–10 |
| `justfile` | One public `ci-rehearsal` dispatcher recipe, removed with the harness | 8 |

### Task 1: Canonical Protocol and Audit Primitives

**Files:**
- Create: `scripts/ci_rehearsal/__init__.py`
- Create: `scripts/ci_rehearsal/model.py`
- Create: `scripts/ci_rehearsal/canonical.py`
- Create: `scripts/test_ci_rehearsal_protocol.py`

**Interfaces:**
- Consumes: no rehearsal code.
- Produces: `Purpose`, `TerminalClass`, `AttemptState`, `Invocation`, `FixtureResult`, `canonical_bytes(value) -> bytes`, `digest(value) -> str`, `redact(value) -> object`, and `verify_chain(records) -> None`.

- [ ] **Step 1: Write failing closed-schema and canonicalization tests**

```python
def test_invocation_rejects_unknown_and_missing_fields() -> None:
    raw = complete_invocation_dict()
    with self.assertRaisesRegex(ProtocolError, "unknown fields"):
        Invocation.from_dict({**raw, "candidate_verdict": "allow"})
    raw.pop("proof_head_sha")
    with self.assertRaisesRegex(ProtocolError, "missing fields"):
        Invocation.from_dict(raw)

def test_canonical_bytes_are_order_independent_and_reject_floats() -> None:
    self.assertEqual(canonical_bytes({"b": 2, "a": 1}), b'{"a":1,"b":2}')
    with self.assertRaisesRegex(ProtocolError, "floating point"):
        canonical_bytes({"elapsed": 1.2})
```

- [ ] **Step 2: Run the protocol test and observe the expected failure**

Run: `python3 scripts/test_ci_rehearsal_protocol.py`

Expected: `ModuleNotFoundError: No module named 'ci_rehearsal'`.

- [ ] **Step 3: Implement closed dataclasses and canonical primitives**

Define exact string enums `Purpose = {activation, steady-state, canary}`, `TerminalClass = {allow, deny, malformed, timeout, infrastructure, terminal}`, and the D3 attempt states through `PUBLISHED_VALIDATED`, `PUBLISHING_UNCERTAIN`, `SUPERSEDED_NONPUBLISHABLE`, and `TERMINAL_ABANDONED`. `Invocation.from_dict` must compare `set(raw)` to its explicit field set, require 40-character lowercase hex Git SHAs and 64-character lowercase SHA-256 digests, require `attempt_number >= 1`, and prohibit `authority_surface_change` or any verdict field. `FixtureResult.from_dict` must require a real boolean for `authority_surface_change`, a closed terminal class, ordered string finding digests, invocation digest, key version, purpose, and signature digest.

Implement `canonical_bytes` recursively for `None`, booleans, integers, strings, lists, and string-keyed dictionaries with UTF-8, sorted keys, `ensure_ascii=False`, and separators `(',', ':')`; reject floats, bytes, duplicate keys supplied through parsed-pair input, non-string keys, and unknown Python types. Implement `redact` as an allowlist of schema fields rather than a substring scrubber. `verify_chain` must recompute every `sequence`, `prior_hash`, `record_hash`, and signature reference and reject gaps, forks, altered records, and trailing omission relative to the signed checkpoint.

- [ ] **Step 4: Run focused protocol tests**

Run: `python3 scripts/test_ci_rehearsal_protocol.py`

Expected: all tests pass, including malformed UTF-8, duplicate JSON key, null substitution, noncanonical bytes, changed order, missing sequence, and fork cases.

- [ ] **Step 5: Commit the independently reviewable protocol slice**

```bash
git add scripts/ci_rehearsal/__init__.py scripts/ci_rehearsal/model.py scripts/ci_rehearsal/canonical.py scripts/test_ci_rehearsal_protocol.py
git commit -m "test: define disposable authority protocol"
```

### Task 2: Policy-Free Fixture Engine and Signing Boundary

**Files:**
- Create: `scripts/ci_rehearsal/fixture_engine.py`
- Create: `scripts/test_ci_rehearsal_fixture_engine.py`

**Interfaces:**
- Consumes: `Invocation`, `FixtureResult`, `canonical_bytes`, `digest` from Task 1; a `Signer` protocol with `key_version: str` and `sign(payload: bytes, purpose: Purpose) -> bytes`.
- Produces: `FixtureSpec`, `SignedFixtureResult(result: FixtureResult, signature: bytes)`, `Signer`, `verify_signed_result(invocation, result, signature, verification_key) -> None`, and `FixtureEngine.run(invocation: Invocation, fixture_id: str) -> SignedFixtureResult`; fixtures `allow_false`, `allow_true`, `deny`, `malformed`, and `timeout`.

- [ ] **Step 1: Write tests proving result ownership and purpose binding**

```python
def test_publisher_cannot_override_signed_classification() -> None:
    signed = engine.run(invocation(Purpose.STEADY_STATE), "allow_false")
    mutated = replace(signed.result, authority_surface_change=True)
    with self.assertRaisesRegex(SignatureError, "result signature"):
        verify_signed_result(invocation, mutated, signed.signature, signer.public_key())

def test_true_is_owned_by_fixture_and_bound_to_activation() -> None:
    with self.assertRaisesRegex(FixtureError, "purpose"):
        engine.run(invocation(Purpose.STEADY_STATE), "allow_true")
    self.assertTrue(engine.run(invocation(Purpose.ACTIVATION), "allow_true").result.authority_surface_change)

def test_kms_verification_rejects_wrong_protocol_parameters() -> None:
    for mutation in (wrong_curve, wrong_algorithm, wrong_digest, malformed_der, wrong_key_version, wrong_purpose):
        with self.subTest(mutation=mutation.__name__), self.assertRaises(SignatureError):
            verify_kms_signature(**mutation(valid_signature_case()))
```

- [ ] **Step 2: Verify the tests fail before implementation**

Run: `python3 scripts/test_ci_rehearsal_fixture_engine.py`

Expected: import failure for `ci_rehearsal.fixture_engine`.

- [ ] **Step 3: Implement the fixed fixture table and signing adapter**

The committed fixture table is data, not repository policy: `allow_false=(allow, False, steady-state)`, `allow_true=(allow, True, activation)`, `deny=(deny, False, both authorizing purposes)`, `malformed` emits deliberately noncanonical bytes, and `timeout` raises `FixtureTimeout` before signing. The payload has exactly four fields: `invocation_digest`, `canonical_result_digest`, `engine_result_key_version`, and `purpose`; sign the SHA-256 digest of its canonical bytes. Use `HmacTestSigner` only inside isolated unit tests. Both live KMS keys are `KeySpec=ECC_NIST_P256`, `KeyUsage=SIGN_VERIFY`; `KmsSigner` calls `Sign(SigningAlgorithm="ECDSA_SHA_256", MessageType="DIGEST", Message=digest_bytes)`, where `digest_bytes` is exactly 32 bytes. AWS returns an ASN.1 DER ECDSA signature. Offline verification loads the DER SPKI public key, requires EC P-256, decodes/re-encodes DER strictly, validates `1 <= r,s < curve_order`, accepts mathematically valid high-S or low-S because `cryptography` verification does, and never normalizes signature bytes before digesting/storing them. It calls `ec.ECDSA(utils.Prehashed(hashes.SHA256()))` over the same 32-byte digest. The key ARN/version, public-key digest, curve, algorithm, purpose, and canonical-payload digest are all bound to the run plan and record. Reject wrong curve, algorithm, digest length/value, noncanonical/malformed DER, invalid r/s, key version, public-key digest, or purpose. Export `GetPublicKey` once, then verify every fixture/audit record offline; a verification path that silently calls KMS is rejected.

- [ ] **Step 4: Run engine and protocol tests**

Run: `python3 scripts/test_ci_rehearsal_fixture_engine.py && python3 scripts/test_ci_rehearsal_protocol.py`

Expected: both programs report success; unsigned changes, wrong purpose, wrong invocation, wrong key version, extra output, malformed bytes, and timeout all fail closed.

- [ ] **Step 5: Commit the fixture-engine slice**

```bash
git add scripts/ci_rehearsal/fixture_engine.py scripts/test_ci_rehearsal_fixture_engine.py
git commit -m "test: add policy-free rehearsal fixtures"
```

### Task 3: Disposable Resource Contract and Instance Configuration

**Files:**
- Create: `ci/1016-rehearsal-resources.toml`
- Create: `scripts/ci_rehearsal/cloudformation.yaml`
- Create: `scripts/ci_rehearsal/provision.py`
- Create: `scripts/ci_rehearsal/requirements.in`
- Create: `scripts/ci_rehearsal/requirements.lock`
- Create: `scripts/ci_rehearsal/config.py`
- Create: `scripts/test_ci_rehearsal_config.py`

**Interfaces:**
- Consumes: canonical digest helpers from Task 1.
- Produces: `ResourceContract`, `InstanceConfig`, `load_contract(path)`, `load_instance(path, contract)`, and `render_preflight(config) -> dict[str, object]`.

- [ ] **Step 1: Write configuration rejection tests**

```python
def test_instance_rejects_production_repository_and_inline_secret(tmp_path: Path) -> None:
    path = write_instance(tmp_path, repository="seungpyoson/bolt-v2", app_private_key="secret")
    with self.assertRaisesRegex(ConfigError, "production repository|unknown fields"):
        load_instance(path, contract())

def test_contract_denies_mutating_app_permissions() -> None:
    self.assertEqual(contract().app_permissions["checks"], "write")
    self.assertEqual(contract().app_permissions["contents"], "read")
    self.assertNotIn("administration_write", contract().app_permissions)

def test_aws_authority_and_operator_roles_are_disjoint() -> None:
    self.assertFalse(contract().authority_actions & contract().operator_actions)
    self.assertNotIn("checks:write", contract().operator_capabilities)
    self.assertEqual(contract().aws_services, {"lambda", "kms", "dynamodb", "s3"})

def test_preflight_blocks_every_create_until_ids_are_separate() -> None:
    with self.assertRaisesRegex(ProvisionError, "production denylist"):
        provision.preflight(instance_with_repository_node_id(PRODUCTION_NODE_ID))
    self.assertEqual(provider.create_calls, [])
```

- [ ] **Step 2: Run and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_config.py`

Expected: import failure for `ci_rehearsal.config`.

- [ ] **Step 3: Add the exact committed resource contract**

The TOML must declare: private sandbox repository only; protected ref `main`; contexts `trusted-ci-verifier-rehearsal` and `trusted-ci-verifier-rehearsal-feedback`; authority App permissions Checks write plus required reads and no repository mutation; and a separate operator App installed only on the exact sandbox repository node ID with `metadata: read`, `administration: write`, `contents: write`, and `pull_requests: write`, but no Checks permission at all. Operator Contents writes own synthetic refs and `.mergify.yml`; Mergify configuration changes occur only through those sandbox commits, not a broad PAT or unspecified Mergify API token. Declare Lambda authority/observer deployments, two ECC KMS keys, DynamoDB conditional append, S3 Object Lock, exact SSM parameter ARNs for the two rehearsal App private keys, and cleanup probes. Each Lambda execution role may read only its exact SSM parameter; operator IAM is limited to exact sandbox resources/fault fixtures and cannot access authority KMS, DynamoDB authority writes, S3 evidence writes, or the authority App parameter. Both Apps/roles deny every identity in the production denylist.

The committed `[run_plan]` is deliberately small and makes no statistical claim: two clean full ceremonies, two lost-create/delayed-read runs, two pre-precursor abort/restore runs at each relevant cut point, and one run of every deterministic negative row. It reports every raw observation plus min/max where a row has two samples; it does not compute or claim p90, p95, convergence, confidence, or operational budgets. The exact expanded order, fault seeds, and repetition counts are signed before the first live mutation. Additional runs require a separate owner-approved worst-case cost/time ceiling and a new immutable run-plan digest; they never rewrite the first run's evidence.

- [ ] **Step 4: Add denylist-first reproducible provisioning**

`cloudformation.yaml` defines the exact Lambda functions/roles, ECC_NIST_P256 KMS keys, DynamoDB table, S3 bucket with versioning/Object Lock, SSM resource policies/paths, log groups, and outputs. It accepts only `RunId`, `SandboxRepositoryNodeId`, `SandboxRepositoryFullName`, and `ProductionDenylistDigest`; IAM resources use those exact values and explicit denies.

Before any create call, run:

```bash
python3 -m scripts.ci_rehearsal.provision preflight \
  --instance /private/tmp/bolt-v2-1016-d4-instance.toml \
  --production-denylist /private/tmp/bolt-v2-1016-production-denylist.toml
```

Preflight performs read-only `GET /repos/{owner}/{repo}` and AWS STS `GetCallerIdentity`, loads and digests the production denylist, requires the already-existing sandbox repository to be private/empty and its node ID/full name/owner to differ from every production entry, requires the sandbox AWS account/region/resource prefix to differ from every production entry, and writes a canonical SHA-256-bound local preflight receipt (the KMS audit key does not exist yet). Its provider interface exposes no create method until that exact receipt is supplied.

Then deploy reproducibly:

```bash
python3 -m scripts.ci_rehearsal.provision aws-deploy \
  --template scripts/ci_rehearsal/cloudformation.yaml \
  --instance /private/tmp/bolt-v2-1016-d4-instance.toml \
  --preflight-receipt "$D4_PROVISION_PREFLIGHT_ROOT"
```

The command uses CloudFormation `CreateChangeSet`, `DescribeChangeSet`, `ExecuteChangeSet`, and `DescribeStacks`, records the exact template/change-set digests, and refuses replacements or resources outside the run prefix. Create each App through GitHub's App Manifest flow with `provision github-app --kind authority` and `--kind operator`; the local callback exchanges the one-time code at `POST /app-manifests/{code}/conversions`, validates the returned permission manifest, sends the private key directly to the exact SSM SecureString parameter, never logs or persists the response, and exits after printing only App/installation IDs and digests. Install each App solely on the exact sandbox repository and verify the operator App has no Checks permission. No broad PAT is accepted.

- [ ] **Step 5: Pin the verification dependency and immutable Lambda base**

Create `requirements.in` with exactly:

```text
cryptography==48.0.1
cffi==2.0.0
pycparser==3.0
```

Generate `requirements.lock` for the selected Lambda Python platform with one `--hash=sha256:...` entry for every allowed wheel, using only artifacts downloaded from the approved package index. Before implementation continues, select an AWS-supported Lambda Python base image, record its immutable image digest, and execute `python -c 'import boto3,botocore; print(boto3.__version__, botocore.__version__)'` inside that exact image. If the embedded SDK versions or image digest cannot be verified and recorded, stop: do not invent versions or use a floating Lambda tag. Package `cryptography`, `cffi`, and `pycparser` from the hash-locked wheels; the AWS SDK remains the version proven inside the immutable base image and the service asserts those exact observed versions at startup.

- [ ] **Step 6: Implement strict instance loading**

The untracked instance TOML path is mandatory via `--instance /absolute/path.toml`; reject paths inside the repository, group/world-readable mode, inline private keys/tokens/passwords, unknown fields, non-private repositories, missing production denylist, identical test/production node IDs, non-HTTPS service endpoints, non-rehearsal context names, mutable evidence retention, or permission drift. Credential values are managed resource handles only. `render_preflight` prints identifiers, permissions, digests, and negative tests but never secret material.

- [ ] **Step 7: Run configuration/provisioning tests and a secret-pattern scan**

Run: `python3 scripts/test_ci_rehearsal_config.py`

Expected: all positive and negative cases pass.

Run: `rg -n '(BEGIN .*PRIVATE KEY|github_pat_|ghp_|secret\s*=|token\s*=)' ci/1016-rehearsal-resources.toml scripts/ci_rehearsal scripts/test_ci_rehearsal_config.py`

Expected: no matches.

- [ ] **Step 8: Commit the resource-contract slice**

```bash
git add ci/1016-rehearsal-resources.toml scripts/ci_rehearsal/cloudformation.yaml scripts/ci_rehearsal/provision.py scripts/ci_rehearsal/requirements.in scripts/ci_rehearsal/requirements.lock scripts/ci_rehearsal/config.py scripts/test_ci_rehearsal_config.py
git commit -m "test: define disposable rehearsal resources"
```

### Task 4: Conditional Append-Only State Machine

**Files:**
- Create: `scripts/ci_rehearsal/state.py`
- Create: `scripts/test_ci_rehearsal_state.py`

**Interfaces:**
- Consumes: Task 1 records and digests; `AppendBackend.compare_and_append(expected_sequence: int, expected_hash: str, record: bytes) -> AppendReceipt`.
- Produces: `AppendReceipt(accepted: bool, sequence: int, record_hash: str)`, `AuthorityDomain`, `AuthorityState.create_attempt`, `complete_attempt`, `reserve_publication`, `mark_ready`, `mark_uncertain`, `adopt_publication`, `supersede_nonpublishable`, `append_tombstone`, and `checkpoint`.

- [ ] **Step 1: Write race, replay, rollback, and uncertainty tests**

```python
def test_concurrent_reservations_admit_exactly_one() -> None:
    results = race(lambda: state.reserve_publication(domain, nonce))
    self.assertEqual(sum(result.accepted for result in results), 1)

def test_uncertain_domain_cannot_retry_or_be_superseded_without_live_proof() -> None:
    state.mark_uncertain(domain, create_attempt_digest)
    with self.assertRaisesRegex(StateError, "merge-ineligibility proof"):
        state.create_attempt(replace(domain, proof_head_sha=NEW_HEAD), fresh_nonce())
```

- [ ] **Step 2: Run tests and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_state.py`

Expected: import failure for `ci_rehearsal.state`.

- [ ] **Step 3: Implement transitions with backend-enforced uniqueness**

Use the exact nonce-independent authority-domain key from D3. Every transition is a new record; there is no update/delete API. Require one terminal authorizing record and one reservation maximum, monotonic sequence/hash, unique delivery IDs, unique nonces, immutable purpose and tuple digest, and a higher-generation tombstone that permanently disables bootstrap. `PUBLISHING_UNCERTAIN` accepts only exact matching-check adoption or a separately appended, independently observed inability-to-merge proof before supersession. Retry-budget exhaustion never supplies that proof.

- [ ] **Step 4: Add local SQLite test backend and live conditional-backend contract suite**

The SQLite backend is test-only, uses `BEGIN IMMEDIATE`, a unique domain index, insert-only triggers, and an external signed checkpoint fixture to detect restored snapshots. The live backend implements the same contract against the selected disposable managed store. Run the identical backend contract tests against SQLite locally and against the live store only during sandbox preflight.

- [ ] **Step 5: Run the state suite**

Run: `python3 scripts/test_ci_rehearsal_state.py`

Expected: success for duplicate deliveries, two simulated replicas, restart, restored snapshot, old key/binary, repeated nonce, same-SHA rerun, concurrent reservation, tombstone, uncertain adoption, invalidation, chain damage, and terminal-domain closure.

- [ ] **Step 6: Commit the state slice**

```bash
git add scripts/ci_rehearsal/state.py scripts/test_ci_rehearsal_state.py
git commit -m "test: add rehearsal append-only authority state"
```

### Task 5: GitHub Observation and One-Shot Publisher Adapter

**Files:**
- Create: `scripts/ci_rehearsal/github.py`
- Create: `scripts/ci_rehearsal/operator.py`
- Create: `scripts/ci_rehearsal/faults.py`
- Create: `scripts/test_ci_rehearsal_github.py`

**Interfaces:**
- Consumes: `InstanceConfig`; `HttpTransport.request(method, url, headers, body) -> HttpResponse`.
- Produces: `GitHubObserver.observe_domain`, `observe_terminal`, `observe_merge_ineligibility`, `list_exact_checks`, `GitHubPublisher.create_once`, `GitHubPublisher.reconcile`; `SandboxOperator.apply(operation: OperatorOperation) -> MutationReceipt`; `OwnerPlatformStopControl.preflight() -> StopControlReceipt` and `disable(installation_id: int) -> StopControlReceipt`; and `FaultProxy.arm(fault: FaultSpec) -> FaultReceipt`.

- [ ] **Step 1: Write scripted-transport tests for lost responses and identity**

```python
def test_create_once_never_retries_ambiguous_acceptance() -> None:
    transport.queue(TimeoutAfterSend(), exact_check(app_id=APP_ID, external_id=DOMAIN))
    with self.assertRaises(PublishingUncertain):
        publisher.create_once(publication)
    self.assertEqual(transport.count("POST", "/check-runs"), 1)
    self.assertEqual(publisher.reconcile(publication).external_id, DOMAIN)

def test_same_name_wrong_app_is_conflict_not_success() -> None:
    transport.queue(checks(same_name(app_id=OTHER_APP)))
    with self.assertRaisesRegex(GitHubConflict, "publisher identity"):
        observer.list_exact_checks(domain)

def test_operator_cannot_publish_or_escape_allowlist() -> None:
    self.assertNotIn("create_check", operator.capabilities)
    with self.assertRaisesRegex(OperatorDenied, "production denylist"):
        operator.apply(change_ruleset(repository_node_id=PRODUCTION_NODE_ID))

def test_fault_proxy_changes_observation_not_github_identity() -> None:
    receipt = proxy.arm(delay_check_read(domain=DOMAIN, observations=3))
    self.assertEqual(receipt.scope, (SANDBOX_NODE_ID, DOMAIN))
    self.assertNotIn("forge_app_identity", proxy.capabilities)

def test_owner_stop_control_can_only_remove_installation() -> None:
    self.assertEqual(stop_control.operations, {"delete-user-installation"})
    self.assertFalse({"create-check", "approve", "restore", "rotate"} & stop_control.operations)
    self.assertNotIn("disable-authority-installation", operator.operations)
```

- [ ] **Step 2: Verify tests fail before the adapter exists**

Run: `python3 scripts/test_ci_rehearsal_github.py`

Expected: import failure for `ci_rehearsal.github`.

- [ ] **Step 3: Implement policy-free observation**

Fetch repository node ID, ref/object/tree/parents, PR constituents, reviews, ruleset payload and qualified App source, installation identity, `.mergify.yml` blob, queue head, checks, and merge result. Normalize only structural identities and digests; do not inspect repository policy or infer `authority_surface_change`. Terminal observation must repeat every publication identity immediately before reservation, before create, after create, and before merge.

- [ ] **Step 4: Implement one-shot create and exact reconciliation**

`create_once` requires a durable `READY_TO_PUBLISH` receipt and sends one POST with context, exact proof SHA, completed/success conclusion, and authority-domain digest as `external_id`. On timeout, connection loss, ambiguous status, or unreadable response it returns `PUBLISHING_UNCERTAIN` and never POSTs again. `reconcile` filters by exact SHA, context, App ID, installation identity where exposed, and `external_id`; zero matches remains uncertain, one match is adoptable, and duplicates/mismatches are terminal conflicts.

- [ ] **Step 5: Implement the separate operator and fault boundaries**

`SandboxOperator` mints short-lived installation tokens from its dedicated operator App key in the exact SSM SecureString parameter; its execution role can read only that parameter. The App is installed solely on the allowlisted sandbox repository and has `administration: write`, `contents: write`, `pull_requests: write`, and metadata read, with Checks explicitly absent. A closed `OperatorOperation` enum covers synthetic ref/config commits, PR creation, queue admission/removal, sandbox ruleset changes, `.mergify.yml` route/bypass/Freeze/Merge Protections/exclusion commits, and cleanup other than H9 authority-App disable. Each request must match the exact sandbox repository node ID/name and resource ID and must not match any production denylist entry. It has no authority-App private-key handle or token path, trusted-context Checks method, DynamoDB authority writer, KMS signer, or S3 evidence writer.

H9 uses a separate, already-existing GitHub repository/App owner session outside both rehearsal Apps, Lambdas, IAM roles, SSM parameters, and services. No credential from that session is stored in the harness or added as a new approval gate. Before the run, `OwnerPlatformStopControl.preflight` calls `GET /user` and `GET /user/installations`, proves the exact owner principal can see the authority installation on only the sandbox repository, and records the principal ID, installation ID, repository node ID, endpoint, timestamp, request ID, and response digest without credentials. Its sole mutation is `DELETE /user/installations/{installation_id}` for that exact preflight-bound installation. The closed interface exposes no check creation, approval, result change, restore, bootstrap, epoch/key rotation, or alternate publisher. H9 captures the deletion response and independent `GET /repos/{owner}/{repo}/installation` not-found observation as platform evidence; failure to prove this exact stop-only path before scenarios is `NO_GO`.

`FaultProxy` sits between the authority service and its Checks create/read transport and may delay or drop a create response, hide a bounded number of reads, return a proven pre-acceptance failure, duplicate inbound webhook/rerequest delivery, or cancel the disposable invocation; it cannot edit GitHub responses, forge App identity, alter signed results, or write a check itself. Every arm/disarm is append-only evidence and scoped to one run/scenario/domain.

- [ ] **Step 6: Run GitHub/operator/fault adapter tests**

Run: `python3 scripts/test_ci_rehearsal_github.py`

Expected: all fixture-response tests pass, including wrong repository, SHA, App, installation, context, purpose, ruleset binding, duplicate run, status API, ordinary PR head, delayed visibility, moved ref, and insufficient permissions.

- [ ] **Step 7: Commit the GitHub adapter slice**

```bash
git add scripts/ci_rehearsal/github.py scripts/ci_rehearsal/operator.py scripts/ci_rehearsal/faults.py scripts/test_ci_rehearsal_github.py
git commit -m "test: add one-shot rehearsal check publisher"
```

### Task 6: Immutable Evidence and Independent Reconstruction

**Files:**
- Create: `scripts/ci_rehearsal/evidence.py`
- Create: `scripts/test_ci_rehearsal_evidence.py`

**Interfaces:**
- Consumes: Task 1 canonical/chain functions and Task 4 append receipts; `ObjectStore.put_if_absent(digest, bytes) -> ObjectReceipt` and `get(digest) -> bytes`.
- Produces: `ObjectReceipt(created: bool, digest: str)`, `ReconstructionReport(complete: bool, outcome: str, discrepancies: tuple[str, ...])`, `EvidenceWriter.append_event`, `store_blob`, `seal_scenario`, and `Reconstructor.reconstruct(bundle_root) -> ReconstructionReport`.

- [ ] **Step 1: Write reconstruction and damage tests**

```python
def test_reconstructs_without_mutable_service_state() -> None:
    bundle = fixture_bundle("allow")
    report = Reconstructor(store.public_reader()).reconstruct(bundle.root_digest)
    self.assertTrue(report.complete)
    self.assertEqual(report.outcome, "merged")

def test_offline_reconstruction_never_calls_aws_or_github() -> None:
    report = Reconstructor(exported_objects(), exported_public_keys()).reconstruct(ROOT)
    self.assertTrue(report.complete)
    self.assertEqual(network.calls, [])

def test_every_damage_class_is_detected() -> None:
    for damage in (delete, alter, reorder, fork, rollback, hide_blob, expire_copy):
        with self.subTest(damage=damage.__name__), self.assertRaises(EvidenceError):
            Reconstructor(damage(fixture_store())).reconstruct(ROOT)
```

- [ ] **Step 2: Run and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_evidence.py`

Expected: import failure for `ci_rehearsal.evidence`.

- [ ] **Step 3: Implement allowlisted evidence events**

Store ordered API metadata without authorization headers or response bodies containing secrets; include delivery IDs, monotonic/external timestamps, state sequence/hash, request/response status and digest, fault controls, fixture/result/signature digests, `external_id`, check identity, raw integer nanosecond durations, retry/resource/API/storage/cost observations, expected/actual outcome, discrepancies, terminal class, and cleanup state. Large bodies are immutable objects keyed by SHA-256. Seal each scenario with a signed root over ordered event and blob digests.

- [ ] **Step 4: Implement standalone reconstruction**

The reconstructor accepts only a read-only S3 evidence export plus DER KMS public keys and their run-plan digests. It verifies signatures locally with the pinned `cryptography` dependency, chain order, checkpoints, all blob references, scenario identity, expected-versus-actual terminal result, and completeness markers; its transport raises on every network operation, so it cannot call KMS, AWS, GitHub, DynamoDB, or the authority service and cannot reinterpret semantic findings.

- [ ] **Step 5: Run evidence tests**

Run: `python3 scripts/test_ci_rehearsal_evidence.py`

Expected: allow, deny, spoof, moved-state, retry, uncertain-publication, invalidation, terminal, stop-only, audit-damage, and cleanup fixture bundles reconstruct; each damage mutation fails.

- [ ] **Step 6: Commit the evidence slice**

```bash
git add scripts/ci_rehearsal/evidence.py scripts/test_ci_rehearsal_evidence.py
git commit -m "test: add reconstructable rehearsal evidence"
```

### Task 7: Closed Scenario Matrix, Ceremony, Abort, and Fault Scripts

**Files:**
- Create: `scripts/ci_rehearsal/scenarios.py`
- Create: `scripts/test_ci_rehearsal_scenarios.py`

**Interfaces:**
- Consumes: Task 1 enums only; runtime actions are injected as a `ScenarioActions` protocol.
- Produces: `SCENARIOS`, `HYPOTHESES`, `CUT_POINTS`, `ScenarioRunner.run(scenario_id) -> ScenarioReceipt`.

- [ ] **Step 1: Write exact matrix-coverage tests**

```python
def test_matrix_covers_every_required_row_and_hypothesis() -> None:
    self.assertEqual(set(HYPOTHESES), {f"H{n}" for n in range(1, 11)})
    self.assertEqual(set(SCENARIOS), EXPECTED_SCENARIO_IDS)
    covered = {hypothesis for scenario in SCENARIOS.values() for hypothesis in scenario.hypotheses}
    self.assertEqual(covered, set(HYPOTHESES))

def test_stale_proof_runs_at_every_cut_point() -> None:
    self.assertEqual(set(CUT_POINTS), {"before-reservation", "before-create", "during-delayed-visibility", "after-lost-response", "after-create", "before-merge"})

def test_run_plan_is_immutable_and_small() -> None:
    self.assertEqual(RUN_PLAN.full_ceremony_repetitions, 2)
    self.assertEqual(RUN_PLAN.deterministic_negative_repetitions, 1)
    with self.assertRaisesRegex(ScenarioError, "run-plan digest"):
        ScenarioRunner(replace(RUN_PLAN, full_ceremony_repetitions=3), signed_run_plan_digest=RUN_PLAN_DIGEST)
```

- [ ] **Step 2: Run and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_scenarios.py`

Expected: import failure for `ci_rehearsal.scenarios`.

- [ ] **Step 3: Encode all D4 scenario rows as closed data**

Define exactly these IDs: `baseline-allow`, `full-ceremony`, `pre-precursor-abort`, `baseline-deny`, `same-name-spoof`, `wrong-identity`, `base-movement`, `constituent-movement`, `mergify-self-change`, `exempt-matrix`, `duplicate-race`, `lost-create-response`, `create-failure-before-acceptance`, `uncertain-invalidation`, `retry-allowlist`, `terminal-cases`, `classification-ownership`, `stop-only-disable`, `audit-damage`, and `cleanup`. Each definition names H1–H10 coverage, required live/local mode, fresh synthetic branch/domain requirement, ordered actions, expected terminal state, prohibited transitions, evidence fields, and cleanup obligations.

Use this exact minimum hypothesis mapping (scenarios may cite additional hypotheses): H1 → `same-name-spoof`; H2 → `exempt-matrix`; H3 → `base-movement` and `constituent-movement`; H4 → `same-name-spoof`; H5 → `duplicate-race`; H6 → `lost-create-response`; H7 → `uncertain-invalidation`; H8 → `classification-ownership`; H9 → `stop-only-disable`; H10 → `audit-damage` and `cleanup`. The aggregate report rejects a mapping that omits any of these required associations.

- [ ] **Step 4: Encode the full ceremony and abort sequences**

`full-ceremony` must execute admission lock, post-lock precursor refresh, final rules/Mergify/Freeze controls, dormant precursor analogue, promotion and irreversible tombstone, nonpublishing canary, activation queue/proof head, terminal proof, publication, merge observation, and cleanup. `pre-precursor-abort` runs at every cut point after lock and before precursor analogue, restores baseline controls and merge availability, and proves no check, promotion, or tombstone. Only `ScenarioActions` performs mutations, and every action is paired with before/after observation and an explicit inverse allowed only before precursor.

Expand the committed small run plan into ordered run instances before live execution. Sign its digest with the audit key and bind it into every scenario receipt. The driver may not change scenario order, seeds, repetitions, or drop observations after seeing results. Any expansion stops the run until the owner approves a worst-case additional cost/time ceiling, then creates a new signed plan and separate evidence lineage. Repetition demonstrates basic repeatability only and does not set or statistically justify a production retry, outage, or abort budget.

- [ ] **Step 5: Run matrix tests**

Run: `python3 scripts/test_ci_rehearsal_scenarios.py`

Expected: exact scenario/hypothesis/cut-point coverage passes; attempts to omit cleanup, reuse a domain/success, publish from canary/abort, or attach an inverse after precursor fail validation.

- [ ] **Step 6: Commit the scenario-definition slice**

```bash
git add scripts/ci_rehearsal/scenarios.py scripts/test_ci_rehearsal_scenarios.py
git commit -m "test: encode trusted-control rehearsal matrix"
```

### Task 8: Integrate the Authority Driver and Cheap Local Gate

**Files:**
- Create: `scripts/ci_rehearsal/driver.py`
- Create: `scripts/ci_rehearsal/service.py`
- Create: `scripts/test_ci_rehearsal_driver.py`
- Modify: `justfile`

**Interfaces:**
- Consumes: all Tasks 1–7 interfaces plus injected live adapters.
- Produces: `WebhookService.handle(headers: Mapping[str, str], body: bytes) -> HttpResponse`, `python3 -m scripts.ci_rehearsal.driver {serve,preflight,local,live,aggregate,reconstruct,report}` and `just ci-rehearsal ...`; live subcommands include `cleanup-authority` and `cleanup-observer`.

- [ ] **Step 1: Write end-to-end fake-adapter tests**

```python
def test_allow_orders_terminal_query_reservation_create_and_validation() -> None:
    receipt = driver.run("baseline-allow", actions=fake_actions())
    self.assertEqual(receipt.actions, ["observe", "invoke", "complete", "terminal-query-1", "reserve", "terminal-query-2", "create-once", "validate", "merge-observe", "seal"])

def test_live_mode_requires_explicit_sandbox_identity() -> None:
    with self.assertRaisesRegex(DriverError, "preflight receipt"):
        driver.main(["live", "baseline-allow", "--instance", INSTANCE])

def test_webhook_is_trigger_only_and_delivery_is_unique() -> None:
    response = service.handle(valid_github_headers(delivery="d1"), fixture_webhook())
    self.assertEqual(response.status, 202)
    self.assertEqual(state.deliveries(), ("d1",))
    self.assertEqual(service.handle(valid_github_headers(delivery="d1"), fixture_webhook()).status, 200)
    self.assertEqual(publisher.create_calls, 0)
```

- [ ] **Step 2: Run and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_driver.py`

Expected: import failure for `ci_rehearsal.driver`.

- [ ] **Step 3: Implement the orchestration-only driver**

`serve` exposes only `POST /github/webhook` and `GET /health`; it validates the GitHub webhook signature through the managed verification boundary, conditionally appends the unique delivery ID, and enqueues an internal trigger without treating event bytes as authority evidence. `preflight` validates the instance, verifies exact App permissions and negative endpoints, confirms the repository is private/synthetic and not in the production denylist, records ruleset/Mergify baselines, tests managed sign/verify, conditional append, immutable put, and service health, then seals a preflight receipt. `local` runs only scenarios declared local. `live SCENARIO` requires the exact unexpired preflight digest, clean harness revision, fresh scenario/domain, and explicit `--confirm-sandbox-node-id` equal to the observed test repository node ID. `reconstruct` invokes only the offline reader. The driver contains no scenario-specific policy branches beyond dispatch through `SCENARIOS`.

- [ ] **Step 4: Add one public Just recipe**

```make
ci-rehearsal *args:
    python3 -m scripts.ci_rehearsal.driver {{args}}
```

Do not add it to `CI_LINT_SUITES`, source-fence standalone lists, workflows, required checks, or production lanes. The plan's execution PR is a rehearsal tool slice, not a permanent CI registration.

- [ ] **Step 5: Run the complete local harness once**

Run: `python3 scripts/test_ci_rehearsal_protocol.py && python3 scripts/test_ci_rehearsal_fixture_engine.py && python3 scripts/test_ci_rehearsal_config.py && python3 scripts/test_ci_rehearsal_state.py && python3 scripts/test_ci_rehearsal_github.py && python3 scripts/test_ci_rehearsal_evidence.py && python3 scripts/test_ci_rehearsal_scenarios.py && python3 scripts/test_ci_rehearsal_driver.py`

Expected: all focused suites pass with no network access.

Run: `just ci-rehearsal local --all`

Expected: local protocol, append, crash, replay, race, uncertainty, audit-damage, and reconstruction rows seal passing receipts; every live-only row reports `NOT_RUN_LIVE_REQUIRED`, never `PASS`.

- [ ] **Step 6: Run cheap repository checks once at the coherent head**

Run: `git diff --check && just ci-lint-workflow && just source-fence-static`

Expected: all commands exit 0; no Rust compilation or full CI is invoked.

- [ ] **Step 7: Commit the integrated local harness**

```bash
git add scripts/ci_rehearsal/driver.py scripts/ci_rehearsal/service.py scripts/test_ci_rehearsal_driver.py justfile
git commit -m "test: integrate disposable control-plane rehearsal"
```

### Task 9: Cleanup Executor and Destruction Proof

**Files:**
- Create: `scripts/ci_rehearsal/cleanup.py`
- Create: `scripts/test_ci_rehearsal_cleanup.py`

**Interfaces:**
- Consumes: `InstanceConfig`, `EvidenceWriter`, injected `CleanupAdapter` methods for each resource.
- Produces: `CleanupPlanner.authority_phase(config) -> tuple[CleanupStep, ...]`, `CleanupPlanner.observer_phase(config) -> tuple[CleanupStep, ...]`, `CleanupExecutor.execute(plan) -> CleanupReceipt`, `negative_probe(config) -> ProbeReport`, and `ExternalDestructionVerifier.verify(inventory, platform_capture) -> ExternalCleanupReport`.

- [ ] **Step 1: Write ordering, idempotency, and surviving-capability tests**

```python
def test_cleanup_revokes_publication_before_deleting_observability() -> None:
    names = [step.name for step in CleanupPlanner.authority_phase(config)]
    self.assertLess(names.index("disable-app-installation"), names.index("remove-webhook"))
    self.assertNotIn("destroy-observer", names)
    self.assertEqual(CleanupPlanner.observer_phase(config)[-1].name, "destroy-evidence-writer")

def test_surviving_check_writer_fails_cleanup() -> None:
    adapters.github.create_check_succeeds = True
    with self.assertRaisesRegex(CleanupError, "check writer remains"):
        negative_probe(config, adapters)

def test_phase_two_requires_external_platform_capture() -> None:
    with self.assertRaisesRegex(CleanupError, "external platform record"):
        ExternalDestructionVerifier.verify(inventory, empty_capture())
```

- [ ] **Step 2: Run and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_cleanup.py`

Expected: import failure for `ci_rehearsal.cleanup`.

- [ ] **Step 3: Implement phase one: disable authority while preserving observation**

Phase one preserves an independently credentialed observer/evidence writer while it: stops new triggers; disables App installation/check publication; revokes fixture signing; disables the authority Lambda and schedules; destroys the authority DynamoDB writer; removes webhook and authority mutable routes; removes sandbox ruleset, Mergify routes, Freeze, protections, exclusions, and required contexts or archives the sandbox repository according to the instance retention choice; and deletes every non-observer disposable resource. The observer records exact before/after provider and GitHub responses. Every step is idempotent, and the executor refuses production node IDs, names, App IDs, AWS accounts/resources, and endpoints from the denylist.

- [ ] **Step 4: Implement negative probes and the final immutable checkpoint**

While the observer still exists, probe authentication, conditional append, fixture signing, deployment health/mutable route, webhook/scheduler delivery, and Checks write. All authority capabilities must fail. Record AWS resource-not-found/disabled state and GitHub/Mergify absence. Seal a final KMS-signed immutable cleanup checkpoint containing those probes and the exact phase-two destruction plan; export the complete evidence bundle and KMS public keys, then verify offline reconstruction and retained-evidence readability. A surviving credential, endpoint, installation, scheduler, state writer, signer, or check writer makes phase one terminally failed.

- [ ] **Step 5: Implement phase two: destroy observer and evidence writer**

Only a verified phase-one cleanup receipt plus successful offline reconstruction unlocks phase two. Remove both sandbox-only App installations while their exact keys remain usable for that removal, then revoke/delete both App SSM SecureStrings, revoke audit-signing use, destroy the independent observer Lambda/roles and evidence-writer credential, disable S3 writes, and apply the approved retained-evidence disposition without deleting immutable evidence needed for review. Inert App registrations may remain only if they have no installation and their sole private key is deleted. The destroyed observer cannot prove its own destruction. A separate read-only operator session outside every rehearsal App, IAM role, Lambda, SSM parameter, and evidence writer queries GitHub App installations plus AWS CloudTrail/resource APIs for the exact inventory and captures deletion/disabled/not-found states. The report explicitly trusts those provider records and the owner/operator capture for post-destruction truth; it makes no recursive or self-proving claim. Retained evidence remains independently readable and hash-verifiable.

- [ ] **Step 6: Run cleanup tests**

Run: `python3 scripts/test_ci_rehearsal_cleanup.py`

Expected: ordered two-phase cleanup, interrupted/resumed cleanup, already-absent resources, production-denylist refusal, phase-two refusal before a verified checkpoint, all negative probes, observer destruction, and retained-evidence verification pass.

- [ ] **Step 7: Commit the cleanup slice**

```bash
git add scripts/ci_rehearsal/cleanup.py scripts/test_ci_rehearsal_cleanup.py
git commit -m "test: prove rehearsal resource cleanup"
```

### Task 10: Measurements, Aggregate Receipt, and Owner Decision Report

**Files:**
- Create: `scripts/ci_rehearsal/report.py`
- Create: `scripts/test_ci_rehearsal_report.py`
- Create during execution only, outside Git: `$D4_EVIDENCE_DIR/aggregate-receipt.json`, where `D4_EVIDENCE_DIR` is printed by the driver after allocating a UUIDv4 run ID
- Create during execution only, outside Git: `$D4_EVIDENCE_DIR/owner-decisions.md`

**Interfaces:**
- Consumes: sealed `ScenarioReceipt` objects and independent `ReconstructionReport` objects.
- Produces: `PlatformCapture(provider: str, external_principal_id: str, inventory_digest: str, captured_at: str, observations: tuple[PlatformObservation, ...], capture_digest: str)`, `load_platform_capture(path: Path) -> PlatformCapture`, `build_aggregate(receipts, platform_capture: PlatformCapture) -> AggregateReceipt`, and `render_owner_report(aggregate) -> str`.

- [ ] **Step 1: Write completeness and honest-statistics tests**

```python
def test_report_refuses_missing_hypothesis_or_cut_point() -> None:
    with self.assertRaisesRegex(ReportError, "missing H7|before-merge"):
        build_aggregate(incomplete_receipts(), valid_platform_capture())

def test_report_refuses_missing_post_destruction_platform_capture() -> None:
    with self.assertRaisesRegex(ReportError, "post-destruction platform capture"):
        build_aggregate(complete_receipts(), platform_capture=None)

def test_terminal_tail_is_not_rendered_as_a_maximum() -> None:
    report = render_owner_report(complete_aggregate())
    self.assertIn("unbounded terminal tail", report)
    self.assertNotIn("maximum terminal outage", report)

def test_small_run_reports_raw_min_max_without_statistics() -> None:
    report = render_owner_report(complete_aggregate())
    self.assertIn("raw observations", report)
    self.assertIn("minimum", report)
    self.assertIn("maximum", report)
    self.assertNotRegex(report, r"p90|p95|confidence|convergence")
```

- [ ] **Step 2: Run and observe import failure**

Run: `python3 scripts/test_ci_rehearsal_report.py`

Expected: import failure for `ci_rehearsal.report`.

- [ ] **Step 3: Implement raw measurement aggregation**

Preserve and print every raw observation. For the deliberately repeated rows, compute only integer count, minimum, and maximum for phase/total duration, API/reconciliation calls, rate-limit use, service resource use, evidence bytes, storage requests, and direct cost in the provider's smallest integer billing unit. Do not compute percentiles, confidence, convergence, or an inferred operational budget. Publication uncertainty is observation-only and never contributes an abandonment budget. Terminal tail is labeled unbounded.

- [ ] **Step 4: Implement go/no-go and the two separate owner decisions**

`load_platform_capture` is a closed-schema parser: it requires GitHub/AWS provider observations for every exact resource in the inventory, the external principal ID, provider request IDs/timestamps/status/digests, retained-evidence observation, and a capture digest; unknown, missing, malformed, production-matching, or inventory-mismatched fields fail. The aggregate maps every H1–H10 and every scenario/cut point to exact receipt/root digests and independent reconstruction status and commits the typed platform-capture digest. Any omitted capture, missing/failed/ambiguous cleanup row, or unreconstructed mandatory row yields `NO_GO`. The report then separately explains D5: after the real precursor, terminal failure has no recovery PR and may freeze ordinary merges indefinitely. It must never infer D5 acceptance from D4 values.

- [ ] **Step 5: Run report and all focused tests**

Run: `python3 scripts/test_ci_rehearsal_report.py && find scripts -maxdepth 1 -type f -name 'test_ci_rehearsal_*.py' -print0 | sort -z | xargs -0 -n1 python3`

Expected: all tests pass; incomplete or ambiguous evidence produces `NO_GO`; complete fixture evidence produces two visibly separate decision sections.

- [ ] **Step 6: Commit the report slice**

```bash
git add scripts/ci_rehearsal/report.py scripts/test_ci_rehearsal_report.py
git commit -m "test: report rehearsal evidence and owner decisions"
```

### Task 11: Provision, Execute, Independently Reconstruct, and Tear Down the Live Sandbox

**Files:**
- Read: `ci/1016-rehearsal-resources.toml`
- Use untracked: `/private/tmp/bolt-v2-1016-d4-instance.toml`
- Write evidence only: `$D4_EVIDENCE_DIR`, the absolute run directory printed by successful preflight
- Modify repository files: none

**Interfaces:**
- Consumes: the complete harness and owner-provided platform authorization for a disposable private repository, GitHub App registration/installation, Mergify test integration, and managed ephemeral service/signing/state/object resources.
- Produces: immutable scenario receipts, cleanup receipt, independent reconstruction report, aggregate receipt, and owner-decision report; no surviving publisher capability.

- [ ] **Step 1: Obtain the unavoidable platform preconditions before mutation**

Record explicit authorization and a worst-case cost/time ceiling for initial provisioning and the fixed small run plan. The owner supplies an already-existing empty private sandbox repository, an existing Mergify sandbox integration for that repository, and a separate sandbox AWS account/region; write their nonsecret IDs to the `0600` instance file and the production identities to the separate `0600` denylist. Run Task 3's read-only provisioning preflight before creating any App or AWS resource. Only after it passes, execute the reproducible CloudFormation and two GitHub App Manifest commands from Task 3, install both Apps solely on the sandbox repository, and verify exact App/Mergify outputs and permissions. Mergify behavior is configured only by operator-App commits to the sandbox `.mergify.yml`. Do not copy production secrets, workflows, source, rulesets, Mergify configuration, or App installation. No resource may be created until the Lambda image/SDK, dependency, denylist, separate-account, separate-repository, and Mergify-sandbox gates pass.

- [ ] **Step 2: Run preflight before any scenario**

First read the nonsecret sandbox node ID from the validated instance document into `D4_SANDBOX_NODE_ID`, then run: `just ci-rehearsal preflight --instance /private/tmp/bolt-v2-1016-d4-instance.toml --confirm-sandbox-node-id "$D4_SANDBOX_NODE_ID"`

Expected: sealed PASS receipt proving private/synthetic identity, exact permissions, required reads, denied writes, App and Mergify identities, ruleset baseline, managed signing, append, immutable object, and production denylist separation. Any failure stops execution and begins cleanup.

- [ ] **Step 3: Execute local rows, then live rows one at a time**

Run: `just ci-rehearsal local --all --instance /private/tmp/bolt-v2-1016-d4-instance.toml`

Expected: deterministic rows pass; live-only rows remain explicitly unexecuted.

For each ID assigned to `D4_SCENARIO_ID` from `just ci-rehearsal live --list`, and with `D4_PREFLIGHT_ROOT` set to the exact digest printed by preflight, run:

```bash
just ci-rehearsal live "$D4_SCENARIO_ID" \
  --instance /private/tmp/bolt-v2-1016-d4-instance.toml \
  --preflight-receipt "$D4_PREFLIGHT_ROOT" \
  --confirm-sandbox-node-id "$D4_SANDBOX_NODE_ID"
```

Expected: a fresh domain and terminal sealed receipt for the exact row. Stop on the first ambiguity or safety-critical failure; never convert it to a pass or widen a budget.

- [ ] **Step 4: Repeat the complete ceremony and abort runs from clean baselines**

Use the immutable signed run-plan digest: two clean `full-ceremony` runs, two lost-create/delayed-read runs, two `pre-precursor-abort` runs at each relevant cut point, and one of each deterministic negative row. Record every raw observation, dependency fault, rate limit, resource use, evidence volume, cost, and operator time. Do not choose budgets or claim statistical confidence. If a discrepancy motivates more runs, stop and obtain owner approval for a worst-case additional cost/time ceiling before signing a separate expanded run plan.

- [ ] **Step 5: Execute phase-one authority cleanup and seal it**

Run: `just ci-rehearsal live cleanup-authority --instance /private/tmp/bolt-v2-1016-d4-instance.toml --confirm-sandbox-node-id "$D4_SANDBOX_NODE_ID"`

Expected: App publication, authority Lambda, fixture signer, DynamoDB writer, webhooks/schedulers, mutable authority routes, sandbox merge controls, and all other authority capabilities are disabled or deleted while the independent observer/evidence writer remains. Negative probes fail, resource deletion is observed, and the observer uses the still-live audit-signing boundary to seal the final immutable cleanup checkpoint plus phase-two plan.

- [ ] **Step 6: Build the final aggregate and independently reconstruct before observer destruction**

Run: `just ci-rehearsal aggregate --evidence-dir "$D4_EVIDENCE_DIR"`

Expected: prints `D4_AGGREGATE_ROOT` covering all scenarios, raw measurements, phase-one cleanup, negative probes, immutable objects, KMS public-key digests, and the signed cleanup checkpoint. Aggregation before phase-one cleanup is rejected.

Give a separate read-only reviewer only the immutable evidence export and public verification roots, not service/state credentials. With `D4_AGGREGATE_ROOT` set to that printed digest, run `just ci-rehearsal reconstruct --bundle "$D4_AGGREGATE_ROOT" --offline`.

Expected: no network calls; offline public-key verification reconstructs at least one allow, deny, spoof, moved-state, retry, uncertain-publication, invalidation, terminal, stop-only, audit-damage, and cleanup case and maps every H1–H10 row. Record reviewer source and exact configured model if AI is used.

- [ ] **Step 7: Execute phase-two observer/evidence-writer destruction**

Run: `just ci-rehearsal live cleanup-observer --instance /private/tmp/bolt-v2-1016-d4-instance.toml --cleanup-root "$D4_AGGREGATE_ROOT" --confirm-sandbox-node-id "$D4_SANDBOX_NODE_ID"`

Expected: command first re-verifies the signed cleanup root and offline reconstruction, then removes both sandbox-only App installations, deletes both App SSM parameters, revokes audit-signing use, deletes observer Lambda/roles and evidence-writer identity, disables S3 writes, and leaves only immutable read-only retained evidence. It does not claim to prove its own destruction.

From a separate read-only GitHub owner session and AWS read-only profile named in the external operator's local config—not either rehearsal App or role—run:

```bash
python3 -m scripts.ci_rehearsal.provision verify-destroyed \
  --inventory "$D4_EVIDENCE_DIR/resource-inventory.json" \
  --external-operator-config /private/tmp/bolt-v2-1016-d4-external-readonly.toml \
  --output "$D4_EVIDENCE_DIR/post-destruction-platform-capture.json"
```

Expected: GitHub installation queries and AWS CloudTrail/Lambda/IAM/SSM/KMS/DynamoDB/S3 APIs show the exact Apps, keys, roles, functions, parameters, mutable writers, and routes deleted, disabled, or denied; retained evidence remains readable. The capture records provider request IDs, timestamps and response digests without credentials. The final report labels provider audit/API records plus owner/operator capture as the post-destruction trust boundary; absence of an external capture is `NO_GO`.

- [ ] **Step 8: Generate the evidence-based owner report**

Run: `just ci-rehearsal report --bundle "$D4_AGGREGATE_ROOT" --platform-capture "$D4_EVIDENCE_DIR/post-destruction-platform-capture.json" --output "$D4_EVIDENCE_DIR"`

Expected: `aggregate-receipt.json` and `owner-decisions.md`; the report is `GO_FOR_OWNER_DECISIONS` only if all mandatory evidence, phase-one cleanup, offline reconstruction, externally captured phase-two destruction, and retained-evidence availability pass. It states that post-destruction proof relies on GitHub/AWS platform records and owner/operator capture, reports raw/min/max observations without statistical claims, proposes no limits unsupported by this small run, and presents D5 separately without accepting it.

### Task 12: Delete the Rehearsal Harness After Evidence Acceptance

**Files:**
- Delete: `scripts/ci_rehearsal/`
- Delete: `scripts/test_ci_rehearsal_protocol.py`
- Delete: `scripts/test_ci_rehearsal_fixture_engine.py`
- Delete: `scripts/test_ci_rehearsal_config.py`
- Delete: `scripts/test_ci_rehearsal_state.py`
- Delete: `scripts/test_ci_rehearsal_github.py`
- Delete: `scripts/test_ci_rehearsal_evidence.py`
- Delete: `scripts/test_ci_rehearsal_scenarios.py`
- Delete: `scripts/test_ci_rehearsal_driver.py`
- Delete: `scripts/test_ci_rehearsal_cleanup.py`
- Delete: `scripts/test_ci_rehearsal_report.py`
- Delete: `ci/1016-rehearsal-resources.toml`
- Modify: `justfile` (remove only `ci-rehearsal`)
- Modify: `docs/ci/1016-program-ledger.md` (record exact evidence/cleanup/deletion PR and main SHAs)

**Interfaces:**
- Consumes: accepted immutable evidence bundle, successful cleanup receipt, and owner disposition.
- Produces: zero executable rehearsal code or repository hook; durable documentation references only.

- [ ] **Step 1: Prove deletion eligibility**

Require the cleanup receipt, post-cleanup negative probes, independent reconstruction, evidence retention location/digest, and owner disposition. Search for every import, recipe, manifest, path, context, resource name, credential handle, webhook, scheduler, and mutable endpoint; any live dependency blocks deletion completion until removed, not retention of the harness.

- [ ] **Step 2: Delete the complete executable boundary in one issue-owned slice**

Use `apply_patch` to delete the listed files and remove only the `ci-rehearsal` Just recipe. Update the ledger with exact evidence roots, cleanup receipt, deletion PR, and remaining D4/D5 decision status. Do not delete immutable historical evidence or claim D5 acceptance.

- [ ] **Step 3: Verify no executable residue**

Run: `rg -n 'ci_rehearsal|ci-rehearsal|trusted-ci-verifier-rehearsal' scripts ci justfile .github`

Expected: no executable/configuration matches; documentation/history references may remain only under `docs/`.

Run: `git diff --check && just ci-lint-workflow && just source-fence-static`

Expected: all commands exit 0 with no Rust compilation.

- [ ] **Step 4: Commit the deletion slice**

```bash
git add -A -- scripts/ci_rehearsal \
  scripts/test_ci_rehearsal_protocol.py \
  scripts/test_ci_rehearsal_fixture_engine.py \
  scripts/test_ci_rehearsal_config.py \
  scripts/test_ci_rehearsal_state.py \
  scripts/test_ci_rehearsal_github.py \
  scripts/test_ci_rehearsal_evidence.py \
  scripts/test_ci_rehearsal_scenarios.py \
  scripts/test_ci_rehearsal_driver.py \
  scripts/test_ci_rehearsal_cleanup.py \
  scripts/test_ci_rehearsal_report.py \
  ci/1016-rehearsal-resources.toml justfile docs/ci/1016-program-ledger.md
git commit -m "chore: remove disposable control-plane rehearsal"
```

The deletion PR remains separate from the eventual production precursor and atomic replacement. Its merge only proves that disposable resources and code are gone; it does not authorize production implementation.
