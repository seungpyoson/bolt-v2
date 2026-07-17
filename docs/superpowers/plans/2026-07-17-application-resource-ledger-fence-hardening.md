# Application Resource Ledger Fence Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the application-resource-ledger source fence reject all five confirmed alternate spellings of forbidden production authority or capability paths.

**Architecture:** Keep the existing Python verifier as the sole structural fence. Add narrowly scoped production-aware helpers for protected definitions, public surface items, use-tree aliases, protected factories, and canonical loader paths; every helper is driven by a mutation test that first demonstrates the current fail-open behavior.

**Tech Stack:** Python 3 standard library, `unittest`, existing `rust_source_scanner` helpers, the existing production `cfg` projection, and repository `just` verification recipes.

## Global Constraints

- Change only `scripts/verify_risk_closure_workspace_authority.py`, `scripts/test_verify_risk_closure_workspace_authority.py`, and the approved design/plan documents.
- Do not change Rust runtime code, compile-fail tests, generated configuration, or runtime audit configuration.
- Add no dependency or alternate verification path.
- Fail closed on unsupported syntax affecting a protected item.
- Run local non-compile verification only; do not run Cargo, Rust tests, builds, or Clippy locally.
- Preserve the current allowed ledger functions, private protected fields, test-only constructors, and canonical module ownership.

---

### Task 1: Production-effective privacy and Clone checks

**Files:**
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py:99-178`
- Modify: `scripts/verify_risk_closure_workspace_authority.py:13-20,478-563`

**Interfaces:**
- Consumes: `production_text(text: str) -> str` and `cfg_truth_without_test(expression: str) -> tuple[bool, bool]`.
- Produces: `_production_cfg_attr_payloads(attributes: str) -> list[str]` and production-effective raw-authority validation.

- [ ] **Step 1: Write failing mutation tests**

Add:

```python
def test_rejects_production_public_raw_authority_hidden_by_test_definition(self) -> None:
    owner = (
        self.root / "src" / "bolt_v3_application_resource_ledger"
        / "risk_closure_workspace.rs"
    )
    owner.write_text(
        owner.read_text(encoding="utf-8").replace(
            "pub(super) struct RiskClosureWorkspaceAuthority;",
            "#[cfg(test)]\npub(super) struct RiskClosureWorkspaceAuthority;\n"
            "#[cfg(not(test))]\npub struct RiskClosureWorkspaceAuthority;",
        ),
        encoding="utf-8",
    )
    ledger = self.root / "src" / "bolt_v3_application_resource_ledger.rs"
    ledger.write_text(
        ledger.read_text(encoding="utf-8") + "\npub use risk_closure_workspace::*;\n",
        encoding="utf-8",
    )

    errors = verifier.authority_errors(self.root)

    self.assertTrue(any("must remain ledger-private" in error for error in errors))

def test_rejects_production_only_cloneable_raw_workspace_authority(self) -> None:
    owner = (
        self.root / "src" / "bolt_v3_application_resource_ledger"
        / "risk_closure_workspace.rs"
    )
    owner.write_text(
        owner.read_text(encoding="utf-8").replace(
            "pub(super) struct RiskClosureWorkspaceAuthority",
            "#[cfg_attr(not(test), derive(Clone))]\n"
            "pub(super) struct RiskClosureWorkspaceAuthority",
        ),
        encoding="utf-8",
    )

    errors = verifier.authority_errors(self.root)

    self.assertTrue(any("must not implement Clone" in error for error in errors))
```

- [ ] **Step 2: Run the new tests and verify RED**

```bash
env PYTHONPATH=scripts python3 -m unittest \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_public_raw_authority_hidden_by_test_definition \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_only_cloneable_raw_workspace_authority
```

Expected: two assertion failures because the current verifier returns no matching error.

- [ ] **Step 3: Implement production-aware checks**

Import `cfg_truth_without_test` beside `production_text`. Add:

```python
def _production_cfg_attr_payloads(attributes: str) -> list[str]:
    payloads: list[str] = []
    code = strip_rust_comments_and_literals(attributes)
    for match in re.finditer(r"#\s*\[\s*cfg_attr\b", code):
        opening = code.find("(", match.end())
        if opening == -1:
            continue
        closing = _matching_delimiter_end(code, opening)
        if closing is None:
            continue
        segments = _top_level_segments(code, opening + 1, closing)
        if len(segments) < 2:
            continue
        start, end = segments[0]
        can_be_true, _ = cfg_truth_without_test(attributes[start:end])
        if can_be_true:
            payloads.extend(attributes[start:end].strip() for start, end in segments[1:])
    return payloads
```

In `authority_errors`, build `owner_production_code` from `production_text(owner_text)`. Capture the attributes and visibility of every production raw definition, then require exactly one definition with exact `pub(super)` visibility:

```python
raw_definitions = list(re.finditer(
    r"(?P<attributes>(?:#\[[^\]]+\]\s*)*)"
    r"(?P<visibility>pub(?:\([^)]*\))?\s+)?struct\s+"
    r"(?:r#)?RiskClosureWorkspaceAuthority\b",
    owner_production_code,
))
if len(raw_definitions) != 1 or raw_definitions[0].group("visibility") != "pub(super) ":
    errors.append(f"raw workspace authority must remain ledger-private in {OWNER}")
```

Reject Clone when a direct derive, production-active `cfg_attr` payload, or protected trait impl names Clone:

```python
conditional_clone = any(
    re.search(r"\bderive\s*\([^)]*\bClone\b", payload)
    for definition in raw_definitions
    for payload in _production_cfg_attr_payloads(definition.group("attributes"))
)
```

- [ ] **Step 4: Run the two new tests plus existing privacy and Clone tests**

Run the two new fully qualified names together with `test_rejects_public_raw_workspace_authority`, `test_comment_cannot_fake_raw_authority_privacy`, and `test_rejects_cloneable_raw_workspace_authority`.

Expected: five tests pass.

- [ ] **Step 5: Commit Task 1**

```bash
git add scripts/verify_risk_closure_workspace_authority.py scripts/test_verify_risk_closure_workspace_authority.py
git commit -m "fix(resources): verify production authority privacy"
```

---

### Task 2: Exact public capability item surface

**Files:**
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py:309-362`
- Modify: `scripts/verify_risk_closure_workspace_authority.py:41-47,414-432,582-626`

**Interfaces:**
- Consumes: `_matching_delimiter_end`, `_top_level_segments`, `_function_definitions`, and production ledger text.
- Produces: raw-aware function recognition, `_protected_struct_public_fields(text: str) -> list[tuple[str, str]]`, and `_unexpected_public_items(text: str) -> list[str]`.

- [ ] **Step 1: Write failing public-surface mutation tests**

Add three tests that make these exact mutations and assert an `exact public capability surface` error:

```python
ledger.write_text(
    ledger.read_text(encoding="utf-8").replace(
        "pub struct RecoveryWorkspaceHandle;",
        "pub struct RecoveryWorkspaceHandle {\n"
        "    pub new_risk: NewRiskWorkspaceHandle,\n"
        "}",
    ),
    encoding="utf-8",
)
```

```python
ledger.write_text(
    ledger.read_text(encoding="utf-8")
    + "\nfn escalate(_: RecoveryWorkspaceHandle) -> NewRiskWorkspaceHandle { panic!() }\n"
    + "pub static ESCALATE_RECOVERY: "
    + "fn(RecoveryWorkspaceHandle) -> NewRiskWorkspaceHandle = escalate;\n",
    encoding="utf-8",
)
```

```python
ledger.write_text(
    ledger.read_text(encoding="utf-8")
    + "\nimpl RecoveryWorkspaceHandle {\n"
    + "    pub fn r#escalate(&self) -> NewRiskWorkspaceHandle { panic!() }\n"
    + "}\n",
    encoding="utf-8",
)
```

Name the tests `test_rejects_public_capability_field`, `test_rejects_public_capability_function_pointer`, and `test_rejects_raw_identifier_public_capability_method`. During final adversarial replay, add `test_rejects_public_tuple_capability_field` and `test_rejects_crate_visible_tuple_capability_field` with `pub NewRiskWorkspaceHandle` and `pub(crate) NewRiskWorkspaceHandle` tuple fields respectively.

- [ ] **Step 2: Run the five tests and verify RED**

Run their fully qualified unittest names. Expected: all five fail because none of the mutations enters the current ordinary-function or named-field census.

- [ ] **Step 3: Implement raw-aware and non-function surface scanning**

Define `RUST_IDENT` and use it in `FUNCTION_HEADER`:

```python
RUST_IDENT = r"(?:r#)?[A-Za-z_][A-Za-z0-9_]*"
```

Normalize function names with `name.removeprefix("r#")`. Add:

```python
def _protected_struct_public_fields(text: str) -> list[tuple[str, str]]:
    fields: list[tuple[str, str]] = []
    for type_name in (
        "ApplicationResourceLedger",
        "NewRiskWorkspaceHandle",
        "RecoveryWorkspaceHandle",
    ):
        declaration = re.search(rf"\bstruct\s+(?:r#)?{type_name}\b", text)
        if declaration is None:
            continue
        delimiters = [
            index
            for delimiter in ("{", "(", ";")
            if (index := text.find(delimiter, declaration.end())) != -1
        ]
        if not delimiters:
            fields.append((type_name, "<unclosed>"))
            continue
        opening = min(delimiters)
        if text[opening] == ";":
            continue
        closing = _matching_delimiter_end(text, opening)
        if closing is None:
            fields.append((type_name, "<unclosed>"))
            continue
        for start, end in _top_level_segments(text, opening + 1, closing):
            field = text[start:end].strip()
            if re.match(
                r"(?:#\[[^\]]+\]\s*)*pub(?:\([^)]*\))?(?:\s|$)", field
            ):
                fields.append((type_name, _normalize_rust_fragment(field)))
    return fields

def _unexpected_public_items(text: str) -> list[str]:
    allowed_structs = {
        "ApplicationResourceLedger",
        "NewRiskWorkspaceHandle",
        "RecoveryWorkspaceHandle",
    }
    unexpected: list[str] = []
    for match in re.finditer(
        r"\bpub(?:\([^)]*\))?\s+"
        r"(?P<kind>const|static|type|enum|trait|mod|struct|union)\s+"
        r"(?P<name>(?:r#)?[A-Za-z_][A-Za-z0-9_]*)",
        text,
    ):
        item_name = match.group("name").removeprefix("r#")
        if match.group("kind") != "struct" or item_name not in allowed_structs:
            unexpected.append(match.group(0).strip())
    return unexpected
```

Append the existing exact-surface error if either helper returns entries.

- [ ] **Step 4: Run all eight public-surface tests and verify GREEN**

Run the five new names together with `test_rejects_public_opaque_raw_authority_accessor`, `test_rejects_qualified_public_function_modifiers`, and `test_rejects_public_function_without_explicit_return_type`.

Expected: eight tests pass.

- [ ] **Step 5: Commit Task 2**

```bash
git add scripts/verify_risk_closure_workspace_authority.py scripts/test_verify_risk_closure_workspace_authority.py
git commit -m "fix(resources): close capability surface census"
```

---

### Task 3: Grouped aliases and protected trait resolution

**Files:**
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py:280-308`
- Modify: `scripts/verify_risk_closure_workspace_authority.py:59-76,242-257,325-447`

**Interfaces:**
- Consumes: raw-aware `RUST_IDENT` and `_top_level_segments`.
- Produces: `_use_aliases(text: str) -> list[tuple[str, str]]` and qualified-path-aware protected type and trait matching.

- [ ] **Step 1: Write failing grouped type- and trait-alias tests**

```python
def test_rejects_conversion_trait_through_grouped_use_aliases(self) -> None:
    ledger = self.root / "src" / "bolt_v3_application_resource_ledger.rs"
    ledger.write_text(
        ledger.read_text(encoding="utf-8")
        + "\nuse self::{NewRiskWorkspaceHandle as N, RecoveryWorkspaceHandle as R};\n"
        + "impl From<N> for R {\n"
        + "    fn from(_: N) -> Self { panic!() }\n"
        + "}\n",
        encoding="utf-8",
    )

    errors = verifier.authority_errors(self.root)

    self.assertTrue(
        any("must not implement construction or conversion traits" in error for error in errors)
    )
```

During final adversarial replay, add `test_rejects_raw_workspace_clone_impl_through_grouped_trait_alias` with:

```python
owner.write_text(
    owner.read_text(encoding="utf-8")
    + "\nuse core::{clone::Clone as C};\n"
    + "impl C for RiskClosureWorkspaceAuthority {\n"
    + "    fn clone(&self) -> Self { Self }\n"
    + "}\n",
    encoding="utf-8",
)
```

- [ ] **Step 2: Run both tests and verify RED**

Expected: two assertion failures because `N` and `R` are absent from the protected-type closure and `C` is absent from the protected-trait closure.

- [ ] **Step 3: Implement grouped-use alias expansion**

Make `RUST_PATH` raw/absolute-aware:

```python
RUST_PATH = rf"(?:::)?{RUST_IDENT}(?:::{RUST_IDENT})*"
```

Add:

```python
def _use_aliases(text: str) -> list[tuple[str, str]]:
    aliases: list[tuple[str, str]] = []
    for match in re.finditer(r"\buse\s+(?P<tree>[^;]+);", text, re.DOTALL):
        tree = match.group("tree").strip()
        opening = tree.find("{")
        if opening == -1:
            alias = re.fullmatch(
                rf"(?P<target>{RUST_PATH})\s+as\s+(?P<alias>{RUST_IDENT})",
                tree,
            )
            if alias is not None:
                aliases.append((alias.group("target"), alias.group("alias")))
            continue
        closing = _matching_delimiter_end(tree, opening)
        if closing is None or tree[closing + 1 :].strip():
            continue
        prefix = tree[:opening].rstrip().removesuffix("::")
        for start, end in _top_level_segments(tree, opening + 1, closing):
            item = tree[start:end].strip()
            alias = re.fullmatch(
                rf"(?P<target>{RUST_PATH})\s+as\s+(?P<alias>{RUST_IDENT})",
                item,
            )
            if alias is not None:
                aliases.append(
                    ("::".join(filter(None, (prefix, alias.group("target")))), alias.group("alias"))
                )
    return aliases
```

Update `_protected_type_names` to close over `TYPE_ALIAS` matches and `_use_aliases(text)`. Strip `r#` from terminal identifiers before comparison. Use the new path expression in `IMPL_HEADER` and `TRAIT_IMPL_HEADER`. When `_has_protected_trait_impl` receives a specific trait name such as `Clone`, resolve that trait's aliases through the same protected-name closure before comparing the implementation header.

- [ ] **Step 4: Run grouped, simple-alias, qualified-target, and Default tests**

Run both new tests with `test_rejects_application_ledger_default_implementation`, `test_rejects_qualified_application_ledger_trait_implementation`, and `test_rejects_trait_implementation_through_local_type_alias`.

Expected: five tests pass.

- [ ] **Step 5: Commit Task 3**

```bash
git add scripts/verify_risk_closure_workspace_authority.py scripts/test_verify_risk_closure_workspace_authority.py
git commit -m "fix(resources): resolve protected use aliases"
```

---

### Task 4: Production free-function and typed factory census

**Files:**
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py:159-221`
- Modify: `scripts/verify_risk_closure_workspace_authority.py:393-411,553-580`

**Interfaces:**
- Consumes: `_function_definitions`, `_protected_type_names`, and production-effective source.
- Produces: `_explicit_factory_definitions(text: str, type_name: str) -> list[tuple[str, str]]`, covering functions plus typed constants/statics.

- [ ] **Step 1: Write failing raw-authority and ledger free-factory tests**

Use these production mutations:

```python
owner.write_text(
    owner.read_text(encoding="utf-8")
    + "\npub(super) fn production_authority_factory() "
    + "-> RiskClosureWorkspaceAuthority { RiskClosureWorkspaceAuthority }\n",
    encoding="utf-8",
)
```

```python
ledger.write_text(
    ledger.read_text(encoding="utf-8")
    + "\npub(super) fn production_ledger_factory() "
    + "-> ApplicationResourceLedger { panic!() }\n",
    encoding="utf-8",
)
```

Add `test_rejects_production_raw_authority_static_factory`, using a third raw-authority mutation which contains no factory function:

```python
owner.write_text(
    owner.read_text(encoding="utf-8")
    + "\npub(super) static RAW_FACTORY: "
    + "fn() -> RiskClosureWorkspaceAuthority = RiskClosureWorkspaceAuthority;\n",
    encoding="utf-8",
)
```

Assert errors containing `production raw workspace authority factory` for the owner function and static tests, and `production application resource ledger factory` for the ledger function test.

- [ ] **Step 2: Run all three tests and verify RED**

```bash
env PYTHONPATH=scripts python3 -m unittest \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_raw_authority_free_factory \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_application_ledger_free_factory \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_raw_authority_static_factory
```

Expected: all three assertions fail because module functions and typed factory bindings are outside the inherent constructor census.

- [ ] **Step 3: Implement explicit protected-return detection**

```python
def _explicit_factory_definitions(text: str, type_name: str) -> list[tuple[str, str]]:
    protected_names = _protected_type_names(text, type_name)
    factories: list[tuple[str, str]] = []
    for _, visibility, _, name, _, returns in _function_definitions(text):
        normalized_returns = _normalize_rust_fragment(returns)
        if any(
            re.search(
                rf"(?<![A-Za-z0-9_]){re.escape(protected)}(?![A-Za-z0-9_])",
                normalized_returns,
            )
            for protected in protected_names
        ):
            factories.append((name.removeprefix("r#"), visibility))
    for match in re.finditer(
        r"\b(?P<visibility>pub(?:\([^)]*\))?\s+)?"
        r"(?:const|static)\s+(?P<name>(?:r#)?[A-Za-z_][A-Za-z0-9_]*)"
        r"\s*:\s*(?P<declared_type>[^=;]+)",
        text,
    ):
        if any(
            re.search(
                rf"(?<![A-Za-z0-9_]){re.escape(protected)}(?![A-Za-z0-9_])",
                match.group("declared_type"),
            )
            for protected in protected_names
        ):
            factories.append(
                (
                    match.group("name").removeprefix("r#"),
                    (match.group("visibility") or "").strip(),
                )
            )
    return factories
```

Apply the helper to production-effective owner and ledger text. Keep the current inherent `Self` constructor census unchanged so the two test-only raw constructors and one test-only ledger constructor remain the exact allowlist.

- [ ] **Step 4: Run factory and inherent-constructor tests**

Run all three new tests plus `test_rejects_second_raw_authority_constructor_definition`, `test_rejects_second_application_ledger_constructor_definition`, `test_rejects_qualified_application_ledger_constructor`, and `test_rejects_application_ledger_constructor_returning_type_alias`.

```bash
env PYTHONPATH=scripts python3 -m unittest \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_raw_authority_free_factory \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_application_ledger_free_factory \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_production_raw_authority_static_factory \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_second_raw_authority_constructor_definition \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_second_application_ledger_constructor_definition \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_qualified_application_ledger_constructor \
  scripts.test_verify_risk_closure_workspace_authority.RiskClosureWorkspaceAuthorityVerifierTests.test_rejects_application_ledger_constructor_returning_type_alias
```

Expected: seven tests pass.

- [ ] **Step 5: Commit Task 4**

```bash
git add scripts/verify_risk_closure_workspace_authority.py scripts/test_verify_risk_closure_workspace_authority.py
git commit -m "fix(resources): census production authority factories"
```

---

### Task 5: Canonical protected-source loader resolution

**Files:**
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py:363-449`
- Modify: `scripts/verify_risk_closure_workspace_authority.py:298-322,628-654`

**Interfaces:**
- Consumes: `_rust_string_literals`, `_path_attribute_value_ranges`, and `INCLUDE_MACRO`.
- Produces: `_source_loader_targets(root: pathlib.Path, source: pathlib.Path, text: str) -> list[pathlib.Path]`.

- [ ] **Step 1: Write failing normalized-loader tests**

```python
def test_rejects_normalized_alternate_raw_authority_loader(self) -> None:
    (self.root / "src" / "consumer.rs").write_text(
        '#[path = "bolt_v3_application_resource_ledger/./risk_closure_workspace.rs"]\n'
        "mod shadow_authority;\n",
        encoding="utf-8",
    )
    errors = verifier.authority_errors(self.root)
    self.assertTrue(
        any("raw authority source must not have alternate source loaders" in error for error in errors)
    )

def test_rejects_parent_normalized_alternate_raw_authority_loader(self) -> None:
    nested = self.root / "src" / "nested"
    nested.mkdir()
    (nested / "consumer.rs").write_text(
        '#[path = "../bolt_v3_application_resource_ledger/'
        './risk_closure_workspace.rs"]\nmod shadow_authority;\n',
        encoding="utf-8",
    )
    errors = verifier.authority_errors(self.root)
    self.assertTrue(
        any("raw authority source must not have alternate source loaders" in error for error in errors)
    )
```

- [ ] **Step 2: Run both tests and verify RED**

Expected: both assertions fail because substring comparison does not resolve `.` or `..`.

- [ ] **Step 3: Resolve loader targets relative to the source file**

Replace the helper with:

```python
def _source_loader_targets(
    root: pathlib.Path,
    source: pathlib.Path,
    text: str,
) -> list[pathlib.Path]:
    code = strip_rust_comments_and_literals(text)
    literals = _rust_string_literals(text)
    protected = {
        (root / OWNER).resolve(): OWNER,
        (root / LEDGER).resolve(): LEDGER,
    }
    found: list[pathlib.Path] = []

    def inspect_range(start: int, end: int) -> None:
        joined = "".join(
            value
            for literal_start, literal_end, value in literals
            if start <= literal_start and literal_end <= end
        )
        if not joined:
            return
        target = pathlib.Path(joined)
        resolved = target.resolve() if target.is_absolute() else (source.parent / target).resolve()
        protected_target = protected.get(resolved)
        if protected_target is not None:
            found.append(protected_target)

    for start, end in _path_attribute_value_ranges(code):
        inspect_range(start, end)
    for match in INCLUDE_MACRO.finditer(code):
        opening = match.end("open") - 1
        closing = _matching_delimiter_end(code, opening)
        if closing is not None:
            inspect_range(opening + 1, closing)
    return found
```

Change the caller to `_source_loader_targets(root, path, active_text)`.

- [ ] **Step 4: Run all seven loader tests**

Run the two new tests together with `test_rejects_duplicate_application_ledger_module_loader`, `test_rejects_duplicate_raw_authority_module_loader`, `test_rejects_concatenated_raw_authority_include_loader`, `test_rejects_cfg_attr_path_module_loaders`, and `test_raw_string_loader_decoy_is_ignored`.

Expected: seven tests pass.

- [ ] **Step 5: Commit Task 5**

```bash
git add scripts/verify_risk_closure_workspace_authority.py scripts/test_verify_risk_closure_workspace_authority.py
git commit -m "fix(resources): canonicalize protected source loaders"
```

---

### Task 6: Full verification, adversarial replay, and publication

**Files:**
- Verify: `scripts/verify_risk_closure_workspace_authority.py`
- Verify: `scripts/test_verify_risk_closure_workspace_authority.py`
- Verify: `docs/superpowers/specs/2026-07-17-application-resource-ledger-fence-hardening-design.md`
- Verify: `docs/superpowers/plans/2026-07-17-application-resource-ledger-fence-hardening.md`

**Interfaces:**
- Consumes: all five completed red-green fixes.
- Produces: clean local non-compile evidence, an internal adversarial-review verdict, a committed exact head, and exact remote branch-head proof.

- [ ] **Step 1: Run the complete Python suite**

```bash
env PYTHONPATH=scripts python3 -m unittest scripts.test_verify_risk_closure_workspace_authority
```

Expected: zero failures and zero errors.

- [ ] **Step 2: Run the verifier directly**

```bash
python3 scripts/verify_risk_closure_workspace_authority.py
```

Expected: `OK: risk-closure workspace geometry has one TOML authority.`

- [ ] **Step 3: Run permitted repository gates**

```bash
just fmt-check
just deny
just ci-lint-workflow
just source-fence-static
```

Expected: every command exits zero. Do not substitute compile-heavy Rust commands.

- [ ] **Step 4: Conduct the final internal adversarial replay**

Run all new fully qualified mutation-test names together. Inspect the implementation for variants of conditional production visibility, public fields/statics/raw identifiers, conditional Clone/grouped conversion aliases, free functions, and normalized absolute/relative loaders. Any new bypass becomes another red-green mutation before proceeding.

- [ ] **Step 5: Check scope and cleanliness**

```bash
git diff f01ab7f1d1fde92cbdf211a5ce187be965f9e11a...HEAD --check
git status --short
git diff --stat f01ab7f1d1fde92cbdf211a5ce187be965f9e11a...HEAD
```

Expected: no whitespace errors, no uncommitted files, and no Rust runtime/API changes beyond the already-reviewed PR scope.

- [ ] **Step 6: Publish through the governed path**

```bash
just sandbox-safe-push
```

Expected: the configured remote branch head exactly equals local `HEAD`. Record the full SHA. Do not request another external review until applicable exact-head checks are green and all local findings remain resolved.
