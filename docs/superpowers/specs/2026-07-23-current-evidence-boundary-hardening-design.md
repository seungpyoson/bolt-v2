# Current Decision-Evidence Boundary Hardening Design

**Issue slice:** #1354 current-only decision-evidence contract  
**Pull request:** #1505  
**Accepted design baseline:** `d9fea30cf7aceeb2dedb8f291af99cbbe2441ba4`

## Objective

Close the six boundary defects found by the exact-head adversarial review without reintroducing historical decoding, migration, alternate writers, stream repair, or work owned by #1385. The result retains one current-only evidence path and makes unsuccessful storage transitions, retained stream state, wire-domain coverage, and verification scope explicit.

## Scope

This change owns:

- process-lifetime sink state after a write or synchronization failure;
- strict content validation for retained observation streams without making observation evidence readiness authority;
- descriptor-relative containment for active and retired evidence paths;
- byte-exact coverage of every admitted current wire enum branch and optional-field representation;
- semantic migration of stale tests exposed by the current recorder;
- exact-head root and backtesting-workspace advisory evidence.

It does not own historical compatibility, migration, truncation, repair, replay, durable retry authority, ordinals, rotation, retirement, compaction, capacity expansion, observation novelty, or restart exact-once. Those boundaries remain unchanged; #1385 retains its declared work.

## Sink State and Failure Algebra

Each machine or observation sink has exactly one process-lifetime state:

```rust
enum DurableSinkState {
    Healthy(File),
    Poisoned(PoisonCause),
}

enum CommitPhase {
    Write,
    Sync,
}

enum PoisonCause {
    CommitIndeterminate {
        phase: CommitPhase,
        cause: Arc<str>,
    },
    StartupContentInvalid {
        cause: Arc<str>,
    },
}
```

The public record failure vocabulary distinguishes:

- `Rejected`: validation or encoding failed before the sink was touched; the sink remains healthy.
- `CommitIndeterminate { phase, cause }`: the first write or synchronization attempt failed after I/O began; the sink becomes poisoned. The result never claims that zero bytes or a complete durable record exists.
- `SinkPoisoned { first_cause }`: the sink was already poisoned; this attempt performs no I/O.

`AppendReceipt` remains constructible only after the complete newline-terminated record is written and `sync_data` succeeds. A write or sync failure permanently poisons only the selected sink. The other sink remains independent.

No caller retries a semantic fact on the same process-lifetime sink after `CommitIndeterminate` or `SinkPoisoned`. The generated effect policies continue to determine domain behavior:

- `MustPrecedeNewRisk` and `ReconciliationFailClosed` stop before the external effect;
- `PreserveResult` and `RiskReducingContinues` preserve the domain result while surfacing the evidence failure;
- `ObservationBoundedFailure` reports once per purpose episode and suppresses repeated reports, but never resumes I/O on the poisoned sink.

The recorder does not latch trading state or repair storage. Restart validation remains the only re-entry, and it grants no generic replay permission.

## Retained Stream Validation

One validator accepts an expected sink and applies the same framing and current-contract rules to both retained streams:

```rust
fn validate_stream(
    file: &mut File,
    expected_sink: KnownSink,
    max_bytes: Option<u64>,
) -> Result<ValidatedStream>;
```

Validation checks the byte ceiling when configured, newline framing, blank records, JSON decoding, exact current identity, exact gate ID, full identity-specific payload decoding, and identity-to-sink membership.

Machine content is recovery authority. Any machine validation failure blocks runtime construction.

Observation content is not recovery or readiness authority. A missing or valid observation stream opens healthy. Invalid retained observation content is left byte-for-byte unchanged and constructs the observation sink as `Poisoned(StartupContentInvalid)`. Machine recovery and the machine sink remain usable. The runtime exposes a typed observation status:

```rust
enum ObservationStreamStatus {
    Available,
    Poisoned { cause: Arc<str> },
}
```

Runtime construction logs the cause once with `log::error!`, and callers may inspect the typed status. The status cannot construct readiness permission. Observation attempts flow through the existing bounded observation outcome and perform no I/O while poisoned.

There is no truncation, repair, replacement file, fallback path, or alternate recorder.

## Descriptor-Relative Path Authority

Lexical configuration validation remains responsible for rejecting empty, absolute, parent-traversal, and non-normal relative paths. Filesystem containment uses one Unix descriptor-relative authority on the supported macOS development and Linux production targets. `catalog_directory` is resolved exactly once to an absolute directory; its original pathname is then discarded and the resolved components are opened without symlink traversal into a retained descriptor.

The runtime opens and retains `catalog_directory` as a directory descriptor. For every configured relative path it:

1. walks each parent component with `openat(parent_fd, component, O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC)`;
2. opens or creates the final active stream with `openat(parent_fd, basename, O_NOFOLLOW | O_RDWR | O_APPEND | O_CLOEXEC | O_CREAT, private_mode)`;
3. validates the opened descriptor with `fstat`;
4. checks retired basenames relative to the same walked parent descriptors with `fstatat(..., AT_SYMLINK_NOFOLLOW)`.

Symlinked intermediate or final components are invalid even when their target remains inside the catalog. Active streams are compared by device and inode after opening. The retained descriptors, rather than later pathname lookups, are the sole filesystem authority.

The existing per-stream canonicalize-then-reopen and before/after pathname metadata checks are removed. After the one catalog resolution, active and retired operations use only retained directory descriptors. Non-Unix builds do not provide a weaker production constructor; `DecisionEvidenceRuntime::open` returns an explicit unsupported-target error before inspecting or creating any stream.

The implementation is decomposed around retained directory handles so tests can open a parent, rename or replace its pathname, and prove subsequent relative opens remain anchored to the original descriptor. No filesystem abstraction trait or second runtime implementation is introduced.

## Exhaustive Current Wire Corpus

Rust frozen wire types remain the only authority for field shape and enum domains. TOML continues to own identity, purpose, sink, policy, and consumer relationships; it does not enumerate payload cases.

Every exact current identity owns an ordered, append-only positive JSONL corpus and a small raw rejection corpus. Typed case builders co-located with the codecs produce the positive cases. Coverage is linear rather than Cartesian:

- one canonical baseline case;
- at least one case for every frozen enum variant and tagged payload branch reachable from that identity;
- every optional field serialized with a present value at least once;
- every optional field serialized as explicit `null` at least once;
- every optional field omitted at least once when omission is admitted;
- additional cases only for semantic combinations whose validity depends on multiple fields.

Absent and explicit null may decode to the same semantic value, but both accepted byte representations remain independently frozen.

Frozen enum helpers use exhaustive matches without wildcard arms. Adding a frozen variant therefore fails compilation until its typed coverage case is adjudicated. Behavioral tests encode every typed case, compare it byte-for-byte with committed JSONL, decode it through the sole exact-identity dispatch, and assert the union of cases covers the complete declared variant and optional-state inventory.

Raw rejection cases cover failures that are clearer as immutable bytes, including unknown enum/tag spelling, wrong gate ID, wrong exact pair, malformed framing, extra fields, legacy identities, and wrong-sink identities. Existing programmatic per-field missing/wrong-type and absent/null semantic mutation tests remain.

Tests inspect behavior and committed bytes, not Rust source text.

## Test Migration

Tests using the old fake-writer observation model are migrated to the current semantic contract:

- recorder assertions take a checkpoint after fixture setup and examine only facts emitted by the phase under test;
- tests assert fact kind and semantic content rather than whole-stream emptiness when setup legitimately emits evidence;
- failure tests assert the typed `RecordFailure` category and stable production context, never an injected fake error literal;
- the complete root suite runs after focused migrations to enumerate any additional same-class stale assertions hidden by fail-fast.

Assertions are not weakened: zero-submit, fail-closed, risk-reducing-continuation, and exact evidence-emission behavior remain directly tested.

## Backtesting Verification

The backtesting vertical slice is a separate Cargo workspace and must have explicit advisory evidence whenever this PR changes it. The authoritative advisory workflow runs, at the exact PR head:

- formatting for both workspaces;
- root clippy, unfiltered tests, and release build;
- backtesting-workspace clippy, unfiltered tests, and host-native release build using its locked manifest.

The workflow directly contains these commands; no second verification manifest or source-scanning test is added. The PR body states only coverage the workflow actually performs.

## Required Behavioral Evidence

The implementation is complete only when tests prove:

- validation rejection leaves a healthy sink untouched;
- partial write and post-write sync failures return `CommitIndeterminate` with the correct phase and poison only that sink;
- later attempts return `SinkPoisoned`, preserve bytes, and perform no I/O;
- every generated effect policy preserves its declared domain behavior under indeterminate and poisoned outcomes;
- machine corruption blocks startup;
- blank, torn, unknown, malformed, legacy, and wrong-sink observation histories remain unchanged and pre-poison only the observation sink;
- observation poisoning is visible through typed startup status and bounded record outcomes;
- intermediate and final symlinks, parent replacement, active aliases, and retired-path aliases cannot escape descriptor authority;
- every current identity's committed corpus is byte exact and covers all admitted enum, tagged, and optional byte states;
- the migrated risk/admission tests retain their original semantic guarantees;
- root and backtesting exact-head advisory lanes complete successfully.

## Operational Consequences

A torn or indeterminate machine stream remains fail-closed on restart. Operators preserve it and follow the existing archive-and-forward-fix posture; the runtime never truncates or repairs it. A poisoned observation stream creates an explicit diagnostic gap but cannot block machine recovery or live readiness. Live activation remains prohibited until the separately documented hard-cutover quiescence contract is satisfied.
