# Strict sccache Runtime Reconciliation Plan

> **Execution:** Use `superpowers:executing-plans` task by task. Preserve the existing branch and commits. Do not publish, merge, or edit PR #1494 until the gates below authorize that action.

**Goal:** Reconcile the existing PR #1497 implementation with the approved design at `e66cf11f6`, producing one reproducible ARM64/X64 sccache v0.16.0 derivative whose typed configuration, compiler environment, request classification, child-launch, transport, storage, and publication boundaries fail closed without silently bypassing the cache.

**Architecture:** The derivative owns one canonical `ConsumerConfigSnapshot` materializer/encoder/schema/loader, one typed `StrictClientControls`, one irreversible family-aware `CompilerEnvironment`, one retained `ClassifiedRequest`, one private `StrictChildLauncher`, and one response-commit/terminal-poison authority. The repository owns one digest-pinned upstream patch, one strict build-provenance TOML, one verifier, one cacheless build recipe, one release workflow, and one runner registry. PR #1494 remains the consumer/adoption change.

**Authoritative files:**

- `docs/superpowers/specs/2026-07-21-strict-sccache-runtime-design.md`
- `ci/sccache-strict/sccache-v0.16.0-strict.patch`
- `ci/sccache-strict.toml`
- `scripts/sccache_strict.py`
- `scripts/test_sccache_strict.py`
- `scripts/build_strict_sccache.sh`
- `.github/workflows/sccache-strict-release.yml`
- `ci/github-actions-runners.toml`

## Global gates

- Preserve the local branch and its ahead commits. Never reset, recreate, force-push, or overwrite unrelated runner mappings.
- Validate the patch only in the pristine `/private/tmp/sccache-v0.16.0-review` tree at upstream commit `b799af2eea02bba9e0ef2550775fe10296b62981`. Require a clean tree before application, exact forward application, and exact reverse application after all tests.
- Runtime values come from TOML. Code owns schemas, variants, types, and semantic allowlists; it owns no alternate runtime defaults or environment fallbacks.
- Tests exercise behavior, protocol, process, transport, storage, and filesystems. Add no source-scanning tests.
- Use remote-first Rust verification. Cheap Python, TOML, patch, formatting, and actionlint checks run locally. Native architecture suites and replica builds run in the dedicated workflow.
- Every rejection must happen at the earliest governed boundary. Environment errors are fatal; only truthful `NotCompilation` and typed `CannotCache` authorize exactly one classified client execution.
- Do not merge #1497 until #1495 has native approval and is merged, then mechanically rebase #1497 and preserve both workflow runner mappings.
- Do not remove the temporary design and plan until implementation findings and exact-head evidence are resolved. Their removal must be a behavior-neutral commit followed by a full evidence rerun and fresh code-only review.

---

### Task 1: Establish the exact implementation delta and requirement ledger

**Files:** Read-only inspection of all authoritative files and the pristine patched upstream tree.

- [ ] Record the local head, upstream head, merge base, clean status, and complete changed-file list without changing branch history.
- [ ] Verify the current patch applies to the pristine pinned tree and capture its upstream file list with `git diff --name-only` inside that tree.
- [ ] Build a requirement ledger from the approved design with one row per boundary: snapshot, controls, raw environment, retained request, launcher, jobserver, poison gate, storage/output, reproducibility, publisher, and positive capability.
- [ ] Map every row to implementation symbols, behavior tests, workflow evidence, and unresolved gaps. Treat missing or ambiguous evidence as unfinished work.
- [ ] Confirm the PR owns only the standalone derivative and immutable publisher. Flag consumer adoption, install pins, storage location, runtime values, and job-wide wrapper enforcement as #1494 scope.

**Evidence:** clean branch/status record; exact patch forward/reverse baseline; changed-file list; requirement-to-symbol/test/evidence ledger; targeted `rg`/compiler inspection showing no second implementation path.

### Task 2: Make `ConsumerConfigSnapshot` the sole configuration authority

**Files:** patch, strict TOML, verifier, Python tests, upstream Rust behavior tests.

- [ ] Add failing tests for the sole materializer: it consumes adoption TOML plus governed job identity, emits one canonical bounded content-addressed snapshot to a fresh job-local destination, and refuses overwrite.
- [ ] Add failing loader tests for missing, duplicate, unknown, malformed, oversized, mutable, symlinked, non-regular, wrong-name, digest-mismatched, noncanonical, credential-bearing, and schema-incompatible snapshots.
- [ ] Implement one canonical encoder/schema/loader. The snapshot includes socket, timeouts, frame limits, compiler path/family identities, routing expectations, governed environment values, schema identity, and handshake policy, but no credentials or self-authorizing locator.
- [ ] Make server and client consume the bootstrap locator exactly once, open with no-follow bounded regular-file checks, verify the digest-derived name, and parse once.
- [ ] Construct one `StrictClientControls` from the parsed snapshot plus exact validated routing input. Pass the complete typed object to selector validation, connection, framing, handshake, and request construction; prohibit child/key access and later selector reads.
- [ ] Delete or make unreachable every later environment/filesystem socket, timeout, identity, frame, routing, or policy lookup. Reject broad SCCACHE/bootstrap namespaces and adoption-extensible control tables.

**Evidence:** materializer round trips; byte-canonical digest/name tests; mutation and no-follow negatives; exact one-consumption tests; server/client digest mismatch tests; handshake identity tests; behavioral proof that later environment/file changes cannot alter policy.

### Task 3: Close the raw compiler-environment and request boundary

**Files:** patch and upstream Rust behavior tests.

- [ ] Add failing raw Unix environment tests before upstream filtering for Rust and C-family requests: duplicate byte names, missing required names, governed mismatch, malformed bytes, credentials, interposition/search state, per-run metadata, unknown names, and routing/control leakage are fatal before connection.
- [ ] Define one closed code-owned family schema plus exact snapshot-governed names. Separate only the verified bootstrap locator, exact routing entries, and exact `CARGO_MAKEFLAGS` orchestration name before irreversible construction of `CompilerEnvironment`.
- [ ] Preserve each admitted compiler name/value byte-for-byte and unconditionally key it in deterministic name order using one domain-separated, presence-tagged, length-framed canonical component. Rust env-dep may add absent dependencies but may not remove or replace this component.
- [ ] Prove `option_env!` distinguishes absent, present-empty, and present-value state; prove `RUSTC_BOOTSTRAP` and all governed Rust/C values affect keys and behavior; prove outer run metadata, credentials, socket, and bootstrap state never enter compiler children or keys.
- [ ] Retain `ClassifiedRequest` locally and bind every response to its request digest plus snapshot digest. Only `Cacheable` enters server compilation/storage. Truthful `NotCompilation` or typed `CannotCache(reason)` consumes the matching retained request exactly once with original argv and classified I/O under `CompilerEnvironment`.
- [ ] Remove raw-environment recovery, environment-driven `CannotCache`, legacy unhandled execution, EOF/IPC fallback, server auto-start, and request rebinding.

**Evidence:** Rust and C raw-environment/key tests; procedural-macro environment fixture; absent/empty/value tests; request/snapshot digest mismatch and replay tests; exactly-once client-execution tests; EOF/server-death/error negatives.

### Task 4: Centralize compiler selection, classification, and child launch

**Files:** patch and upstream Rust behavior tests.

- [ ] Reject compiler-named-symlink dispatch and require an absolute byte-exact configured selector before lookup, canonicalization, filesystem inspection, connection, or client execution. Server independently validates the same selector/family.
- [ ] Probe only the configured executable with fixed family arguments and the purpose-tagged empty `IdentityProbeEnvironment`. Cover Rust, GCC, Clang with `--no-default-config`, mismatch, poisoned search/interposition state, and the transparent-wrapper residual.
- [ ] Complete the byte-oriented pre-parse argument table and first-match reason precedence for response files, Clang config/defaults, GCC specs/wrappers/tool/plugin indirection, unknown flags, compile-and-link, preprocessing, and genuine non-compilation.
- [ ] Run Rust output discovery as the first request-specific subprocess under the shared classification deadline/ceiling, then complete destination classification before `CompileStarted`, hashing, storage, or compilation.
- [ ] Make `StrictChildLauncher` the only constructible governed compiler spawn path. Its sealed capability accepts only empty `IdentityProbeEnvironment` or `CompilerEnvironment`, typed argv/cwd, and classified I/O.
- [ ] The launcher uses `env_clear`, installs exact canonical entries, configures standard streams explicitly, and closes all other descriptors. It never invokes jobserver configuration or propagates `MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS`, or jobserver descriptors; it may retain an opaque scheduling permit.
- [ ] Vary `CARGO_MAKEFLAGS` values and referenced descriptor numbers between otherwise identical jobs. Prove the wrapper accepts and discards it without parsing/logging, child descriptors are closed, keys are identical, and the second invocation is a genuine hit.

**Evidence:** selector/path/family tests; raw-byte argument matrix and precedence tests; command-order logs; launcher capability tests for every child class; `/proc/self/fd` child census; jobserver variation/key/hit test; representative Rust and C `Cacheable` paths.

### Task 5: Bound process groups and linearize transport poison

**Files:** patch, strict TOML, and upstream Linux behavior tests.

- [ ] Put identity probes, Rust output discovery, dep-info, preprocessing, and server compilation into launcher-owned dedicated process groups with one absolute classification deadline, shared output-byte reservation, termination grace, cleanup deadline, and exact-child reap.
- [ ] Test hangs, combined exact-limit/limit-plus-one output, cancellation, leader-exits-first, descendants retaining pipes, ignored `SIGTERM`, group `SIGKILL`, zombie members, vanished processes, PGID reuse fencing, `/proc` ambiguity, cleanup exhaustion, and server reuse only after confirmed quiescence.
- [ ] Implement one atomic poison-pending transition that blocks new response commits, closes the listener, cancels accepted/in-flight requests, and proceeds to terminal nonzero daemon poison on cleanup uncertainty.
- [ ] Put response-gate acquisition, bounded frame serialization/write, socket abort, and terminal transition under the same existing absolute cleanup deadline. Only a frame whose final framed byte reaches the transport before terminal poison succeeds.
- [ ] Test two clients with one accepted and one in flight, barriers before each authority frame, partial writes, full writes just before poison, poison while the gate is held, and a real Unix-stream slow reader with a reduced send buffer.
- [ ] On deadline expiry, terminate nonzero immediately without waiting for gate ownership, cooperation, destructors, or a second timeout. Prove the poisoned daemon accepts no later work and emits no later complete authority.

**Evidence:** deterministic process-group fixtures; no-zombie/no-live-member assertions; bounded wall-clock results; two-client and slow-reader frame tests; full-versus-partial commit results; daemon exit status and no-later-authority tests.

### Task 6: Reconcile strict storage, output, and deadline semantics

**Files:** patch, strict TOML, and upstream Rust behavior tests.

- [ ] Require exactly one governed S3 backend and configured mode at startup. Reject missing/alternate/multilevel/disk/direct modes, capability failures, rate limiting, and read-write demotion.
- [ ] Treat only authoritative S3 `NotFound` as a miss. Make read errors/timeouts, corrupt members, missing stdout/stderr, IPC errors, and unexpected responses fatal before alternate execution.
- [ ] Include the strict cache-format token in both governed compiler hashers. Always write named stdout and stderr members, including empty payloads.
- [ ] In read-only mode compile a genuine miss without any `put`; in read-write mode withhold success until the required write succeeds. Required write failure/timeout returns nonzero.
- [ ] Classify nontransactional destinations before `CompileStarted`. Stage every hit output through pinned parent descriptors, validate all members before mutation, perform descriptor-relative same-directory commit/rollback, and make every operational or rollback uncertainty fatal and quiescent.
- [ ] Route startup, IPC frame, cache read, required write, classification, termination, cleanup, response commit, and retry arithmetic through the opaque checked deadline API and TOML-owned ceilings. Reject zero, malformed, overflow, and out-of-range values.

**Evidence:** storage capability/mode tests; genuine hit/miss tests; empty-member round trip; corrupt/missing member negatives; read-only zero-put and read-write completion tests; output destination/staging/rollback matrix; deadline and overflow tests on both Linux architectures.

### Task 7: Reconcile the canonical build, verifier, and immutable publisher

**Files:** strict TOML, verifier, Python tests, build recipe, release workflow, runner registry.

- [ ] Make strict TOML the sole owner of source/patch/toolchain/executor/feature/target/replica/build/runtime-ceiling/publisher/retry values. Strictly reject missing, duplicate, unknown, malformed, unpinned, or internally inconsistent configuration.
- [ ] Make the canonical build recipe extract outside the repository, verify source digest, require a non-empty zero-context patch file list, prove forward/reverse application, build the debug test binary, run the complete patched library suite, then build network-disabled with fresh cacheless state.
- [ ] Produce two isolated native replicas per ARM64/X64 architecture. Bind manifests to exact head, run, attempt, architecture, replica, source, patch, workflow, recipe, executor, toolchain, settings, and binary digest; require per-architecture byte equality.
- [ ] Verify ELF architecture, static/no-interpreter/no-shared-library properties, host execution, strict startup negatives, abstract-socket lifecycle, and no filesystem socket.
- [ ] Keep builders and verification unprivileged. Publisher receives write permission only inside the configured protected environment and executes no repository-controlled code with its token present.
- [ ] Verify and invoke only the TOML-pinned absolute GitHub CLI. Atomically reserve the content-derived tag; create/upload/verify an owned draft; publish only from exact-current `main`; require immutable readback, attestation, exact assets/digests, and current-main recheck.
- [ ] Implement bounded known-ID cleanup and unknown-ID release census with same-origin unvisited continuation URLs, configured or repository-ID-bound canonical paths, decoded exact query validation, uniqueness, and permanent tag preservation. Never automatically delete an immutable result or tag.
- [ ] Preserve both #1495 and #1497 entries in `ci/github-actions-runners.toml` after the post-#1495 mechanical rebase.

**Evidence:** Python configuration/manifest/publisher tests; actionlint; shell syntax/format checks; live read-only releases pagination probe; four-candidate workflow artifacts; byte equality; host/ELF/static checks; exact-head verified bundle; publisher negative matrix.

### Task 8: Resolve findings and gather exact-head evidence

**Files:** only authoritative implementation files unless a review identifies an in-scope defect.

- [ ] Run targeted Python tests, Ruff, actionlint, TOML parsing, shell checks, patch forward/reverse checks, and `git diff --check` locally.
- [ ] Run the complete patched upstream Rust suite in the pristine tree for each governed target through the canonical build path, or use the repository's remote-first workflow when local execution is economically inappropriate.
- [ ] Obtain four cacheless native replica results, exact byte equality per architecture, positive Rust/C cacheability and genuine second/cross-job hits, transport/process/storage negatives, and publisher verification at the exact branch head.
- [ ] Perform an internal adversarial review of the complete diff against the requirement ledger. Every unique substantive issue is a finding; fix or obtain explicit in-scope risk acceptance, then rerun affected evidence.
- [ ] Inspect thread-level PR review state and resolve every applicable thread. Commit and push fixes before further review discussion.

**Evidence:** exact commands/results and artifact/run IDs recorded outside the stable PR body; updated ledger with no unresolved row; clean worktree; exact head SHA.

### Task 9: Integrate governance, remove scaffolding, and hand off review

- [ ] Wait for native approval of PR #1495. Merge it only through native controls; never use admin merge.
- [ ] Rebase #1497 mechanically onto the resulting `main`, resolve only the runner-registry overlap, preserve both mappings, and rerun every affected check.
- [ ] When all implementation findings and evidence are resolved, delete this plan and the temporary design in one behavior-neutral commit. Compare the code/config/workflow diff with the last reviewed implementation head.
- [ ] Rerun all exact-head native/static evidence and obtain fresh independent code-only review. Resolve and rerun until no substantive finding remains.
- [ ] Push with plain `git push`, report the exact head SHA, request the reviewer resolved from node ID `U_kgDOEZMFhA`, and detach without waiting on advisory CI.
- [ ] Merge only after native approval and live ruleset verification. Then dispatch publication for exact-current `main`, verify the immutable release, attestation, asset IDs, asset digests, tag target, and archive continuation behavior.
- [ ] Only after immutable assets are proven may PR #1494 resume and pin them. Missing or ambiguous publication evidence keeps #1494 blocked.

## Completion boundary

PR #1497 is complete only when the code-only exact head has clean static/native evidence, four verified cacheless replicas, required native approval, and no unresolved findings. The roadmap remains incomplete until #1497 is merged, its immutable assets are published and verified, and PR #1494 completes its separate adoption evidence.
