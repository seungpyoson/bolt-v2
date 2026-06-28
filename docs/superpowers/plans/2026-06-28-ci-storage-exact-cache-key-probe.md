# CI Storage Exact Cache-Key Probe Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a generic read-only `ci-storage-audit --cache-key label=exact-key` probe that reports whether exact GitHub Actions cache keys exist.

**Architecture:** Extend the existing `scripts/ci_storage_audit.py` data layer with a cache-key probe mode. GitHub's `actions/caches?key=` filter is treated as prefix-based; exactness is enforced locally by filtering returned cache entries to `entry["key"] == requested_key`. No workflow job, workflow-job verifier mandate, nextest-specific key generation, or deletion behavior is included.

**Tech Stack:** Python standard library, GitHub CLI REST API wrapper already in `scripts/ci_storage_audit.py`, `unittest`, `just`.

---

## Spec

### Problem

PR #986 tried to answer whether a GitHub Actions cache key that CI appeared to save was visible through the Actions cache API. The useful part is a generic exact-key lookup. The flawed part is treating GitHub's `key` query parameter as exact when live API behavior returns prefix matches.

### Invariants

- Read-only only: no cache deletion, cache mutation, workflow dispatch, or GitHub settings change.
- Generic cache inspection: no root-nextest-only logic in the CLI.
- Exactness is local: API prefix matches are not enough to report an exact key as present.
- Stable JSON is append-only: new keys may be added, existing `ci_storage_audit.py --json` fields must not be renamed or removed.
- Permanent CI-job promotion remains deferred and governed by the #939 checkpoint criteria.

### User-Facing Behavior

Run:

```bash
just ci-storage-audit --cache-key nextest=nextest-archive-v3-Linux-ARM64-test-profile-shards-4-abc123
```

or:

```bash
python3 scripts/ci_storage_audit.py \
  --repo seungpyoson/bolt-v2 \
  --cache-key nextest=nextest-archive-v3-Linux-ARM64-test-profile-shards-4-abc123 \
  --cache-key cargo=v0-rust-cargo-registry-git-v1-Linux-arm64-abc123
```

Text output reports, for each requested key:

- label
- requested key
- status: `present` or `missing`
- `exact_count`
- `api_prefix_count`
- matching entries with cache ID, ref, size, and last-accessed timestamp
- a note when the API returned prefix matches but no exact match

JSON output uses the same data shape under `cache_key_probes`, with `exact_count` and `api_prefix_count` as separate fields.

### Non-Goals

- No `cache-persistence-audit` workflow job.
- No changes to `.github/workflows/ci.yml`.
- No workflow-job contract checks in `scripts/verify_ci_workflow_hygiene.py`.
- No nextest-specific cache-key computation.
- No stale-cache candidate generation.
- No deletion or armed janitor behavior.

### Evidence Required

- Unit tests prove exact present, exact missing, prefix collision, repeated keys, and invalid input.
- A live optional smoke command may be run read-only against a known cache prefix, but implementation completion does not require a live cache to exist.
- Local checks: `python3 scripts/test_ci_storage_audit.py`, `just ci-lint-workflow`, and `just fmt-check`.

## Implementation Plan

### Task 1: Add Cache Entry Normalization

**Files:**
- Modify: `scripts/ci_storage_audit.py`
- Test: `scripts/test_ci_storage_audit.py`

- [ ] Add a helper in `scripts/ci_storage_audit.py`:

```python
def cache_entry_from_raw(raw: dict[str, Any]) -> dict[str, Any]:
    return {
        "cache_id": raw.get("id"),
        "ref": optional_text(raw.get("ref")),
        "key": optional_text(raw.get("key")),
        "last_accessed_at": optional_text(raw.get("last_accessed_at")),
        "size_bytes": nonnegative_int(raw.get("size_in_bytes")),
    }
```

- [ ] Replace the inline entry construction in `fetch_cache()` with `cache_entry_from_raw(raw)`.

- [ ] Run:

```bash
python3 scripts/test_ci_storage_audit.py
```

Expected: existing tests still pass.

### Task 2: Add Probe Request Parsing

**Files:**
- Modify: `scripts/ci_storage_audit.py`
- Test: `scripts/test_ci_storage_audit.py`

- [ ] Add:

```python
class CacheKeyProbeRequest(NamedTuple):
    label: str
    key: str
```

and import `NamedTuple` from `typing`.

- [ ] Add parser:

```python
def parse_cache_key_probe(raw: str) -> CacheKeyProbeRequest:
    if "=" not in raw:
        raise AuditError("--cache-key must be LABEL=KEY")
    label, key = raw.split("=", 1)
    label = label.strip()
    key = key.strip()
    if not label:
        raise AuditError("--cache-key label must not be empty")
    if not key:
        raise AuditError("--cache-key key must not be empty")
    return CacheKeyProbeRequest(label=label, key=key)
```

- [ ] Add tests for:

```python
self.assertEqual(
    ci_storage_audit.parse_cache_key_probe("nextest=exact-key"),
    ci_storage_audit.CacheKeyProbeRequest("nextest", "exact-key"),
)
```

and invalid inputs:

```python
for raw in ("nokey", "=key", "label="):
    with self.subTest(raw=raw):
        with self.assertRaises(ci_storage_audit.AuditError):
            ci_storage_audit.parse_cache_key_probe(raw)
```

- [ ] Run:

```bash
python3 scripts/test_ci_storage_audit.py
```

Expected: new parser tests pass.

### Task 3: Implement Exact-Key Probe Logic

**Files:**
- Modify: `scripts/ci_storage_audit.py`
- Test: `scripts/test_ci_storage_audit.py`

- [ ] Update `FakeClient.api()` in `scripts/test_ci_storage_audit.py` to support responses keyed by `(path, sorted_params)`:

```python
response_key = (path, tuple(sorted((params or {}).items())))
value = self.responses.get(response_key, self.responses.get(path))
if value is None:
    raise KeyError(response_key)
```

- [ ] Add probe implementation:

```python
def fetch_cache_key_probes(
    client: GhClient,
    requests: list[CacheKeyProbeRequest],
) -> list[dict[str, Any]]:
    probes: list[dict[str, Any]] = []
    for request in requests:
        payload = require_object(
            client.api(
                "actions/caches",
                params={"key": request.key, "per_page": "100"},
                paginate=True,
            ),
            "actions/caches",
        )
        raw_entries = list_field(payload, "actions_caches", "actions/caches")
        prefix_entries = [
            cache_entry_from_raw(raw)
            for raw in raw_entries
            if isinstance(raw, dict)
        ]
        exact_entries = [
            entry for entry in prefix_entries
            if entry.get("key") == request.key
        ]
        api_prefix_count, count_source = count_with_source(payload, fallback=len(prefix_entries))
        probes.append(
            {
                "label": request.label,
                "key": request.key,
                "present": bool(exact_entries),
                "exact_count": len(exact_entries),
                "api_prefix_count": api_prefix_count,
                "api_prefix_count_source": count_source,
                "api_prefix_enumerated_count": len(prefix_entries),
                "prefix_only_count": max(0, len(prefix_entries) - len(exact_entries)),
                "entries": exact_entries,
            }
        )
    return probes
```

- [ ] Add tests for exact present and exact missing.

- [ ] Add the mandatory prefix-collision regression test:

```python
client = FakeClient(
    {
        (
            "actions/caches",
            (("key", "foo"), ("per_page", "100")),
        ): {
            "total_count": 1,
            "actions_caches": [
                {
                    "id": 301,
                    "ref": "refs/heads/main",
                    "key": "foo-longer",
                    "last_accessed_at": "2026-06-25T10:00:00Z",
                    "size_in_bytes": 1024,
                }
            ],
        },
    }
)

probes = ci_storage_audit.fetch_cache_key_probes(
    client,
    [ci_storage_audit.CacheKeyProbeRequest("probe", "foo")],
)

self.assertFalse(probes[0]["present"])
self.assertEqual(probes[0]["exact_count"], 0)
self.assertEqual(probes[0]["api_prefix_count"], 1)
self.assertEqual(probes[0]["api_prefix_enumerated_count"], 1)
self.assertEqual(probes[0]["prefix_only_count"], 1)
self.assertEqual(probes[0]["entries"], [])
```

- [ ] Run:

```bash
python3 scripts/test_ci_storage_audit.py
```

Expected: all exact-key probe tests pass.

### Task 4: Wire CLI Mode and Output

**Files:**
- Modify: `scripts/ci_storage_audit.py`
- Test: `scripts/test_ci_storage_audit.py`

- [ ] Add to `parse_args()`:

```python
parser.add_argument(
    "--cache-key",
    action="append",
    default=[],
    metavar="LABEL=KEY",
    help="Probe an exact Actions cache key. Repeat to probe multiple keys.",
)
```

- [ ] Add snapshot builder:

```python
def build_cache_key_probe_snapshot(
    client: GhClient,
    *,
    repo: str,
    snapshot_utc: str,
    requests: list[CacheKeyProbeRequest],
) -> dict[str, Any]:
    return {
        "snapshot_utc": snapshot_utc,
        "repo": repo,
        "cache_key_probes": fetch_cache_key_probes(client, requests),
    }
```

- [ ] Add text renderer:

```python
def render_cache_key_probe_text(snapshot: dict[str, Any]) -> str:
    probes = list_field(snapshot, "cache_key_probes", "cache_key_probes")
    lines = [
        f"CI cache key probe for {snapshot['repo']}",
        f"Snapshot: {snapshot['snapshot_utc']}",
        "",
        "Cache key probes:",
    ]
    for raw in probes:
        if not isinstance(raw, dict):
            continue
        status = "present" if raw.get("present") else "missing"
        lines.append(
            f"  - {raw.get('label')}: {status}; "
            f"exact_count={raw.get('exact_count')} "
            f"api_prefix_count={raw.get('api_prefix_count')} "
            f"api_prefix_enumerated={raw.get('api_prefix_enumerated_count')} "
            f"key={raw.get('key')}"
        )
        if raw.get("exact_count") == 0 and raw.get("api_prefix_enumerated_count", 0) > 0:
            lines.append("      note=API returned prefix matches, but no exact key matched")
        entries = raw.get("entries")
        if isinstance(entries, list):
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                lines.append(
                    f"      id={entry.get('cache_id')} ref={entry.get('ref')} "
                    f"size={human_bytes(nonnegative_int(entry.get('size_bytes')))} "
                    f"last_accessed_at={entry.get('last_accessed_at')}"
                )
    return "\n".join(lines)
```

- [ ] In `main()`, branch before default branch inference:

```python
snapshot_utc = isoformat_utc(dt.datetime.now(dt.UTC))
if args.cache_key:
    snapshot = build_cache_key_probe_snapshot(
        client,
        repo=repo,
        snapshot_utc=snapshot_utc,
        requests=[parse_cache_key_probe(raw) for raw in args.cache_key],
    )
    if args.json:
        print(json.dumps(snapshot, indent=2, sort_keys=True))
    else:
        print(render_cache_key_probe_text(snapshot))
    return 0
```

- [ ] Add renderer tests proving prefix-only rows show the note and do not render cache IDs as exact matches.

- [ ] Run:

```bash
python3 scripts/test_ci_storage_audit.py
```

Expected: parser, probe, and renderer tests pass.

### Task 5: Local Gate Wiring

**Files:**
- Modify: `justfile`
- Modify: `scripts/verify_ci_workflow_hygiene.py`
- Modify: `scripts/test_verify_ci_workflow_hygiene.py`

- [ ] Add this command to `ci-lint-workflow-inner` in `justfile`, immediately after `python3 scripts/test_verify_ci_workflow_hygiene.py`:

```just
    if ! python3 scripts/test_ci_storage_audit.py; then
        failed=1
    fi
```

- [ ] Add the command to `CI_LINT_WORKFLOW_INNER_REQUIRED_COMMANDS` in `scripts/verify_ci_workflow_hygiene.py`:

```python
CI_LINT_WORKFLOW_INNER_REQUIRED_COMMANDS = (
    "python3 scripts/test_ci_storage_audit.py",
    "python3 scripts/test_root_bin_sidecars.py",
)
```

- [ ] Update `assert_local_verification_gate_recipes_are_enforced()` in `scripts/test_verify_ci_workflow_hygiene.py` so the passing fixture includes:

```python
    python3 scripts/test_ci_storage_audit.py
```

inside `ci-lint-workflow-inner`, before `python3 scripts/test_root_bin_sidecars.py`.

- [ ] Add a negative check in `assert_local_verification_gate_recipes_are_enforced()`:

```python
    missing_storage_audit_test = justfile_text.replace("    python3 scripts/test_ci_storage_audit.py\n", "")
    missing_storage_audit_test_errors = verifier.verify_local_verification_gate_recipes(missing_storage_audit_test)
    if not any(
        "justfile ci-lint-workflow-inner must run python3 scripts/test_ci_storage_audit.py" in error
        for error in missing_storage_audit_test_errors
    ):
        raise AssertionError(
            f"ci storage audit test wiring drift was silent, got: {missing_storage_audit_test_errors}"
        )
```

- [ ] Do not add any workflow checks for a cache-persistence job.

- [ ] Run:

```bash
python3 scripts/test_ci_storage_audit.py
just ci-lint-workflow
just fmt-check
```

Expected: all three pass. If `just ci-lint-workflow` fails for unrelated existing drift, capture the exact failure and do not widen scope.

## Internal Adversarial Review

### Finding 1: Exact-key bug can reappear

Risk: A future implementation may trust GitHub's `key` filter and report prefix matches as exact hits.

Resolution: The prefix-collision regression test is mandatory. The output model separates `exact_count` from `api_prefix_count`, so a prefix-only response is visible and cannot be mistaken for a hit.

### Finding 2: Output can be ambiguous

Risk: A user may see `count=1` and infer the exact key exists when the API only returned a prefix match.

Resolution: Do not use a single `count` field in probe output. Use `exact_count`, `api_prefix_count`, `api_prefix_enumerated_count`, and `prefix_only_count`.

### Finding 3: Scope can creep into CI workflow governance

Risk: Reintroducing PR #986's permanent `cache-persistence-audit` job would add workflow noise and verifier burden before the signal is proven useful.

Resolution: The implementation plan explicitly excludes workflow changes and workflow-hygiene enforcement. #939 records the promotion criteria for reconsidering a permanent CI job later.

## External Model Review Request

Use this prompt for Gemini/Kimi/Claude adversarial review before implementation:

```text
You are reviewing a plan for bolt-v2. Be adversarial and focus on correctness, scope control, and test adequacy.

Context:
- PR #986 tried to add a same-run GitHub Actions cache persistence audit.
- The useful part is a generic exact-key probe for the existing read-only storage audit CLI.
- GitHub's Actions cache API `key` query behaves as a prefix filter, so exactness must be enforced locally.
- Current decision: implement only `ci-storage-audit --cache-key label=exact-key`; do not add a CI workflow job.

Planned behavior:
- Query `repos/{owner}/{repo}/actions/caches?key=<requested-key>&per_page=100`.
- Normalize returned cache entries.
- Filter exact matches with `entry["key"] == requested_key`.
- Report `present`, `exact_count`, `api_prefix_count`, `api_prefix_enumerated_count`, `prefix_only_count`, and exact matching entries.
- Support repeated `--cache-key`.
- Preserve existing broad `ci_storage_audit.py --json` contract.
- Add unit tests for exact present, exact missing, prefix collision, repeated keys, invalid input, and renderer output.

Non-goals:
- No workflow job.
- No `.github/workflows/ci.yml` changes.
- No workflow-job hygiene mandate.
- No nextest-specific key generation.
- No deletion or stale-cache janitor behavior.

Please answer:
1. Is this plan sufficient to avoid the PR #986 false-positive exact-key bug?
2. Are the output fields clear enough to avoid confusing prefix matches with exact matches?
3. Is any test missing?
4. Is any non-goal actually required for this to be useful?
5. Are there hidden integration risks in `ci_storage_audit.py`, `just ci-lint-workflow`, or the stable JSON contract?

Return findings first, ordered by severity. If no blocking findings, say so explicitly.
```

## Completion Criteria

- Plan remains limited to generic CLI/data-layer support.
- Prefix-collision test fails on PR #986-style implementation and passes on the corrected implementation.
- Existing storage audit JSON contract remains backward-compatible.
- Local verification commands are run or any inability to run them is reported plainly.
