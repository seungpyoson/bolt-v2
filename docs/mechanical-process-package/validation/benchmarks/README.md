# Held-Out Benchmark Corpus

This directory freezes the held-out seeded fault families registered in:

- [2026-04-20-benchmark-manifest-v1.toml](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-validation-protocol/docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml:1)

These benchmark descriptors are protocol artifacts.
They do not execute the faults themselves.

Each descriptor freezes:

- the base subject head
- the fault family
- the protocol-owned fixture and runner
- the mutation surface
- the expected fail-closed outcome
- what must not be redefined after seeing results

Future scientific-validation descriptors should follow:

- [descriptor-template-v1.toml](/Users/spson/Projects/Claude/bolt-v2/.worktrees/issue-208-validation-protocol/docs/mechanical-process-package/validation/benchmarks/descriptor-template-v1.toml:1)

They must not:

- use `tests/**` from the subject as the held-out fixture source
- cite subject-authored tests as the held-out evidence layer
