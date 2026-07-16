# Risk-Closure Verification Closure Design

## Goal

Close the repeated-review loop on PR #1430 by aligning each guarantee with evidence that can actually prove it. The Rust authority state machine is unchanged. This slice repairs compiler-negative evidence, makes the TOML authority definition finite and recursive, and replaces the misleading arithmetic-prediction claim with a structural owner boundary.

## Root Cause

The previous workflow mixed three different controls:

- Rust ownership properties were asserted through guessed diagnostic text without compiler execution.
- The TOML census used examples rather than a complete definition of an occurrence.
- A Python expression evaluator was treated as if it could recognize arbitrary Rust expressions equivalent to configured byte values.

External review consequently became the first executable or counterexample-driven specification pass. The corrected workflow defines a closed acceptance matrix before implementation and requires the applicable evidence before another external review request.

## Closed Acceptance Matrix

### Compiler-negative properties

The governed harness proves these production-configuration properties:

| Property | Probe | Stable evidence |
| --- | --- | --- |
| Reservation is not cloneable | Call `clone` | rustc `E0599` |
| Committed reservation is consumed | Use after `commit` | rustc `E0382` |
| Reservation private state is inaccessible | Directly assign `active` | rustc `E0616` |
| Recovery lease is not cloneable | Call `clone` | rustc `E0599` |
| Terminal release consumes the lease | Use after `release_terminal` | rustc `E0382` |
| Lease private state is inaccessible | Directly assign `active` | rustc `E0616` |
| Permit is not cloneable | Call `clone` | rustc `E0599` |
| Terminal release consumes the permit | Use after `release_terminal` | rustc `E0382` |
| Permit fields cannot construct a value | Struct literal | Structured rustc private-struct-construction diagnostic (rustc assigns no error code) |

The harness invokes rustc with JSON diagnostics and asserts error codes whenever rustc provides them. For the code-less private struct-construction error, it asserts the structured error level, private-field span label, and narrowly scoped message shape rather than the complete rendered diagnostic. A positive-control snippet must compile, proving that the harness, module path, edition, and production `cfg` are valid independently of the negative probes.

The lane-provided `rustc` remains the compiler. Exact-head governed nextest is the proof; local agents do not bypass the remote-first policy.

### TOML authority occurrences

An authority occurrence is any dictionary key named `risk_closure_workspaces` at any depth in any repository TOML document, including dictionaries inside arrays of tables. Each occurrence is identified by both file path and TOML key path.

The only allowed occurrence is:

```text
config/risk-closure-workspaces.toml :: risk_closure_workspaces
```

The allowed value must be a table and remains subject to the generator's exact-key, exact-schema, activation, and geometry validation. Nested occurrences in the canonical file, nested occurrences in other files, array-table occurrences, empty tables, scalar values, and capacity-only tables all fail.

### Workspace size authority

The authoritative guarantee is structural:

- the configuration type and generated constant are private to the owner module;
- workspace storage and allocation implementation are private to that module;
- production exposes no independent authority constructor, permit constructor, replacement path, or alternate configuration route;
- capacity is derived from the TOML arena and slot geometry.

Exact arena and slot integer literals outside generated Rust remain rejected as defense-in-depth. Semantic `const` or `static` names that claim a closure workspace, slot, or arena size remain rejected.

The verifier will not claim to evaluate arbitrary Rust arithmetic. The Python-AST expression evaluator and its expression-equivalence tests are removed. Expressions such as shifts, complements, symbolic composition, const functions, casts, and type-dependent overflow form an open-ended Rust-language problem and are not a sound authority boundary. A future production allocator must remain behind the private owner API and receive geometry only from the generated configuration.

## Verification Sequence

1. Add regressions and observe the intended failures.
2. Implement the compiler harness, recursive TOML enumeration, and structural-fence wording.
3. Run local formatting, Python verifier tests, dependency policy, CI lint, and `source-fence-static`.
4. Conduct an internal adversarial review against this acceptance matrix.
5. Commit and publish a clean draft head.
6. Obtain exact-head governed Rust evidence, including the compiler-negative harness.
7. Only after applicable exact-head evidence is green, prepare another external review request.

Any pushed correction invalidates prior Rust evidence and returns the sequence to step 6. Review does not substitute for an unfinished executable gate.

## Scope

This changes verification and tests only. It does not add production authority construction, durable-transition permit issuance, Capsule persistence, resource-ledger integration, production activation, deployment, merge authority, or trading authority.
