# Immutable Evidence Record Identities Design

## Decision

PR #1470 must not change a serialized evidence domain while continuing to emit
the historical `(kind, schema_version)` pair. Before the richer entry-skip
reason domain can ship, Bolt will replace the global ordered-schema mechanism
with immutable, per-record identities resolved through one generated Rust
dispatcher.

An identity is the exact pair `(kind, schema_version)`. The numeric component
has no ordering semantics: readers must not compare it with `<`, `>`, or a
global current version. A new wire shape or semantic meaning receives a new
registered pair. Existing pairs are permanent and are never reinterpreted.

The new entry-skip wire shape receives a new registered `kind`; unchanged
record kinds continue emitting their existing v15 identity byte-for-byte. No
new envelope field and no global v16 are introduced.

## Invariants

1. A registered identity permanently binds its wire shape, enum domains,
   units, semantic conversion, producer gate ID, and recovery-consumer set.
2. TOML owns exact-pair registration, current-encoder selection, producer-gate
   ownership, decoder action, and recovery-consumer membership. Rust owns wire
   DTOs and exhaustive conversions to runtime or recovery semantics.
3. Generated Rust is the only identity resolver. It dispatches from the
   declared pair before payload parsing; it never tries one decoder and falls
   back to another.
4. Each record family has exactly one current encoder. Historical identities
   are decode-only.
5. Unknown identities fail closed. A known identity relevant to the active
   recovery consumer fails closed on malformed payload or failed conversion.
6. A known identity explicitly irrelevant to the active recovery consumer is
   skipped after envelope and identity validation, without parsing its payload.
   This is consumer policy decided before payload parsing, never a parse-error
   fallback.
7. Historical evidence is never rewritten. Supported historical
   recovery-bearing identities have exact Rust decoders and converge on the
   same canonical recovery events as current identities.
8. `gate_version` remains recorded diagnostic metadata. It must be non-empty,
   but the running package version cannot select or invalidate a decoder.
   Registered `gate_id` remains an exact identity-owned constraint.
9. An evolving runtime semantic enum is never serialized through a historical
   identity. A changed semantic domain receives a dedicated wire type and an
   exhaustive typed conversion before its new identity becomes current.
10. New wire identities contain no `Unclassified`, generic `other`, wildcard,
    textual semantic fallback, or implicit default. The frozen v15 decoder may
    retain historical deterministic decoding rules solely because those rules
    are already part of v15's immutable meaning; they are never selected by a
    parse failure and are never used for new writes.
11. The Python decision-evidence migration lane is retired after every
    historically supported recovery identity and disposition is represented
    in the Rust registry and tests.

## Stable Envelope

The existing JSONL envelope remains the framing contract:

- one complete JSON object per line;
- `kind` and `schema_version` form the identity;
- `recorded_at_utc_ns` is a positive event-recording timestamp;
- `gate_id` is the identity-owned producer ID;
- `gate_version` is non-empty diagnostic provenance;
- the identity-specific payload remains under its registered payload field.

The header probe tolerates payload fields so it can resolve relevance without
decoding irrelevant payloads. Identity-specific payload DTOs use strict field
validation except where the frozen legacy identity already defined optional or
defaulted decoding behavior.

## Registry

The Rust-parsed TOML registry contains one row per immutable semantic binding.
A row may list multiple exact `(kind, schema_version)` pairs only when those
pairs intentionally share the same frozen decoder action, gate ownership, and
consumer set. Each row registers:

- `kind` and its exact `schema_version` values;
- generated Rust identity and record-kind names;
- record family and its optional current encoder;
- exact producer `gate_id`;
- closed decoder action;
- closed set of recovery consumers.

The recovery-consumer set is not a binary recovery flag. It is a closed Rust
domain covering every current reader, including entry-chain, reservation,
settlement, and shadow-PnL consumers. An explicit empty set means observation
only. Unknown consumers, missing classifications, duplicate pairs, duplicate
current encoders, mutation/deletion of frozen rows, and TOML/Rust binding drift
fail generation or compilation.

## Rust Boundaries

Generated code provides:

- a sealed `EvidenceRecordIdentity` enum;
- total `(kind, schema_version)` resolution;
- identity metadata and recovery-consumer membership;
- a closed decoder-action binding consumed by exhaustive Rust matches;
- exactly one current encoder binding per record family.

Handwritten Rust provides:

- private wire DTOs for changed identities and exact frozen DTOs for historical
  shapes whose runtime semantics have diverged;
- exact payload decoding and encoding;
- exhaustive conversion from runtime semantics to the current wire DTO;
- exhaustive conversion from supported wire DTOs to canonical recovery
  events.

The generator must not encode Rust field shapes in TOML and must not generate
compatibility guesses. TOML type paths are compile-time bindings, never runtime
string dispatch.

## Entry-Skip Cutover

The existing `(entry_skip, 15)` identity and its wire enum are frozen,
including legacy `Unclassified` decoding. They become decode-only.

The richer entry-skip record receives a new registered `kind` and a dedicated
wire enum that contains every approved typed reason and no `Unclassified`.
The runtime semantic reason converts exhaustively to that new wire enum. It is
impossible to encode a new reason under `(entry_skip, 15)`.

The changed reason enum is not present in `strategy_input_snapshot`; therefore
that record kind does not receive a new identity in this slice.

## Recovery and Deployment

The first executable containing this change must:

1. open a legacy-only evidence stream through the new registry;
2. recover every registered consumer to the same state as the existing reader;
3. stop before strategy activation on an unknown or relevant corrupt identity;
4. append the new entry-skip identity only after recovery succeeds;
5. recover the resulting mixed stream identically after kill/restart.

Once the new identity has been appended, an older executable must fail before
live activation. Repository policy already makes rollback a pause or forward
fix; the cutover must state and test this consequence explicitly.

The existing evidence byte ceiling and file-order reduction semantics remain
unchanged.

## Required Evidence

- byte-exact deterministic registry generation;
- append-only compatibility validation for registry rows;
- captured historical records for every compatibility path exercised by this
  change;
- strict round-trip and shape-rejection tests for every new identity;
- exhaustive runtime-to-wire and wire-to-recovery conversions;
- all-legacy first-start recovery equivalence;
- mixed historical/current stream recovery equivalence for every consumer;
- unknown identity, malformed relevant payload, invalid gate ownership,
  duplicate discriminator, truncation, and downgrade fail-closed tests;
- malformed registered-irrelevant payload leaves the active recovery consumer
  unchanged;
- no remaining global-current or ordered schema comparison in reader paths;
- no Python migration lane after Rust historical coverage is complete.

## Remaining PR #1470 Corrections

After the identity foundation is green, #1470 still must:

1. replace the opaque RV-source novelty dimension with a generated typed
   source-map shape, configuration-derived roster authority, roster-drift
   rejection, and exact symbolic cardinality;
2. replace the entry-skip boolean result with a typed disposition that never
   reports `AttemptFailedAndRetained` as recorded or appended.

These corrections do not weaken the already accepted
`EvidenceEpisodeId -> Set<CompleteTypedSemanticKey>` model.
