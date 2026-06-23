# #927 — Implementation Plan (code-level)

**Reads with:** `plan.md` (architecture, invariant, three-layer model, findings F1–F10/F7′/F8′).
This file is the **build sequence** — concrete functions, signatures, and proof obligations, every
"modify" anchored to `scripts/test_lane_governor.py` at HEAD `a59ae0776`.

**Single file changed:** `scripts/test_lane_governor.py` (the guard + its analyzer). `lane_governor.py`,
`rust_verification.py`, `ci/rust-verification.toml`, and the `justfile` are **read-only** for this work.
**Plus** one **new committed data file**: the discovered-but-unlabeled manifest (§3).

**Run/verify with CI Python 3.12** (`/opt/homebrew/bin/python3.12`) — local default 3.14 differs; the
guard runs under 3.12 in CI. No new third-party deps (`just`, `ast`, `json`, `subprocess`, `tomllib` only).

---

## Current state being replaced (anchors @ `a59ae0776`)

- `_justfile_recipe_python_scripts(recipe_name)` `:381-398` — body-only text parser; matches **only**
  `python3 scripts/X.py`. Returns 0 for dependency-only recipes (F3). **Replaced by the §1 evaluator.**
- `_cheap_lane_python_scripts()` `:401-419` — seed = `.py` labels ∪ `source-fence-static-inner` body.
  **Replaced by the §1 fixed-point discovery.**
- `_RepoSharedStateWriteAnalyzer` `:153-378` — write detector. `_origin` `:257-276`, `_call_origin`
  `:278-297`. **Extended by §2 (precise-origin).** Other methods unchanged.
- `_repo_shared_state_write_findings(path)` `:422-426` — `ast.parse` + visit. Unchanged.
- `test_cheap_lanes_do_not_write_repo_root_shared_state()` `:429-447` — hard-codes the F10 floor at
  `:431-434`. **Rewired by §3 + §4.**

---

## §1 — Discovery as a three-layer fixed point (Changes 1 + 3 + 2-classification)

Replace `_justfile_recipe_python_scripts` and `_cheap_lane_python_scripts` with the components below.
All new helpers are module-level functions (mirror the existing style). Discovery is **fully static** —
the only subprocess is one `just --dump`.

### 1a. `_just_dump() -> dict` — structured recipe source (Change 2 fail-closed)
- `subprocess.run(["just","--dump","--dump-format","json"], cwd=REPO_ROOT, capture_output=True, text=True)`.
- Hard-fail (raise `AssertionError`) on: non-zero exit, invalid JSON, or missing required top-level keys
  (`recipes`, `assignments`/`settings` as the installed `just` emits). **Shape-assert; do not pin the
  `just` version.**

### 1b. `_eval_assignment(expr, dump) -> str` — recursive `just` value evaluator
- Handle the dump's expression forms: literal string; `['concatenate', a, b]`; `['variable', NAME]`
  (look up in `dump` assignments, evaluate recursively); `['call', 'justfile_directory', …]` → `str(REPO_ROOT)`.
- **Required** so `rust_verification_owner` → `<repo>/scripts/rust_verification.py` (F9, `justfile:18,19`).
- Any unrecognized expression node → hard-fail (fail-closed, R1).

### 1c. `_recipe_command_lines(recipe, dump) -> list[str]` — body → command lines
- Stitch each recipe `body` fragment into lines: string fragments + `['variable', NAME]` → `_eval_assignment`.
- **Flatten nested-array fragments recursively** (F9). Substitute `{{var}}` via 1b.
- Yield the inner command of **command-substitution `$(…)`** and **pipeline** segments (`justfile:481-483`)
  and the negated form `if ! <cmd>` (`justfile:499`, F4) as their own command lines.

### 1d. `_cheap_gate_closure(dump) -> tuple[set[str] recipes, dict gates]`
- Seed recipes = the 3 public cheap gates. **Derive them, do not hard-code** (Change 3): scan every recipe
  body for a `local_verification_gate.py <gate> --` call; the gate names found, cross-checked against
  `local-gate:*` labels in `ci/rust-verification.toml [local_lane_policy].cheap_lane_labels`, are the seeds.
- Walk `recipe['dependencies']` **and** `-- just <inner>` routing (parsed from command lines, 1c) to a
  **fixed point**. **Scope every later classification to this recipe set** (not the whole justfile — F2;
  `live-*`/`bte-*` must not be classified).

### 1e. `_validate_gates(gates, labels)` (Change 3)
- For each derived `<gate>`: require a matching `local-gate:<gate>` label, a **non-private** recipe,
  a real `<gate>-inner` recipe, and `recipe-name == gate-name` (unless in an explicit `_GATE_ALIASES`
  constant). Hard-fail **either direction**: label-without-recipe, recipe-without-label, private recipe
  invoking the coordinator, wrong/missing inner. (AC7.)

### 1f. `_classify_command(line) -> Literal['none','py-exec','boundary','dynamic-shell']`
- Over every command position in the closure's command lines (1c): a recognized Python interpreter
  (`python3`/`python`/`sys.executable`) + script operand → `py-exec` (feeds 1g); a bare PATH command /
  non-Python interpreter (`bash`,`sh`) / non-Python operand → `boundary`; **an unresolved `{{var}}`, an
  unrecognized interpreter wrapper (`env python3`, `${PYTHON}`, an alias), or a shell-expanded command
  position → `dynamic-shell` → hard-fail** (Change 2 completeness, L2(c)). (AC5, AC6.)

### 1g. `class _CodeExecutionEdgeResolver(ast.NodeVisitor)` — the L1/L2 edge resolver (NEW)
The core of Change 1. Runs on **one script's AST**; returns `(resolved_targets: set[Path], hard_fails: list[str])`.
- **Scope-aware name table.** Mirror the existing `_visit_isolated_body` pattern (`:185-191`): keep a
  `bindings: dict[str,Optional[ResolvedTarget]]`; on `FunctionDef`/`AsyncFunctionDef` push a **copy**
  (inherits outer bindings already visited top-down — covers assigned-before-use incl. the nested
  `run_in_nongit` at `test_clean_merged_artifacts.py:1185/1188`); pop on exit. Lookup =
  innermost→outer→module. **Only a single, unconditional, textually-prior `Assign` binding resolves;**
  multiple/conditional/absent → leave unresolved → L2.
- **Edge recognition binds to real call expressions only** (F8′): `subprocess.run/Popen/call/check_call/check_output`,
  and the loader allowlist `spec_from_file_location` / `SourceFileLoader` / `importlib.import_module` /
  `runpy.run_path` / `runpy.run_module`. **Never** treat a bare list literal as an edge (ignore
  `test_command_understanding.py:277-299`; catch the real `subprocess.run(-c)` at `:163`).
- **Target grammar** (`_resolve_target(node) -> ResolvedTarget|UNRESOLVED`): literal str; `str(EXPR)`/`Path(EXPR)`
  (unwrap); `Name` (scope lookup above); `BinOp(/)`, `.joinpath()`, `.parent`, `.parents[N]` chains (recursive);
  `__file__`-anchored chains → repo paths. A **module name** (`import_module`/`run_module`, constant) →
  `scripts/<name>.py` **only if that file exists**; constant name with no such file → **boundary** (stdlib, no
  fail); non-constant name → **L2**.
- **Trivial-wrapper, sibling-path (GLM/GPT blocker).** A local helper whose `name`/path **parameter** flows
  into a loader/spawn — incl. `DIR / f"{name}.py"`, `SCRIPTS_DIR / f"{name}.py"`,
  `Path(__file__).with_name(f"{name}.py")`, where the parameter is the **sole** non-literal — resolves one hop
  from the constant call-site arg. **This supersedes the general f-string exclusion** for that sole-non-literal
  case. Anchors: `_load` at `test_lane_governor.py:18-19` / `test_local_verification_gate.py:18-19`, called
  `_load("rust_verification")`/`_load("lane_governor")`/`_load("local_verification_gate")`. Other f-strings,
  cross-module-imported constants, dynamically built argv → **L2**.
- **Interpreter shape** for subprocess argv[0]: Python interpreter → resolve argv[1]; non-Python (`bash`/`sh`)
  or bare PATH → **boundary**; an **opaque** argv that is a passed-in variable → **boundary** (the
  coordinator/owner dynamic dispatch — `local_verification_gate.py:51`; callees discovered via 1d + labels).
- **`-c` heuristic (boundary + sub-tripwire).** A non-constant `-c` string → boundary. A **constant** `-c`
  string — after **adjacent-string-literal concatenation** (incl. parenthesized multiline, `ast.Constant`
  joins) — that names literal `scripts/…` paths → those must be scan-set members else hard-fail
  (`test_command_understanding.py:155-164`). `-c` whose payload is pure-tmp → no edge.
- **`eval(`/`exec(`** and the dynamic code-exec family (`os.system`, `os.popen`, `os.exec*`, `os.spawn*`,
  `subprocess.getoutput`, `subprocess.getstatusoutput`, `pty.spawn`) → **L2 hard-fail** (F8 drift-fence).

### 1h. `_discover_cheap_lane_scripts() -> set[Path]` — the fixed point (replaces `_cheap_lane_python_scripts`)
- `seed` = (cheap-labeled `.py`, existence-checked as today `:404-411`) ∪ (closure scripts from 1c/1d/1f).
- Worklist loop: for each script, run `_CodeExecutionEdgeResolver`; **add resolved targets, recurse** until
  no new script is added. Accumulate `hard_fails`; any non-empty → the test fails with the reasons.
- Returns the full scan set (labeled + closure + all L1-fence transitive targets incl. `nextest_fingerprint.py`,
  `cargo-shim`, `clean_merged_artifacts.py`, the nine F7′ load targets).

---

## §2 — Analyzer precise-origin, by expression not name (Change 4)

Extend `_RepoSharedStateWriteAnalyzer` **only** — additive, no behavior removed.
- In `_call_origin` `:278-297`: add `if isinstance(func, ast.Name) and func.id == "repo_path": return _REPO_ORIGIN`
  (F6 proves `repo_path(` is owner-only → false-positive-safe).
- In `_origin` `:257-276`: recognize the **expression** `Path(__file__)[.resolve()/.absolute()]*.parent`
  and `…​.parents[N]` → `_REPO_ORIGIN` (the script's own directory / an ancestor — a repo-root path).
  Implement as a small matcher on the Attribute/Subscript chain bottoming out at `Path(__file__)`.
  **Key:** keyed by the **expression shape, regardless of the bound variable's name** — so a write through a
  `SCRIPT_DIR`/`ROOT` bound from `Path(__file__)…` (anchors `test_cargo_shim.py:15`, `rust_verification.py:22`)
  is caught. **Do not** treat the bare name `repo` as an origin (synthetic tmp across 100+ scripts → false cascade).
- Existing origin propagation through `.parent`/path methods (`:264-296`) already chains; **verify by test
  (§4 obligation b), do not assume.**

---

## §3 — Committed discovered-but-unlabeled manifest (Change 2 floor)

The walk-independent floor. Two parts:
1. **Emit-once script** (a tiny `if __name__ == "__main__"` mode or a sibling one-shot): run `_discover_cheap_lane_scripts`,
   subtract the cheap-labeled `.py` seed, write the sorted remainder to
   `scripts/cheap_lane_discovered_unlabeled.manifest` (one repo-relative path per line). Commit it.
   **Seed verified at HEAD** (the emit must contain at least these): `local_verification_gate.py`,
   `rust_verification.py`, `test_nextest_fingerprint.py`, `nextest_fingerprint.py`, `clean_merged_artifacts.py`,
   `cargo-shim`, and the nine F7′ targets (`lane_governor.py`, `command_understanding.py`,
   `cancel_obsolete_dispatch_runs.py`, `ci_provenance.py`, `ubicloud_runner_minutes.py`,
   `developer_tool_storage_hygiene.py`, `find_same_sha_main_evidence.py`, `require_sp_reviewer.py`,
   `require_resolved_review_threads.py`).
2. **Assertion in the test:** `live_discovered_unlabeled ⊇ committed_manifest`, **by name**. A walk/fence
   regression drops a path from live but not the manifest → RED. Additions are allowed (live ⊋ manifest is
   fine); a **removal** requires a deliberate manifest edit. This is the floor — **not** a re-derivation
   (it re-computes nothing) and **not** a count.

---

## §4 — Tests (acceptance criteria → concrete cases)

Add focused test functions (each `assert`-based, runnable under the file's `main()` `:1078`). Use **synthetic
JSON-dump fixtures and synthetic AST snippets** — **never mutate the real `justfile`/toml**.

- **(a) Discovery completeness** → AC1: assert the §3 manifest-subset; assert `owner`, `coordinator`,
  `test_nextest_fingerprint.py`, and `nextest_fingerprint.py` (as a child of the test) are all discovered.
- **(b) Precise-origin non-vacuity** → AC2: planted repo-root write caught in `-m`, constant- &
  function-scope subprocess path (incl. `cargo-shim`), and the `Path(__file__)…`/`repo_path()` idiom in
  **module and function scope** (anchors `test_cargo_shim.py:15`, `test_clean_merged_artifacts.py:1185/1188`),
  across direct/assigned/chained/`.parent`/`.parents[N]`/`.joinpath`; the real owner → **0** findings; **no
  new findings** across the existing scan set (run §1h + §2 over today's tree → empty).
- **(c) In-process loads = L1, green today** → AC3: each of the nine F7′ targets, loaded via the `_load`
  f-string wrapper, is added + analyzed clean; a non-constant load target → hard-fail; the `:277-299` list
  literals are **not** edges.
- **(d) `-c` heuristic** → AC4: concatenated constant `-c` naming a `scripts/…` path not in the set fails;
  pure-tmp `-c` passes; non-constant `-c` = boundary.
- **(e) Layer-2 tripwires** → AC5: positively-Python unresolvable target; `Name` zero/multiple/conditional;
  `eval`/`exec`/`os.system`; unresolved `{{var}}` → each fails. Boundaries do **not** fail: PATH command;
  `bash` + repo-local operand (`test_run_rust_probe.py:75`); coordinator opaque dispatch
  (`local_verification_gate.py:51`); `$(…)`/pipeline `-c` naming no `scripts/…` path.
- **(f) `just --dump` fail-closed** → AC6: malformed / missing-key / unrecognized-expression / truncated-body
  synthetic dump fails; a synthetic new `local-gate:*` gate auto-discovers its inner scripts; a multi-hop
  dependency chain reaches a fixed point.
- **(g) Gate-derivation** → AC7: the five disagreement fixtures hard-fail.
- **(h) Drift** → AC8: script removed from a cheap recipe drops from the closure; a direct-only cheap-labeled
  script stays scanned **and** cheap-classified (don't regress F2); the `if ! python3 scripts/X.py` form
  (`justfile:499`) is discovered.
- **(i)** AC9 subcrate mirror preserved (extend existing `test_subcrate_lane_policy_matches_repo_policy` `:450`).

---

## §5 — Inventory constant (Change 5)
Publish the recognized invocation-form inventory (interpreters, loader allowlist, code-exec family, mutating
methods) as a single module constant referenced by §1g/§2 — single source of truth, no scattered literals.

---

## §6 — Verification gate (the order Codex proves it)
1. Work **inside the existing worktree** `.worktrees/927-cheap-lane-guard` — already on branch
   `fix/927-cheap-lane-guard-completeness` (off clean `main`), with `plan.md` + `implementation.md` present
   (untracked — commit them alongside your code). Do **not** create the branch; it already exists. **No**
   close-keyword anywhere (commits or PR body).
2. Implement §1→§2→§3→§5, then §4 tests.
3. `/opt/homebrew/bin/python3.12 scripts/test_lane_governor.py` → **all green**, **including** the existing
   acquire/concurrency suite (don't regress).
4. Prove non-vacuity live: temporarily plant a repo-root write in a throwaway scanned script → test goes
   **RED** on it → revert. (Proof obligation b, run for real, not asserted on faith.)
5. Confirm the committed manifest matches the live emit (§3) and contains the verified seed.
6. Push branch; **exact-head CI green** is the final oracle; required reviewer `U_kgDOEZMFhA` approves. PR
   body uses `#927` reference **without** a close-keyword (squash hoists commit msgs onto `main`).

**Stop conditions:** if the same step fails twice, stop and report (don't thrash). If discovery surfaces a
**real** repo-root writer in a currently-reachable script (not expected — all clean today per F1/F5), that is
a genuine finding → surface it, do not silence it.
