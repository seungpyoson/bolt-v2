# #927 — Cheap-Lane Shared-State Guard: Complete, Drift-Proof Discovery

**Status:** Implementation branch active; root-review fixes are part of this branch before lock. Anchored to
`main` lineage from `a59ae0776`.
**Issue:** #927 (follow-up to #924, commit `cc5e6c4f0`). **Epic:** #333.
**Provenance:** Synthesized across **seven** external adversarial review rounds (GLM / GPT / Kimi / Claude),
every load-bearing claim re-verified at source by the author. Revision **A⁗** (three-layer model;
**round-5 reversal applied**: in-process loads restored to L1 fence edges, and the broad repo-local
non-Python over-fence narrowed to a **positively-Python-shaped** unresolvable target — the broad form
false-failed clean shims and the coordinator's dynamic dispatch).

---

## 1. Problem & root cause

`scripts/test_lane_governor.py::test_cheap_lanes_do_not_write_repo_root_shared_state` (added in #924)
statically scans a set of Python scripts with an AST analyzer (`_RepoSharedStateWriteAnalyzer`) and
fails if any **writes or deletes filesystem state under the repo root**. The repo runs "cheap" local
verification lanes at **unbounded** concurrency, so any script that can run that way must use
process-private temp/cache paths, never shared repo-root files — otherwise two concurrent local `just`
runs race and corrupt shared state.

The guard's **discovery is incomplete and not drift-proof**:

- The scan set = (`.py` entries in `cheap_lane_labels`) ∪ (bare `python3 scripts/X.py` lines parsed from
  **only** the `source-fence-static-inner` recipe).
- It misses scripts reached **transitively** (a scanned script spawns another `scripts/*.py`), scripts
  reached through invocation forms other than bare `python3 scripts/X.py`, and it provides no mechanism
  to stay complete as recipes/labels change.

**This is a coverage/fragility fix, not a live bug** — every currently-reachable script is clean today
(verified). The guard is a *future-regression fence*; the defect is that the fence has silent holes.

### Root-cause class
The guard hard-codes both **which scripts to scan** (a partial, hand-assembled set) and **what counts as
a repo-root origin** (only two variable names). #927 must replace both with **derived, fail-closed**
mechanisms so the guard cannot silently under-cover.

---

## 2. Invariant

> For every Python script reachable through a **repo-declared local-verifier entrypoint** — a `justfile`
> recipe that holds a cheap lane, or runs under a held `local-gate:*` gate via ancestor re-entrancy
> pass-through, **plus the statically-resolvable code-execution edges (subprocess spawns and in-process
> module loads) those scripts reach**, **plus** directly cheap-labeled scripts — **while not serialized by
> the heavy single-flight lane**: the script must never write or delete filesystem state under the repo root.

Notes that make the invariant precise (not circular):

- **"Repo-declared"** excludes a hand-typed `python3 scripts/local_verification_gate.py <gate> -- <arbitrary>`
  — that is the user's own risk, equivalent to running any script directly. Out of scope.
- **"Not serialized by the heavy lane"** is the class boundary — **not** "unbounded." A future
  `cheap_lane_max_concurrent = 1` lane is still non-heavy and can still race another cheap lane.
- **"Filesystem state under the repo root"** matches the analyzer's actual scope (path mutations), not
  environment or in-process global state.
- The scan set is a **fixed point** over the union: seed = (justfile cheap-gate closure) ∪ (cheap-labeled
  scripts, resolved by Python-semantics — see §4), then follow resolvable code-execution edges until no
  new scripts are added.

---

## 3. Verified findings (evidence base — re-verify at HEAD `a59ae0776`)

| # | Finding | Anchor |
|---|---------|--------|
| F1 | 3 cheap public gates, each whose recipe **body** calls `local_verification_gate.py <gate> -- just <gate>-inner`, cheap lane `local-gate:<gate>`, `cheap_lane_max_concurrent=0` | recipes `fmt-check`/`source-fence-static`/`ci-lint-workflow` — coordinator call in each recipe **body** (`justfile:237,319,448`; recipe headers one line above at 236/318/447); `ci/rust-verification.toml [local_lane_policy]` |
| F2 | Runtime concurrency classifier = `cheap_lane_labels`; gated **children** run lock-free via ancestor re-entrancy pass-through (so reducing labels to a closure-mirror would mis-reclassify direct-invocation cheap scripts as heavy — **Option B rejected**) | `lane_governor.py:153,279,321-333` |
| F3 | Current guard scans **109** (reproduced at HEAD); the 3-gate **dependency**-closure is **107**, of which exactly **3** are invoked-but-unscanned (`local_verification_gate.py`, `rust_verification.py`, `test_nextest_fingerprint.py`). The two sets are **not** subset/superset: labels include direct-invocation cheap scripts outside the closure (F2), and the closure includes transitively-reached scripts outside labels — the gap is **directional** (closure-not-scanned), not a size comparison. The body-only parser returns **0** for `fmt-check-inner` and `ci-lint-workflow-inner` (dependency-only recipes) — empirical proof the closure needs `just --dump` `dependencies`, not single-recipe text | 109 reproduced; 107 / 3 from the dependency-walk enumeration Change 1 builds |
| F4 | **Spawn gap is real:** `test_nextest_fingerprint.py` (cheap closure, invoked via the bash-negated form `if ! python3 scripts/test_nextest_fingerprint.py` at `justfile:499`) spawns `scripts/nextest_fingerprint.py`, which writes files and is in **neither** the closure **nor** the labels. Child writes to caller-supplied paths (`--output-path`, `$GITHUB_OUTPUT`); the test passes tmp → not a live bug | `justfile:499`; `test_nextest_fingerprint.py:17,185-204`; `nextest_fingerprint.py:331-332,297-301` |
| F5 | **Owner is clean:** `rust_verification.py`'s only mutating calls all resolve to `root_base()` = `~/.cache/rust-verification`, **independent of the `repo` arg**; `repo_path(args.repo)` feeds reads/cwd only | `rust_verification.py:515-519,522-529,535-536,2252,3999,222-223` |
| F6 | **`repo_path(` is owner-only** (15 call sites); no other scanned script uses it (the apparent matches are a different fn `normalize_repo_path` and `Path(args.repo_root)`), so precise-origin tracking is false-positive-safe | `rust_verification.py`; `nextest_fingerprint.py:63,311` |
| F7 | **Five sub-script execution forms exist in the closure** (see §4) | A `-m`: `test_command_understanding.py:177`; B `-c`: `test_local_verification_gate.py:52,73,138,163` + `test_command_understanding.py:163-164`; C non-`.py` spawn: `test_cargo_shim.py:16,84` (`cargo-shim` is `#!/usr/bin/env python3`, clean); D file-load: `spec_from_file_location`/`SourceFileLoader` with constant paths; E module-load: `importlib.import_module("command_understanding")` `test_command_understanding.py:48` |
| **F7′** | **In-process loads MUST be L1 fence edges, not a membership hard-fail.** **Nine** load **targets** of cheap-labeled tests are **unscanned** today (not labeled, not in the closure): `lane_governor.py`, `command_understanding.py`, `cancel_obsolete_dispatch_runs.py`, `ci_provenance.py`, `ubicloud_runner_minutes.py`, `developer_tool_storage_hygiene.py`, `find_same_sha_main_evidence.py`, `require_sp_reviewer.py`, `require_resolved_review_threads.py`. (Corrected in round 6: `host_health_sampler.py` / `ai_review_deliverables.py` were dropped — their loading tests `test_host_health_sampler.py` / `test_ai_review_glm_fallback.py` are **not** in `cheap_lane_labels`, so they are not reached from a cheap-labeled seed.) A "must already be in the scan set" check would turn the guard **RED on clean code** → the resolver must **resolve→add→recurse** (the round-4 demotion was a regression) | targets grepped against `cheap_lane_labels`; load sites incl. `test_lane_governor.py:20-22,31,2098,2140`, `test_local_verification_gate.py:18-19,29`, `test_command_understanding.py:48`, `test_cancel_obsolete_dispatch_runs.py:21`, `test_ci_provenance.py:243`, `test_ubicloud_runner_minutes.py:21`, `test_developer_tool_storage_hygiene.py:26`, `test_find_same_sha_main_evidence.py:28`, `test_require_sp_reviewer.py:25`, `test_require_resolved_review_threads.py:24` |
| F8 | `eval(`/`exec(` and the dynamic code-exec family (`os.system`, `os.popen`, `os.exec*`, `os.spawn*`, `subprocess.getoutput`, `subprocess.getstatusoutput`, `pty.spawn`) appear in **0** scanned scripts → an enumeration tripwire over them is pure drift-fence, breaks nothing today | (grep at HEAD) |
| F8′ | **Parser fixtures look like execution edges but are not.** `test_command_understanding.py:203,219,235,250,277-299` hold argv-shaped **list literals** (e.g. `["python","-c","import os; os.system('cargo build')"]`, and `["python",…,"not valid python"]`) passed as **test data** to the parser-under-test — never `subprocess.run`. The **same file** also holds a **real** `subprocess.run([sys.executable, "-c", command])` at `:163-164` whose `-c` payload is built by **adjacent-string-literal concatenation** (`:155-162`) and names literal `scripts/…` paths. Edge-detection must bind to real call expressions (catch `:163`) **not** bare list literals (ignore `:277-299`) | `test_command_understanding.py:155-164,277-299` |
| F9 | `just --dump --dump-format json` exposes structural `assignments` + `dependencies` + recipe `body` fragments (fragments may be **nested arrays**, flattened recursively); it does **not** evaluate `-- just <inner>` routing or bash `if ! python3 …` forms (those are body text). The owner is reached **only** via the variable chain `rust_verification_owner := repo_root + "/scripts/rust_verification.py"` (`justfile:19`), `repo_root := justfile_directory()` (`justfile:18`), invoked as `python3 "{{rust_verification_owner}}"` (`justfile:49,241,…`, incl. inside command-substitution `$(…)` at `:481` and `-c` pipelines at `:482-483`) → a literal-text parser cannot see it | (dump inspection); `justfile:18,19,49,241,481-483` |
| F10 | #924 hermeticized exactly `test_verify_bolt_v3_runtime_literals.py` + `test_verify_bolt_v3_strategy_policy_fence.py` — the analyzer's planted-write non-vacuity floor | #924 / `test_lane_governor.py` |

---

## 4. Architecture — three-layer discovery

Static analysis **cannot** trace every way Python can execute code (`-c` with dynamic strings, `eval`/`exec`,
regular `import`). "Full fence" is therefore the wrong frame. The correct, honest architecture is three
layers; every execution form maps to exactly one. **"Python script" means Python by semantics** — a `.py`
file, a `-m scripts.X` module, or a `#!/usr/bin/env python3` repo file that `ast.parse` accepts — **not**
filename extension. The analyzer is **Python-only**: a non-Python executable (a `.sh` shim, a PATH tool)
is unanalyzable and therefore a **documented boundary** (Layer 3), never a hard-fail — hard-failing on every
clean shim would make the guard unusable (it false-fails `test_run_rust_probe.py`, `test_cargo_shim.py` today).

| Layer | Behavior | Forms (all verified in the closure) |
|-------|----------|-------------------------------------|
| **1 — Fence** | Statically resolve the target, **add it to the scan set**, recurse to a **fixed point** | `python3 scripts/X.py`; `-m scripts.X`; a **Python interpreter** (`python3`/`python`/`sys.executable`) in argv[0] whose script operand (argv[1]) resolves — incl. **non-`.py`** Python (`str(SHIM)` → `scripts/cargo-shim`); **in-process explicit loads** (`spec_from_file_location`/`SourceFileLoader` with a resolvable path; `importlib.import_module`/`runpy.run_module` with a constant name **whose `scripts/<name>.py` exists**); `-- just <inner>` routing; **seed = justfile cheap-gate closure** (via the `just --dump` assignment+dependency evaluator, Change 1) **∪ cheap-labeled scripts resolved by Python-semantics** |
| **2 — Tripwire** | **Hard-fail** the test (no silent skip) | (a) a recognized **Python** execution edge (`python3`/`python`/`sys.executable`/`-m`/a recognized loader API) whose target is **non-constant / unresolvable** in a directly-resolved expression, whose explicit wrapper target argument is undefined / conditional / multiply-bound / otherwise unresolvable, or whose omitted wrapper argument has an unresolvable default; (b) `eval(` / `exec(`, or a call to the dynamic code-exec family (`os.system`, `os.popen`, `os.exec*`, `os.spawn*`, `subprocess.getoutput`, `subprocess.getstatusoutput`, `pty.spawn`), in a scanned script; (c) an **unresolved `{{just-variable}}`** in a command position **within the cheap-gate closure** |
| **3 — Boundary** | **Documented** limit; not a Layer-2 trigger | `python3 -c <inline>` with a **non-constant** code string (no file to inspect); regular `import` / `from X import Y`, and `import_module`/`run_module` of a **constant name with no `scripts/<name>.py`** (a stdlib/installed package — module resolution, impractical without `sys.path` simulation); a **non-Python** execution target — a bare PATH command (`git`, `grep`, `cargo`, `printf`), a non-Python interpreter (`bash`/`sh`) + operand, or a repo-local non-Python file (`.sh` shim); an **opaque spawn/load** whose argv/path is parameter-bound and not resolved to a repo Python target (e.g. the coordinator/owner **dynamic dispatch** — `subprocess.run(command)` where `command` is a passed-in verifier list, whose callees are independently discovered via the just-closure + labels; temp-fixture loader wrappers that pass a dynamic path); `__import__` and functional `importlib` variants outside the loader allowlist. **Heuristic tripwire for a constant-resolvable `-c` string:** after concatenating adjacent string literals (incl. parenthesized multiline forms), if the inline code names literal `scripts/…` paths, those must be scan-set members, else fail |

**Why in-process loads are L1 fence edges (resolve+add+recurse), not a membership hard-fail:** nine modules
loaded in-process by cheap-labeled tests are **not** otherwise in the scan set (F7′). A "must already be in
the scan set, else fail" rule would turn the guard **RED on clean code today**. Fencing them (resolve the
target file, add it, analyze it, recurse) closes the class **and** stays green — the loaded modules are
clean, so they pass. A **non-constant / unresolvable** direct load target is a Layer-2 hard-fail; a parameter-bound temp-fixture loader is the documented Layer-3 opaque boundary; a constant
module name with **no** `scripts/<name>.py` is a stdlib/installed import → Layer-3 boundary (so a future
`import_module("json")` does not false-fail).

**Edge recognition binds to real call expressions** (`subprocess.run/Popen/call/check_call/check_output`,
`sys.executable` invocations, and the enumerated loader APIs) — **never a bare argv-shaped list literal** — so
the parser fixtures (F8′) at `:277-299` are not mistaken for edges while the real `subprocess.run(-c)` at
`:163` is. The recognized loader APIs are an explicit allowlist (`spec_from_file_location`, `SourceFileLoader`,
`importlib.import_module`, `runpy.run_path`, `runpy.run_module`); `__import__` and other functional importlib
variants are a documented Layer-3 boundary. **`just --dump` body fragments may be nested arrays and are
flattened recursively** before classification.

---

## 5. The plan (5 changes)

**Change 1 — Discovery as a three-layer fixed point.**
Resolve the justfile closure via `just --dump --dump-format json` with a **mini-evaluator** for the
structured dump: stitch each recipe `body` (string fragments + `['variable', NAME]` references, **which may
be nested arrays — flatten recursively**) into a command line, and evaluate `assignments` recursively —
`['concatenate', …]`, `['variable', NAME]`, `['call', 'justfile_directory']`, literals. This is **required**
so `{{rust_verification_owner}}` resolves to `scripts/rust_verification.py` (F9) — otherwise the owner is
absent from the closure. The evaluator must descend into **command-substitution `$(…)` and pipelines**
(`justfile:481-483`) and recognize the **bash-negated** form `if ! python3 scripts/X.py` (`justfile:499`, F4).
Walk `dependencies` and `-- just <inner>` routing to a fixed point — **expand all L1 edges first**, then any
residual checks. **All `just`-expression evaluation and command classification is scoped to the cheap-gate
closure**, not the whole `justfile` (unrelated recipes — `live-*`, `bte-*` — must not false-fail the cheap-lane
guard).
**Edge recognition binds to real call expressions** (`subprocess.run/Popen/call/check_call/check_output`,
`sys.executable` invocations, and the enumerated loader APIs) — **never a bare argv-shaped list literal**, so
parser fixtures (F8′) are excluded.

**Interpreter shape.** For a subprocess argv, classify by argv[0]: a **Python interpreter**
(`python3`/`python`/`sys.executable`) → resolve argv[1] as the Python script target (L1 if resolvable, else
L2); a **non-Python** interpreter (`bash`/`sh`) or any bare PATH command → Layer-3 boundary (the operand is
not analyzed — anchor `test_run_rust_probe.py:75`, `["bash", str(SCRIPT_PATH)]` → `.github/scripts/run-rust-probe.sh`,
cheap-labeled and clean). An **opaque** argv that is a passed-in variable (not positively Python) → Layer-3
boundary; the coordinator (`local_verification_gate.py:51`) and owner dynamic dispatch fall here, with their
callees discovered independently via the closure + labels.

**Target resolution grammar** (subprocess argv, load paths, and origins): literal string; `str(EXPR)`/`Path(EXPR)`;
a `Name` looked up in **the innermost enclosing function scope, then each outer (enclosing) function scope,
then module scope** (`global`/`nonlocal` resolved at the declared scope; **class and comprehension scopes do
not resolve**; a **single unconditional textually-prior** binding only — anchor: `script` bound in the method
at `test_clean_merged_artifacts.py:1185` and used in the nested `run_in_nongit` at `:1188`); `BinOp(/)`,
`.joinpath()`, **`.parent`, `.parents[N]`** chains — all recursively; a module-name (`import_module`/`run_module`)
mapped to `scripts/<name>.py` **only when that file exists**. **Zero, multiple, or conditional `Name` bindings →
Layer-2 tripwire (fail closed) when the `Name` is in a directly-resolved target expression.**
A loader/spawn invoked through a **trivial wrapper** (a local helper whose path/name **parameter** flows into
the target) resolves one hop from either a constant call-site arg **or a constant default parameter value**
when, and only when, the call omits that arg. An explicit wrapper argument shadows the default; if that
argument is undefined, conditional, multiply-bound, or otherwise unresolved, it is Layer 2, not a fallback
to the default and not an opaque boundary. Only a wrapper argument proven to derive from a function parameter
or temp-fixture source is Layer 3. This **explicitly includes sibling-path construction from the parameter** —
`DIR / f"{name}.py"`, `SCRIPTS_DIR / f"{name}.py"`, `Path(__file__).with_name(f"{name}.py")` — where the
parameter is the **sole** non-literal (anchors: the guard's own `_load` at `test_lane_governor.py:20-22`
and `test_local_verification_gate.py:18-19`, called with constant names `_load("rust_verification")` /
`_load("lane_governor")` / `_load("local_verification_gate")`; default-parameter loaders at
`test_find_same_sha_main_evidence.py:25`, `test_verify_ci_workflow_hygiene.py:167-194`).
The trivial-wrapper rule **supersedes** the general f-string exclusion when the sole non-literal is the resolved
call-site arg/default value. Anything outside this grammar either hard-fails when it is a directly-resolved
positively-Python expression. The direct and wrapper paths must classify the same unresolved target the same
way; only proven dynamic parameter/temp-fixture provenance is recorded as the Layer-3 opaque boundary.
`lane_governor.py` is **untouched** — no runtime concurrency change.

**Change 2 — `just --dump` fail-closed.**
Within the cheap-gate closure: non-zero exit, invalid JSON, missing required keys, an unrecognized `just`
assignment/dependency expression, or a floor breach → **fail the test**.
**Hard floor — a committed name-manifest, not a re-derivation.** The floor is an explicit, committed list of
every **discovered-but-unlabeled** script (the scripts found only by the closure-walk + L1 fences, **not** by
the label seed). The implemented discovery **emits this set once**; it is committed as literal names and the
test asserts **live discovery ⊇ the manifest** — a walk/fence regression (e.g. a future refactor stops
recognizing the in-process-load idiom) drops a script from live discovery but **not** from the manifest → RED.
This is walk-**independent** and therefore non-vacuous; it is **not** a second discovery mechanism (it
re-derives nothing — it is a frozen static oracle changed only by deliberate human commit) and **not** a
frozen count (a count names no script and breaks on legitimate adds). It supersedes the round-5 "every
unlabeled closure script" phrasing, which was **self-vacuous** — re-deriving the expected set from the same
walk it protects means a walk break drops a script from both sides and the check passes (the identical flaw
the spec attributes to the superset oracle).
**Committed manifest (verified at HEAD; the implementation's emit completes it):** 20 literal names today.
It contains the 3 just-closure entries (`local_verification_gate.py`, `rust_verification.py`,
`test_nextest_fingerprint.py`), fence children (`nextest_fingerprint.py`, `clean_merged_artifacts.py`,
`cargo-shim`, `install-cargo-shim`), the nine F7′ in-process-load targets, the install-unit load chain
(`render_install_unit.py`, `test_verify_install_unit_generated.py`, `verify_install_unit_generated.py`), and
the default-parameter loader target `sync_ci_debug_ssh_secret.py`. The manifest file, not this prose count,
is the floor source of truth. (A "superset of the old discovery" oracle is rejected as vacuous — labels seed
the new set, so it can never shrink below them.) A coarse closure-size minimum is **advisory only** (warn,
never hard-fail).
**Completeness = command classification, not token-spotting:** over the scoped structured dump
body+assignments (descending into `$(…)` and pipelines), classify **every command position** as
{no Python execution | recognized Layer-1/2/3 Python execution | unsupported dynamic shell execution}; the
last → hard-fail. An unresolved variable, an unrecognized interpreter wrapper (`python`, `${PYTHON}`,
`env python3`, an alias), or a shell-expanded command position hard-fails. Shape-assert the dump; do **not**
pin the `just` version.

**Change 3 — Derive gates from the coordinator call, not the label suffix.**
For each recipe, extract `<gate>` from its `local_verification_gate.py <gate> --` body line; require a matching
`local-gate:<gate>` label, a **non-private** recipe, and a real `<gate>-inner`. Fail on **any** disagreement
either direction (label-without-recipe, recipe-without-label, **a private recipe that invokes the coordinator**,
wrong gate name, missing inner). Assert recipe-name == gate-name unless listed in an explicit alias constant.

**Change 4 — Analyzer precise-origin (close the vacuity gap), by expression not name.**
Extend `_RepoSharedStateWriteAnalyzer` to treat as a repo-root origin — besides `REPO_ROOT`/`SCRIPTS_DIR` — any
variable bound **one hop, in function or module scope** from `repo_path(...)` **or** from
`Path(__file__).resolve().parent` / `.parents[N]`, **regardless of the variable's name** (name-keying would
leave a vacuity, e.g. a write to a `SCRIPT_DIR` bound module-scope from `Path(__file__)…` at
`test_cargo_shim.py:15`, `rust_verification.py:22`). **Not** the bare name `repo` (a synthetic tmp dir across 100+ test
scripts → false-positive cascade). **Proof gates exercise** direct (`repo/"x"`), assigned (`t = repo/"x"`),
chained (`repo/"a"/"b"`), `.parent`, and `.joinpath()` repo-derived writes — verify the existing origin
propagation through path methods (`test_lane_governor.py:264-267`) actually catches them, do not assume.
(a) a planted write **is** flagged in each form; (b) the real owner yields **zero** findings; (c) **no new
findings** appear across the existing scan set. **Documented misses** (zero current mutating paths): aliasing
across statements, repo passed as a function parameter, and `Path(args.repo_root)` / `Path(args.X)` arg-derived
roots — these are safe today **only because such scripts are invoked with tmp paths**; that assumption is
recorded here, not silently relied on. **Fallback is a hard decision point, not routine:** if (a) fails, fix
the implementation; if (c) surfaces a finding outside the owner, investigate it. Documented-boundary +
"owner writes only under `root_base()`" assertion is the last resort.

**Change 5 — Reword the invariant** exactly as §2 (code-execution edges incl. in-process loads; repo-declared
scope; "not heavy-serialized"; fixed point over the union). Publish the recognized invocation-form inventory
as a single module constant.

---

## 6. Acceptance criteria

1. **Discovery completeness (committed manifest, not a count):** live discovery **⊇ the committed
   discovered-but-unlabeled manifest** (Change 2) — asserted **by name**, walk-independent. The manifest
   provably contains the coordinator, the **owner** (proving the `just`-variable evaluator resolves
   `{{rust_verification_owner}}`), `test_nextest_fingerprint.py`, `scripts/nextest_fingerprint.py`
   **discovered as a child of** `test_nextest_fingerprint.py` (proves F4 closed), and all nine F7′ targets by
   name (`lane_governor.py`, `command_understanding.py`, `cancel_obsolete_dispatch_runs.py`,
   `ci_provenance.py`, `ubicloud_runner_minutes.py`, `developer_tool_storage_hygiene.py`,
   `find_same_sha_main_evidence.py`, `require_sp_reviewer.py`, `require_resolved_review_threads.py`) so an
   emit-time discovery gap cannot be frozen into the floor.
2. **Non-vacuity floor preserved** (F10) **and** a planted repo-root write is caught in each newly-covered
   form: `-m scripts.X`; constant- **and function-scope**-resolvable subprocess paths (incl. non-`.py`
   `cargo-shim`); the precise-origin idiom in **module and function scope** (anchors `test_cargo_shim.py:15`,
   `test_clean_merged_artifacts.py:1185/1188`) across direct/assigned/chained/`.parent`/`.parents[N]`/`.joinpath` writes.
3. **In-process loads are L1 fence edges (stay green today):** loading one of the nine unscanned targets (F7′)
   via `spec_from_file_location`/`SourceFileLoader`/`import_module` **adds it to the scan set and analyzes it**
   (clean → passes), including through the f-string sibling-path **trivial wrapper** (`test_lane_governor.py:20-22`,
   `test_local_verification_gate.py:18-19`) and default-parameter loader wrappers
   (`test_find_same_sha_main_evidence.py:25`, `test_verify_ci_workflow_hygiene.py:167-194`);
   a non-constant / unresolvable direct load target hard-fails; a parameter-bound temp-fixture loader is L3;
   a constant
   module name with no `scripts/<name>.py` (stdlib) is the documented boundary (no fail). **Edge recognition
   binds to real call expressions:** the argv-shaped list literals in `test_command_understanding.py:277-299`
   (F8′) are **not** mistaken for edges, while the real `subprocess.run(-c)` at `:163` **is**.
4. **`-c` handling:** after adjacent-literal concatenation, a constant-resolvable `-c` string naming a
   `scripts/…` path **not** in the scan set fails (anchor `test_command_understanding.py:155-164`); a pure-tmp
   `-c` string (today's `test_local_verification_gate.py` cases) passes; a non-constant `-c` string is the
   documented boundary (no check).
5. **Layer-2 tripwires proven:** a **positively-Python** subprocess/`-m`/loader target that is
   non-constant/unresolvable in a directly-resolved expression; an unresolved wrapper default used by an
   omitted argument; an explicit wrapper argument whose target is undefined / conditional / multiply-bound /
   otherwise unresolvable; a `Name` with zero/multiple/conditional bindings; `eval(`/`exec(` or a dynamic
   code-exec-family call (`os.system`, …) in a scanned script; and an unresolved `{{just-variable}}` in a
   cheap-closure command position — each hard-fails. **Do not fail** (Layer-3 boundaries): a bare PATH command
   (`git`, `grep`, `cargo`, `printf`); a `bash`/`sh` + repo-local operand (anchor `test_run_rust_probe.py:75`);
   an opaque/coordinator dynamic dispatch (`local_verification_gate.py:51`); parameter-bound temp-fixture loader wrappers;
   a `python3 -c` inside a
   `$(…)`/pipeline naming no `scripts/…` path (`justfile:481-483`).
6. **`just --dump` fail-closed proven** via a **synthetic JSON fixture** (no real-`justfile` mutation): a
   malformed / missing-key / unrecognized-expression / truncated-body dump fails; a synthetic new `local-gate:*`
   gate is auto-discovered with its inner scripts; a **multi-hop dependency chain** terminates at a fixed point.
7. **Gate-derivation fixtures** hard-fail: label-without-recipe, recipe-without-label, private coordinator
   recipe, gate-name ≠ recipe-name, missing inner.
8. **Drift checks:** a script removed from a cheap recipe drops from the closure; a direct-only cheap-labeled
   script stays scanned **and** cheap-classified; the **bash-negated** form `if ! python3 scripts/X.py`
   (`justfile:499`) is discovered.
9. Subcrate policy mirror preserved (any `local-gate:*` labels in the subcrate toml are discovered too).
10. Runs under **CI Python 3.12**. Final proof = **exact-head PR CI green + required-reviewer approval**
    (node `U_kgDOEZMFhA`).

---

## 7. Scope, non-goals, deferred

- **In scope:** the guard test (`test_lane_governor.py`) and its analyzer only.
- **Non-goals / rejected:** Option B (labels as a derived closure-mirror — breaks F2 direct-invocation
  classification); Option C (a runtime closure-resolver consumed by `lane_governor` — wrong layer, indirection
  on the O(1) `acquire` hot path); pinning the `just` version; the **round-4 in-process-load membership
  demotion** — rejected (round-5): nine clean load targets are unscanned today (F7′), so a membership hard-fail
  would RED clean code; loads are L1 fence edges (§4).
- **Documented boundaries (Layer 3):** non-constant inline `-c` code; regular `import` / `from X import Y`, and
  a constant `import_module`/`run_module` name with no `scripts/<name>.py` (stdlib/installed; impractical to
  trace without `sys.path` simulation); bare **PATH commands** (`git`, `grep`, `cargo`, `printf`, …); a
  **non-Python** execution target — a `bash`/`sh` + operand or a repo-local non-Python file (`.sh` shim) — the
  Python-AST analyzer cannot inspect it (anchor `test_run_rust_probe.py:75`, cheap-labeled and clean); the
  coordinator/owner **opaque dynamic dispatch** (`local_verification_gate.py:51`), parameter-bound temp-fixture
  loader wrappers, callees discovered via the
  closure + labels; `__import__` and functional `importlib` variants outside the recognized loader allowlist
  (`spec_from_file_location`, `SourceFileLoader`, `importlib.import_module`, `runpy.run_path`, `runpy.run_module`).
  A repo-local file that **is** Python-source despite a non-`.py` name (`cargo-shim`, shebang `python3`) with a
  resolvable path is an **L1 fence edge** instead (it is analyzable).
- **`nextest_fingerprint.py` writes** (F4) are **parameter-mediated** (`--output-path`, `args.repo_root`):
  adding it to the scan set catches **future direct** repo-root writes; its current arg-derived writes remain
  in the documented parameter-passing miss class (and are safe because the test passes tmp).
- **Deferred follow-ups (not blockers):** the reverse "stale label" drift (a label no longer referenced by any
  recipe — cosmetic; the script is still scanned); broadening precise-origin to alias / parameter /
  `Path(args.X)` idioms if a future write needs it; a separate shell-aware check for `.sh` shim repo-root
  writes (out of scope for the Python-AST analyzer).

---

## 8. Risks & open questions

- **R1 — `just --dump` schema / unknown assignment expression.** Mitigated by Change 2 shape-assertion +
  fail-closed on any unrecognized expression + the **committed-manifest** floor; a future `just` upgrade that
  changes the dump fails loudly, never silently empties the closure.
- **R2 — target-path resolution depth.** Change 1's grammar resolves literal / `str()`/`Path()` /
  nested-then-module `Name` / `BinOp` / `.joinpath` / `.parent` / `.parents[N]` chains, constant/default
  wrapper args, and the f-string sibling-path trivial wrapper. Direct unresolved Python-shaped expressions
  and explicit unresolved wrapper arguments drop to the Layer-2 tripwire; proven parameter/temp-fixture
  wrapper args remain the documented Layer-3 opaque boundary.
- **R3 — precise-origin false positives.** Mitigated by F6 (owner-only `repo_path`) + proof-gate (c)
  regression across the whole scan set; the hard-decision fallback covers the residual.
- **R4 — in-process-load resolution depth.** Loads are L1 fence edges (resolve→add→recurse); a direct load whose
  target path/name is non-constant/unresolvable hard-fails (L2). Benign today: repo-targeting load sites use a
  constant name (`import_module`) or a grammar-/trivial-wrapper-resolvable path (incl. the `_load` f-string
  sibling-path helper and constant defaults); parameter-bound temp-fixture loaders stay L3. The nine F7′
  targets are clean and pass once added.
- **R5 — Layer-2 over-fence (round-5 regression, fixed in round-6).** The broad "any unresolvable / repo-local
  non-Python target → L2" would have RED'd clean shims (`test_run_rust_probe.py:75`) and the coordinator's
  dynamic dispatch (`local_verification_gate.py:51`). Fixed: L2 fires only on a **positively-Python-shaped**
  unresolvable target; non-Python / opaque spawns are Layer-3 boundaries.
- **OQ1 — interpreter wrappers** (`uv run python3`, container shims): none today; Change 2's command
  classification fails closed on an unrecognized interpreter in command position — revisit if one is introduced.
