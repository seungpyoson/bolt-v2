# CI Optimization Findings — 2026-06-01

> **Partially superseded (2026-06-27, #1011).** The "sccache — BLOCKED" finding
> and the "leave CI as-is" recommendation below are out of date. The hermetic
> env-scrub that blocked sccache is now resolved by a TOML-governed, CI-only
> `RUSTC_WRAPPER` opt-in (`[remote_compile_cache]` in `ci/rust-verification.toml`
> + `managed_env`), so sccache no longer silently no-ops. Measured on current code
> (same-runner A/B): a cold `nextest archive` build drops ~30% (1516s → 1053s,
> 100% cache hit) with zero overhead on a miss. sccache is now wired into the
> required `test-archive` job as a cold/evicted-target backstop (writes only on
> trusted refs; fail-open). The managed-target lint claim below — that
> `restore-keys` is forbidden on test-archive — is also stale; that cache has
> since been added. See #1011 for the production wiring and evidence.

Investigation into whether the bolt-v2 CI pipeline can be further shortened or
made cheaper. Five adversarially-verified passes were run: (A) per-run speed,
(B) how often the heavy release build runs, (C) Tier-2 paid infrastructure
(faster runners / warm caches), (D) "over-deliver" — what becomes possible if we
are willing to *evolve* a current restriction, verified to never weaken
deploy-trust, hermeticity, the full-suite gate, or release integrity, and (E)
"fully unconstrained" — whether the no-step-change conclusion survives dropping
*all* limits (safety, cost, architecture). Every load-bearing claim below was
re-read at HEAD against the cited file/line, commit, or GitHub Actions run.
Tracking: #518.

## TL;DR

The CI is already heavily and deliberately optimized. Across all three passes,
**no option is a free win with material wall-clock impact.** The biggest levers
are blocked by a tested design lint, by a committed fail-closed deploy-safety
decision, or cost money for a modest, diminishing-returns speedup. A dedicated
Tier-2 pass (warm-cache / faster-infra providers) found **0 viable options** —
the cache-based ones are blocked by the hermetic build, and the persistent-disk
ones conflict with disk governance and cost money for $0 benefit while public.

The over-deliver pass (D, beyond current restrictions) surfaced **one** genuine,
safety-preserving speed lever — a warm dependency cache on the test-build lane,
mirroring a pattern four other lanes already use — but its multi-minute win is
**unproven until measured** and it costs a committed-spec amendment (012 FR-005)
plus the external-review gate. Beyond it sit several **$0 safety/observability
hardening** options that do not touch wall-clock. Still **0 free-and-proven big
wins**: every speed lever needs measurement to confirm, or is hardening, not
speed.

A final fully-unconstrained pass (E) confirms the no-step-change finding is **not
an artifact of the safety constraints**: dropping *all* limits (safety, cost,
architecture) still yields only ~40% (~9m20s → ~5.3 min) and only by stacking
three levers — because the floor (~170–220s) is the dependency-graph compile +
relink + booting NautilusTrader's runtime in the heaviest test, i.e. work that
must happen, not a rule we imposed. No magic-bullet exists.

**Recommendation: leave CI as-is.** If speed ever justifies a small recurring
bill once the repo is private, the only endorsed lever is larger GitHub-hosted
runners on the build + test lanes (Tier-1) — and measure before committing.

## Measured baseline (the common case: PR, source changed, build skipped)

Source: runs `26755741834` and `26753432142` (per-job + per-step timing).

Total wall-clock ≈ **9m20s**. Critical path:

```
detector 8s
  → nextest archive 5m25s   (the "Build nextest archive" step = 4m40s, a clean compile)
  → slowest test shard 3m24s (the "test" step = 2m56s; ~47s imbalance across 4 shards)
  → test (aggregate) 4s
  → gate 3s
```

Everything else runs in parallel **inside** the archive window and is off the
critical path: fmt-check 3m15s (Setup-env 2m26s = the `ci-lint-workflow` Python
battery), source-fence 2m0s, check-aarch64 53s, clippy 39s, deny 31s.

Only two things move wall-clock: the **4m40s test-binary compile** and the
**~3m shard run**.

Billed cost (GitHub bills total job-minutes, not wall-clock; ~13 jobs in
parallel): build-skipped PR ≈ 18 billed min; main push (build runs) ≈ 28 warm /
~83 cold. Standard Linux runner ≈ $0.008/billed-min — **free on public repos**,
free monthly allotment then billed on private.

## Part A — per-run speed

### Actionable (small)
- **Cache the `just` binary download** (`setup-environment/action.yml:72-121` re-downloads it via curl on every job). ~2s wall-clock, free, no tradeoff. Trivial.

### Real levers, but a cost/scope tradeoff (not free)
- **Larger runner for the archive lane only** — est. ~80s. Larger GitHub runners bill 2–4×/min. Breaks no lint (cache keys use `runner.os/arch`, not the `runs-on` label). `ci.yml:292`.
- **6–8 shards instead of 4** — est. ~35s, but +50–100% test runner-minutes and a coordinated edit across ~9 lint-pinned sites. Capped by the `live-node` serialized-test floor.
- **mold/lld linker** — est. ~8s, one-time full-cache bust, risk to the release link. Marginal.

### Blocked / already optimal (do not re-propose)
- **Incremental target cache for the archive lane** — the obvious fix (other lanes have it). **Forbidden by a tested lint**: `verify_ci_workflow_hygiene.py:5566-5577` fails the build if test-archive contains `include-managed-target-dir:` or `restore-keys:`. This encodes the #384 single-writer-cache design (spec 012 FR-004/005/006). The historical full-test target cache was also large (~1.5–1.7 GB current shape; an older shape measured ~7.1 GB — `ci-baseline-2026-05-15.md:124,177`) against GitHub's ~10 GB/repo cache ceiling already shared by ~6 cache families.
- **Compile profile tuning** — already optimal (`Cargo.toml` only customizes `[profile.release]`; test defaults are codegen-units=256/opt-level=0; `CARGO_PROFILE_TEST_DEBUG=0` already set; `CARGO_INCREMENTAL` deliberately scrubbed for hermeticity).
- **Shard rebalance (count→hash / split live-node)** — `count:` deliberately chosen (specs/006 research.md); live-node `max-threads=1` is a real concurrency lock (`tests/nt_runtime_capture.rs` global mutex), not a tuning knob.
- **fmt-check Python battery, redundant compile, aarch64 dedup, path filters, detector, concurrency, tag-reuse** — all already optimal or off the critical path.

## Part B — how often the heavy release build runs

### The frequency, measured
- **Deploy (what consumes the build): ~5 times in the whole project history** — only `v*` tags deploy (`v0.1.0` + four `v0.0.0-smoke-*`).
- **Main-push builds: ~30 in 11 days** (2026-05-20..05-31), bursty (e.g. PRs #481+#482 merged within 1 minute). Every main push runs the full ~20-min release cross-compile because the detector forces `build_required=true` on push (`ci.yml:89-90`).
- Ratio ≈ **30 heavy builds : 1 deploy** — most main release builds are never deployed.

### Why this is intentional (and correct for a money-handling system)
The design builds the exact deployable binary on every main commit, validates it
there, and **never rebuilds at release time** — a tag deploy reuses the exact
artifact a trusted main run produced (`find_same_sha_main_evidence.py`). This is
a committed fail-closed deploy-trust property:
- `specs/008-ci-same-sha-smoke-dedup/research.md:11` — *"Rebuild on missing evidence: rejected ... violates fail-closed acceptance"*; *"tag runs no longer build/upload the artifact."*
- Enforced by `find_same_sha_main_evidence.py` (`REQUIRED_NON_TEST_JOBS` includes `build`) + the gate/deploy lints in `verify_ci_workflow_hygiene.py`.

### Levers examined — 0 recommended, 7 user-decision, 13 rejected
- **Build on tags only / relax push-force-build** (~85% fewer builds, ~1200 billed-min/mo) — **release-breaking** without a tag-time rebuild, and tag-time rebuild is the exact thing specs/008 rejected. Reversing it weakens the "only deploy a main-validated binary" guarantee.
- **Merge queue** — adding `merge_group` alone saves zero and adds ~80 builds/mo; the version that saves (~5–6 builds/mo from collapsing bursts) is blocked because the deploy reuse path requires `event=="push" && head_branch=="main"` provenance (`find_same_sha_main_evidence.py:59-60`). Not worth it.
- **Skip tests on draft PRs** — was blocked by the then-current disk-pressure policy, which made the draft/open PR the *designated* broad-verification path while local broad testing was disk-governed; the gate required `test==success`.
- **Dependabot weekly→monthly** — not a CI change (a separate bot that merely triggers builds); ~4 fewer builds/mo, security still covered by the weekly advisory cron. Trivial.

## Part C — Tier-2 paid infrastructure (faster runners / warm caches): 0 viable

A dedicated pass evaluated Depot, Blacksmith, Namespace (+ Buildjet/Ubicloud/
WarpBuild), plain sccache on GitHub runners, and self-hosted runners.

### sccache-based caching (Depot's Rust cache, plain sccache) — BLOCKED
These warm the build by routing cargo through `RUSTC_WRAPPER=sccache`. bolt-v2
blocks that three independent ways:
- The hermetic env-scrub strips `RUSTC_WRAPPER` (and `RUSTC_WORKSPACE_WRAPPER`, `CARGO_INCREMENTAL`) before every managed build — `rust_verification.py:59,63-64` (`SCRUB_ENV_KEYS`) + `managed_env` pops them at `:273-279`. So sccache silently no-ops on the archive lane (`justfile:123-124` routes through the managed path).
- A workflow lint hard-fails the YAML string `RUSTC_WRAPPER:` — `verify_ci_workflow_hygiene.py:5000`.
- A committed test asserts the scrub — `test_rust_verification_cache_retention.py`.

Unblocking = removing the scrub + amending two lints + rewriting a committed
test = dismantling the hermetic-build invariant. Not a config change. **BLOCKED.**

### Persistent-target-dir caching (Blacksmith sticky-disk, Namespace cache-volume, self-hosted) — NOT WORTH IT
These warm cargo's native target dir transparently (no wrapper), so they survive
the scrub and the archive lint — mechanically the only way to get the warm-cache
win. But:
- The managed target dir is rooted at `${{ github.workspace }}/.rust-verification` (`ci.yml:45`), which is **per-run ephemeral** — a sticky disk caches nothing useful unless you re-root the governed target dir (which itself touches the design).
- A persistent target dir conflicted with the then-current disk-pressure policy, which responded to an uncontrolled persistent `target/` causing an 18 GB blowup.
- **$0 benefit while the repo is public** (standard GitHub runners are free); real spend only once private.
- Self-hosted adds the GitHub-documented public-repo fork-PR code-execution risk + ~$260–300/mo AWS + maintenance.

### Security (corrected, applies to all Tier-2)
Moving the build/test lanes exposes **no** trading or deploy credentials — those
lanes run `contents: read, actions: read` (`ci.yml:47-49`) and trading secrets
resolve from AWS SSM inside the binary on EC2, never in CI. The only AWS access
is the tag-only `deploy` job's OIDC role (`ci.yml:621-664`), which stays on
GitHub-hosted. So Tier-2's real costs are vendor/supply-chain trust + design
conflict + (self-hosted) public-repo RCE — **not** a trading-safety risk.

### Tier-2 conclusion
Larger GitHub runners (faster CPU only, ~80s — "Tier 1") are the practical
ceiling. Tier-2's warm-cache promise is blocked by the repo's own deliberate
hermetic/disk-governance design; capturing it means reversing that design — not
justified for a ~$10–15/month-private / $0-public cost.

## Part D — over-deliver (beyond current restrictions)

A fourth pass asked the opposite question from A–C: not "what fits the rules"
but "what could we build even if it means *evolving* a restriction" — with every
idea adversarially verified to never weaken deploy-trust, hermeticity, the
full-suite merge gate, or release integrity. 17 ideas examined: **0
free-and-proven big wins, 1 genuine speed over-deliver (with caveats), a cluster
of $0 safety/observability hardening, the rest not worth it or unsafe.**

### The one real speed over-deliver — warm the test-build dependency cache
This is the same lever Part A listed as "blocked," correctly reframed. The block
is a *deliberately narrow* rule, not a safety wall:
- Four lanes already cache their **compiled** dependencies keyed on the lockfile
  (`ci.yml:436-441`, the `build-aarch64-release` cache — key uses `Cargo.lock` +
  toolchain + policy, **not** `src/**`, with `restore-keys`). `specs/012` **FR-005**
  enumerates exactly those four keys (`clippy-host`, `check-aarch64-dev`,
  `source-fence-test`, `build-aarch64-release`) and **omits test-archive** —
  verified at `specs/012-ci-cargo-cache-sharing/spec.md:28`.
- test-archive already owns the dependency **source** cache (FR-004,
  `spec.md:27`) but deliberately not the **compiled-artifact** cache, enforced by
  the negative lint `verify_ci_workflow_hygiene.py:5566,5576` (forbids
  `include-managed-target-dir:` and `restore-keys:` on test-archive).
- Give test-archive the same lockfile-keyed compiled-deps cache → on the common
  PR (deps unchanged) it recompiles only the single workspace crate + changed
  test binaries instead of all ~698 dependency crates, plausibly cutting the
  ~4m40s clean compile by minutes.

**Why WORTH_IT, not a slam-dunk (two caveats):**
1. **The time win is HYPOTHETICAL.** Restoring a multi-GB target dir itself costs
   ~30–90s, which partially eats the saving. It must be proven on a real
   before/after PR (`specs/012` SC-003 / FR-008 evidence discipline) — never
   claimed from the ratio alone.
2. **Not a config tweak.** It requires amending committed spec `012` **FR-005**
   to enumerate a fifth managed-target key, *flipping* the lint from "must NOT
   cache" to a positive "deps-cache key MUST contain `Cargo.lock`+toolchain AND
   MUST NOT reference `src/**`/`tests/**`" assertion (+ its unit test), plus the
   project's external-review gate (`014` FR-016). Frame the lint change as
   **strengthening**, not relaxing.

**Safety: PRESERVES all four guarantees.** It mirrors a pattern four lanes
already trust; `nextest archive --locked` (`justfile:124`) pins deps to the
lockfile; the env-scrub + single managed target dir
(`rust_verification.py:51-70,273-279`) keeps the same hermetic envelope; the full
suite still builds and runs across all four shards (no test skipping); release
codegen is untouched. Two independent agents converged on this proposal — high
confidence the mechanism is sound. (They disagreed on scope; the careful
accounting — FR-005 amendment required — is the correct one, verified above.)

### $0 hardening over-delivers (do not touch wall-clock)
- **Supply-chain attestation (SLSA L2/L3) of the deployable binary** — adds a
  third-party-verifiable, offline cryptographic binding of the binary digest,
  checked fail-closed on the EC2 host before start. **Zero CI time cost** (runs
  in the main-only build job, off the PR critical path). The work is on the
  deploy/host side (today there is no automated download/verify/start —
  `deploy/install.sh:1-77` + manual runbook); the new fail-closed start
  dependency must be dry-run before live use. Defense-in-depth on top of the
  already-strong same-SHA-main + ancestry + sha256 chain.
- **Flaky-test surfacing** — bounded `retries` paired with `flaky-result="fail"`
  (nextest 0.9.132 supports it) so a non-deterministic test is **named** FLAKY
  yet **still reds the gate** — never retried into green. **Zero cost on clean
  runs.** Forward-looking signal (suite is deterministic by design today). A lint
  guard asserting the `flaky-result="fail"` pairing is **mandatory** — the
  retries-only variant would silently weaken the gate (UNSAFE).
- **Free GitHub Actions performance metrics + cache-size monitoring** — native
  Insights (GA, $0) plus a scheduled p95 exporter + cache-total reporter to
  automate the manual baseline snapshots and close the cache-total blind spot.
  Read-only, off the gate path.

### Unblock test parallelism (bounded)
The four shards are capped by the `live-node` test-group's `max-threads=1`
(`.config/nextest.toml`). That serialization is a **resource cap, not a
correctness lock** — nextest runs each test in its own process, so NT's
process-global runtime/logger singletons are already isolated across binaries
(verified against the pinned NT rev). It can be converted to bounded concurrency
(`threads-required`), but the win is capped by the measured ~47s shard imbalance
and the heaviest single binary's own wall-time, and it needs a concurrency-
isolation reproduction + before/after timing run before being relied on.

### Considered and declined (for this repo)
- **Cranelift codegen backend / parallel front-end (`-Zthreads`)** — both
  nightly-only on the pinned stable toolchain → would force a second toolchain
  (**dual path**) + a RUSTFLAGS de-scrub exception, weakening hermeticity and
  gate-vs-release compiler parity. **UNSAFE / not viable.** Also mis-targeted:
  the cost is from-scratch *dependency* compilation, which a deps cache attacks
  directly.
- **Merge queue** — near-zero velocity benefit at this PR volume; would require
  rewriting the ~11k-line 3-layer deploy-trust core + re-deriving a P1-locked
  spec, and rests on an unconfirmed "attested-SHA == landed-main-SHA" premise.
  Not worth it.
- **Test-impact analysis** — no effect: single-crate workspace, so `rdeps`
  always selects every test binary. Nothing to narrow.
- **Reproducible-build hash check** — real top-grade trust property but open-
  ended remediation risk into release config if the binary doesn't already
  reproduce. Marginal; do only as the bounded "stand up the diff lane and
  observe" slice.
- **cosign keyless signing** — ~100% duplicative with the SLSA attestation above
  and a dual-path smell (two signature objects for one binary); pick one.
- **Source→binary SHA traceability** — **already delivered**: the trading
  entrypoint refuses to start unless operator-evidence head_sha matches the
  compiled-in `BOLT_V3_BUILD_HEAD_SHA` (`src/bolt_v3_live_canary_gate.rs`), and
  the tag→artifact→main-SHA binding is already triple-asserted. Net-new value
  ≈ cosmetic.

### Over-deliver bottom line
The warm dependency cache is the only lever that could **materially** shorten CI,
now correctly reframed from "forbidden" to "evolvable and safety-preserving — but
unproven until measured, and costing a spec amendment + external review."
Everything else is either **$0 hardening** worth adopting on its own merits (not
for speed) or not worth it for this repo's shape (single crate, single toolchain,
low PR volume, money-handling). This does not change the headline recommendation
**unless** the measured multi-minute upside is judged worth a spec-evolution PR
plus a measurement run.

## Part E — is the floor fundamental? (fully unconstrained)

To test whether the "0 step-changes" conclusion is an artifact of the four
safety guarantees, a fifth pass dropped **all** constraints — safety, cost, and
the current architecture — across six relaxed-constraint lenses (eliminate dep
compilation; infinite bare-metal hardware; relax the full-suite gate; shrink/fork
the dependency graph; attack the test-runtime floor; Bazel/Buck2 remote
execution). Each proposed step-change was then adversarially floor-checked
against the dependency-DAG critical path, link time, the heaviest-test-binary
wall-time, and cache-restore overhead.

**Result: no step-change, even unconstrained.** Of six proposals, four were
**ILLUSORY** (the speedup assumed a free cache restore; restoring a multi-GB
cache is 50–90s of real network I/O) and two were **INCREMENTAL**. The best
realistic unconstrained critical path is **~560s → ~320s (~40%, not a halving)**,
and only by *stacking* three levers — warm dependency cache **+** removing the
`live-node` test serialization **+** a much larger runner — none of which is a
step-change alone, all of which bottleneck on each other.

- **Not a new mechanism, just the old levers unshackled.** "Eliminate dep
  compile" = the Part D warm-cache lever with a worse trust model; "infinite
  hardware" / "remove `max-threads=1`" = the bigger-runner/raw-cores lever;
  "fork NautilusTrader" = dependency-elimination + deleting tests. The single
  genuinely-new technique (Bazel/Buck2 content-addressed remote caching) was
  judged INCREMENTAL: it optimizes *compilation* while the binding wall is *test
  execution* (wrong target), costs ~$20–50k/mo, and sacrifices hermeticity. So
  the constraints were making the obvious solution **expensive and unsafe**, not
  hiding a magic one.
- **The deepest irreducible floor ≈ 80–100s** — the wall-time of the single
  heaviest integration test binary booting NautilusTrader's multi-threaded
  runtime. Because each test runs in its own process and the slowest shard cannot
  finish before its heaviest binary does, no amount of money, cores, or cache
  crosses it. Add the unavoidable relink of the single workspace crate against
  ~700 deps on any source change (~60–90s) + cache/setup (~20–30s) and the true
  floor is **~170–220s** — reachable *only* by rewriting the tests to mock the NT
  runtime away, which sacrifices the production deploy-trust guarantee.
- **No magic-bullet.** bolt-v2 is one workspace crate on ~700 deps; every test
  binary links the full graph, so "skip compilation" is impossible and a
  universal prebuilt artifact (Docker layer / remote cache) only *relocates* the
  cost to a network restore, never erases it.

**Conclusion: the "0 step-changes" finding is NOT an artifact of the safety
constraints.** The bottleneck is work that must happen — compiling a large
dependency graph and booting a real trading runtime in the tests — not a rule we
chose to impose. (Floor figures are quantified projections from the dependency
graph + measured shard data, not a live unconstrained run; the ~40% / ~320s and
~170–220s-floor numbers are estimates, directionally robust across all six
lenses.)

## Recommendation
**Leave CI as-is.** It is well-tuned; the remaining wall-clock levers are blocked
by deliberate safety design or cost money for diminishing returns, and the whole
cost at stake is **$0 while public / ~$10–15/month once private**. If speed
becomes worth a small bill, run a Tier-1 larger-runner experiment on the build +
test lanes and **measure** the actual before/after.

## Verification caveats
- Wall-clock and billed-minute figures for paid options are projections from the
  critical-path data; the only way to get real numbers is to run the change and
  measure.
- GitHub Actions per-repo cache total (vs the ~10 GB limit) could not be measured
  from here (network-restricted); check live before any cache change.
- Some vendor sub-claims were marked UNVERIFIED in the source pass (e.g. Depot
  "billed by the second"); pricing was re-fetched from vendor docs where
  load-bearing. Confirm current vendor pricing before any adoption.
- Part D's headline speed number (warm dep cache cutting the ~4m40s compile by
  minutes) is **HYPOTHETICAL** — it is a ratio-based projection (~698 dep crates
  : 1 workspace crate) net of an unmeasured cache-restore cost, and must be
  proven on a real before/after PR before being claimed.
- Method: four background multi-agent workflows, each finding adversarially
  verified against the code, git history, and committed specs at HEAD; the Part
  D load-bearing facts (FR-005 enumeration, the test-archive lint, the build-lane
  cache precedent) were additionally re-read at HEAD by the main session.
