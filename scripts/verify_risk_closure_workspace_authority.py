#!/usr/bin/env python3
"""Verify that workspace size has one TOML authority and generated Rust."""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tomllib

from rust_source_scanner import (
    char_literal_end,
    quoted_literal_end,
    raw_string_end,
    strip_rust_comments_and_literals,
)
from verify_bolt_v3_provider_leaks import cfg_truth_without_test, production_text


SOURCE = pathlib.Path("config/risk-closure-workspaces.toml")
GENERATED = pathlib.Path(
    "src/bolt_v3_application_resource_ledger/risk_closure_workspace/generated.rs"
)
OWNER = pathlib.Path(
    "src/bolt_v3_application_resource_ledger/risk_closure_workspace.rs"
)
LEDGER = pathlib.Path("src/bolt_v3_application_resource_ledger.rs")
LIB = pathlib.Path("src/lib.rs")
RAW_AUTHORITY_SOURCE_PATH = "bolt_v3_application_resource_ledger/risk_closure_workspace.rs"
LEDGER_SOURCE_PATH = "bolt_v3_application_resource_ledger.rs"
RAW_AUTHORITY_CONSTRUCTION = (
    "RiskClosureWorkspaceAuthority::for_disabled_application_resource_ledger"
)
RAW_CHECKOUT = re.compile(r"\.checkout_(?:new_risk|recovery)\s*\(")
LEDGER_DISTRIBUTION = re.compile(
    r"\b(?:ApplicationResourceLedger|NewRiskWorkspaceHandle|RecoveryWorkspaceHandle|"
    r"new_risk_workspace_handle|recovery_workspace_handle)\b"
)
FUNCTION_HEADER = re.compile(
    r"(?P<attributes>(?:#\[[^\]]+\]\s*)*)"
    r"(?P<visibility>pub\s*(?:\([^)]*\))?\s+)?"
    r"(?P<qualifiers>(?:(?:const|async|unsafe|extern)\s+)*)"
    r"fn\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\(",
    re.MULTILINE,
)
ATTRIBUTE_HEADER = re.compile(r"#\s*\[\s*(?P<name>path|cfg_attr)\b")
INCLUDE_MACRO = re.compile(r"\binclude\s*!\s*(?P<open>[({\[])")
LEGACY_RAW_AUTHORITY_MODULE_DECLARATION = re.compile(
    r"\b(?:pub(?:\([^)]*\))?\s+)?mod\s+bolt_v3_risk_closure_workspace\s*;"
)
RAW_AUTHORITY_CHILD_MODULE_DECLARATION = re.compile(
    r"\b(?P<visibility>pub(?:\([^)]*\))?\s+)?mod\s+risk_closure_workspace\s*;"
)
LEDGER_MODULE_DECLARATION = re.compile(
    r"\b(?:pub(?:\([^)]*\))?\s+)?mod\s+bolt_v3_application_resource_ledger\s*;"
)
RUST_PATH = r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*"
TYPE_ALIAS = re.compile(
    rf"\btype\s+(?P<alias>[A-Za-z_][A-Za-z0-9_]*)"
    rf"(?:\s*<[^;={{}}]*>)?\s*=\s*(?P<target>{RUST_PATH})"
)
USE_ALIAS = re.compile(
    rf"\buse\s+(?P<target>{RUST_PATH})\s+as\s+"
    r"(?P<alias>[A-Za-z_][A-Za-z0-9_]*)\s*;"
)
IMPL_HEADER = re.compile(
    rf"(?P<attributes>(?:#\[[^\]]+\]\s*)*)\bimpl(?:\s*<[^>{{}}]*>)?\s+"
    rf"(?P<target>{RUST_PATH})(?:\s*<[^>{{}}]*>)?"
    r"(?:\s+where\s+[^{}]*)?\s*\{"
)
TRAIT_IMPL_HEADER = re.compile(
    rf"\bimpl(?:\s*<[^>{{}}]*>)?\s+(?P<trait>{RUST_PATH})"
    rf"(?:\s*<[^>{{}}]*>)?\s+for\s+(?P<target>{RUST_PATH})\b"
)
RUST_INTEGER = (
    r"(?:0[xX][0-9a-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|[0-9][0-9_]*)"
    r"(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))?"
)
INTEGER_LITERAL = re.compile(rf"(?<![A-Za-z0-9_]){RUST_INTEGER}(?![A-Za-z0-9_])")
SYMBOLIC_AUTHORITY = re.compile(r"\b(?:const|static)\s+([A-Z][A-Z0-9_]*)\b")
INTEGER_SUFFIX = re.compile(r"(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))$")
IGNORED_TOML_PATH_PARTS = frozenset({".git", ".worktrees", "target"})


def _integer_value(literal: str) -> int:
    normalized = INTEGER_SUFFIX.sub("", literal).replace("_", "")
    if normalized.lower().startswith("0x"):
        base = 16
    elif normalized.lower().startswith("0o"):
        base = 8
    elif normalized.lower().startswith("0b"):
        base = 2
    else:
        base = 10
    digits = normalized[2:] if base != 10 else normalized
    return int(digits, base)


def _production_rust_sources(root: pathlib.Path) -> list[pathlib.Path]:
    sources = set((root / "src").rglob("*.rs"))
    build_script = root / "build.rs"
    if build_script.is_file():
        sources.add(build_script)
    crates = root / "crates"
    if crates.is_dir():
        for path in crates.rglob("*.rs"):
            relative_parts = path.relative_to(crates).parts
            if "src" in relative_parts or path.name == "build.rs":
                sources.add(path)
    return sorted(sources)


def _repository_toml_sources(root: pathlib.Path) -> list[pathlib.Path]:
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not IGNORED_TOML_PATH_PARTS.intersection(path.relative_to(root).parts)
    )


def _positive_integer(value: object) -> int | None:
    if isinstance(value, int) and not isinstance(value, bool) and value > 0:
        return value
    return None


def _matching_delimiter_end(text: str, start: int) -> int | None:
    pairs = {"(": ")", "[": "]", "{": "}"}
    opening = text[start] if start < len(text) else ""
    closing = pairs.get(opening)
    if closing is None:
        return None
    stack = [closing]
    for index in range(start + 1, len(text)):
        char = text[index]
        if char in pairs:
            stack.append(pairs[char])
        elif stack and char == stack[-1]:
            stack.pop()
            if not stack:
                return index
    return None


def _decode_rust_string(token: str) -> str:
    if token.startswith("r"):
        quote = token.find('"')
        hashes = token[1:quote]
        return token[quote + 1 : -(len(hashes) + 1)]
    content = token[1:-1]
    output: list[str] = []
    index = 0
    simple_escapes = {
        "0": "\0",
        "t": "\t",
        "n": "\n",
        "r": "\r",
        '"': '"',
        "'": "'",
        "\\": "\\",
    }
    while index < len(content):
        if content[index] != "\\":
            output.append(content[index])
            index += 1
            continue
        index += 1
        if index >= len(content):
            break
        escape = content[index]
        if escape in simple_escapes:
            output.append(simple_escapes[escape])
            index += 1
        elif escape == "x" and index + 2 < len(content):
            output.append(chr(int(content[index + 1 : index + 3], 16)))
            index += 3
        elif escape == "u" and index + 1 < len(content) and content[index + 1] == "{":
            end = content.find("}", index + 2)
            if end == -1:
                break
            output.append(chr(int(content[index + 2 : end].replace("_", ""), 16)))
            index = end + 1
        elif escape in {"\n", "\r"}:
            index += 1
            if escape == "\r" and index < len(content) and content[index] == "\n":
                index += 1
            while index < len(content) and content[index].isspace():
                index += 1
        else:
            output.append(escape)
            index += 1
    return "".join(output)


def _rust_string_literals(text: str) -> list[tuple[int, int, str]]:
    literals: list[tuple[int, int, str]] = []
    index = 0
    while index < len(text):
        raw_end = raw_string_end(text, index)
        if raw_end is not None:
            if text[index] == "r":
                literals.append((index, raw_end, _decode_rust_string(text[index:raw_end])))
            index = raw_end
            continue
        if text.startswith("//", index):
            end = text.find("\n", index)
            index = len(text) if end == -1 else end
            continue
        if text.startswith("/*", index):
            depth = 1
            cursor = index + 2
            while cursor < len(text) and depth:
                if text.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif text.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            index = cursor
            continue
        if text[index] in {"b", "c"} and index + 1 < len(text) and text[index + 1] == '"':
            index = quoted_literal_end(text, index + 1, '"')
            continue
        if text[index] == '"':
            end = quoted_literal_end(text, index, '"')
            literals.append((index, end, _decode_rust_string(text[index:end])))
            index = end
            continue
        if text[index] == "'":
            end = char_literal_end(text, index)
            if end is not None:
                index = end
                continue
        index += 1
    return literals


def _top_level_segments(text: str, start: int, end: int) -> list[tuple[int, int]]:
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack: list[str] = []
    segments: list[tuple[int, int]] = []
    segment_start = start
    for index in range(start, end):
        char = text[index]
        if char in pairs:
            stack.append(pairs[char])
        elif stack and char == stack[-1]:
            stack.pop()
        elif char == "," and not stack:
            segments.append((segment_start, index))
            segment_start = index + 1
    segments.append((segment_start, end))
    return segments


def _cfg_attr_path_ranges(text: str, start: int, end: int) -> list[tuple[int, int]]:
    header = re.match(r"\s*cfg_attr\b", text[start:end])
    if header is None:
        return []
    opening = text.find("(", start + header.end(), end)
    if opening == -1:
        return []
    closing = _matching_delimiter_end(text, opening)
    if closing is None or closing > end:
        return []
    ranges: list[tuple[int, int]] = []
    segments = _top_level_segments(text, opening + 1, closing)
    for segment_start, segment_end in segments[1:]:
        segment = text[segment_start:segment_end]
        path = re.match(r"\s*path\s*=", segment)
        if path is not None:
            ranges.append((segment_start + path.end(), segment_end))
        else:
            ranges.extend(_cfg_attr_path_ranges(text, segment_start, segment_end))
    return ranges


def _path_attribute_value_ranges(text: str) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    for match in ATTRIBUTE_HEADER.finditer(text):
        opening = text.find("[", match.start(), match.end())
        closing = _matching_delimiter_end(text, opening)
        if closing is None:
            continue
        if match.group("name") == "path":
            equals = re.match(r"\s*=", text[match.end() : closing])
            if equals is not None:
                ranges.append((match.end() + equals.end(), closing))
        else:
            ranges.extend(_cfg_attr_path_ranges(text, match.start("name"), closing))
    return ranges


def _source_loader_targets(text: str) -> list[str]:
    code = strip_rust_comments_and_literals(text)
    literals = _rust_string_literals(text)
    targets = (
        RAW_AUTHORITY_SOURCE_PATH,
        LEDGER_SOURCE_PATH,
    )
    found: list[str] = []

    def inspect_range(start: int, end: int) -> None:
        joined = "".join(
            value
            for literal_start, literal_end, value in literals
            if start <= literal_start and literal_end <= end
        )
        found.extend(target for target in targets if target in joined)

    for start, end in _path_attribute_value_ranges(code):
        inspect_range(start, end)
    for match in INCLUDE_MACRO.finditer(code):
        opening = match.end("open") - 1
        closing = _matching_delimiter_end(code, opening)
        if closing is not None:
            inspect_range(opening + 1, closing)
    return found


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
        condition_start, condition_end = segments[0]
        can_be_true, _ = cfg_truth_without_test(
            attributes[condition_start:condition_end]
        )
        if can_be_true:
            payloads.extend(
                attributes[start:end].strip() for start, end in segments[1:]
            )
    return payloads


def _protected_type_names(text: str, type_name: str) -> set[str]:
    names = {type_name}
    changed = True
    while changed:
        changed = False
        for pattern in (TYPE_ALIAS, USE_ALIAS):
            for match in pattern.finditer(text):
                if match.group("target").split("::")[-1] not in names:
                    continue
                alias = match.group("alias")
                if alias not in names:
                    names.add(alias)
                    changed = True
    return names


def _impl_bodies(text: str, type_name: str) -> list[tuple[str, str]]:
    protected_names = _protected_type_names(text, type_name)
    bodies: list[tuple[str, str]] = []
    for match in IMPL_HEADER.finditer(text):
        if match.group("target").split("::")[-1] not in protected_names:
            continue
        body_start = match.end()
        body_end = _matching_delimiter_end(text, body_start - 1)
        if body_end is None:
            bodies.append((match.group("attributes"), text[body_start:]))
        else:
            bodies.append((match.group("attributes"), text[body_start:body_end]))
    return bodies


def _function_definitions(text: str) -> list[tuple[str, str, str, str, str, str]]:
    definitions: list[tuple[str, str, str, str, str, str]] = []
    for match in FUNCTION_HEADER.finditer(text):
        arguments_start = match.end() - 1
        arguments_end = _matching_delimiter_end(text, arguments_start)
        if arguments_end is None:
            continue
        cursor = arguments_end + 1
        while cursor < len(text) and text[cursor].isspace():
            cursor += 1
        returns = "()"
        if text.startswith("->", cursor):
            return_start = cursor + 2
            body_start = text.find("{", return_start)
            terminator = text.find(";", return_start)
            candidates = [value for value in (body_start, terminator) if value != -1]
            if not candidates:
                continue
            return_end = min(candidates)
            returns = text[return_start:return_end]
        elif cursor >= len(text) or text[cursor] not in {"{", ";"}:
            body_start = text.find("{", cursor)
            if body_start == -1:
                continue
        definitions.append(
            (
                match.group("attributes"),
                (match.group("visibility") or "").strip(),
                _normalize_rust_fragment(match.group("qualifiers")),
                match.group("name"),
                text[arguments_start + 1 : arguments_end],
                returns,
            )
        )
    return definitions


def _constructor_definitions(text: str, type_name: str) -> list[tuple[str, bool, str]]:
    constructors: list[tuple[str, bool, str]] = []
    protected_names = _protected_type_names(text, type_name)
    for impl_attributes, body in _impl_bodies(text, type_name):
        for function_attributes, visibility, _, name, _, returns in _function_definitions(body):
            if not any(
                re.search(rf"\b{re.escape(protected_name)}\b", returns)
                for protected_name in {"Self", *protected_names}
            ):
                continue
            constructors.append(
                (
                    name,
                    "#[cfg(test)]" in impl_attributes
                    or "#[cfg(test)]" in function_attributes,
                    visibility,
                )
            )
    return constructors


def _normalize_rust_fragment(value: str) -> str:
    return re.sub(r"\s+", "", value).rstrip(",")


def _public_function_surface(text: str) -> list[tuple[str, str, str, str, str]]:
    surface: list[tuple[str, str, str, str, str]] = []
    for _, visibility, qualifiers, name, arguments, returns in _function_definitions(text):
        if not visibility.startswith("pub"):
            continue
        surface.append(
            (
                visibility,
                qualifiers,
                name,
                _normalize_rust_fragment(arguments),
                _normalize_rust_fragment(returns),
            )
        )
    return surface


def _has_protected_trait_impl(
    text: str,
    protected_type_names: set[str],
    trait_name: str | None = None,
) -> bool:
    for match in TRAIT_IMPL_HEADER.finditer(text):
        target = match.group("target").split("::")[-1]
        implemented_trait = match.group("trait").split("::")[-1]
        if target in protected_type_names and (
            trait_name is None or implemented_trait == trait_name
        ):
            return True
    return False


def _toml_key_paths(
    value: object,
    target: str,
    prefix: tuple[str | int, ...] = (),
) -> list[tuple[str | int, ...]]:
    paths: list[tuple[str | int, ...]] = []
    if isinstance(value, dict):
        for key, child in value.items():
            path = (*prefix, key)
            if key == target:
                paths.append(path)
            paths.extend(_toml_key_paths(child, target, path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            paths.extend(_toml_key_paths(child, target, (*prefix, index)))
    return paths


def _render_toml_key_path(path: tuple[str | int, ...]) -> str:
    rendered = ""
    for component in path:
        if isinstance(component, int):
            rendered += f"[{component}]"
        else:
            rendered += f"[{json.dumps(component)}]"
    return rendered


def authority_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    authorities: list[tuple[pathlib.Path, tuple[str | int, ...]]] = []
    authoritative_arena_bytes: int | None = None
    authoritative_slot_bytes: int | None = None
    for path in _repository_toml_sources(root):
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect {path.relative_to(root)}: {error}")
            continue
        relative = path.relative_to(root)
        for key_path in _toml_key_paths(document, "risk_closure_workspaces"):
            authorities.append((relative, key_path))
            if relative != SOURCE or key_path != ("risk_closure_workspaces",):
                continue
            workspace = document["risk_closure_workspaces"]
            if not isinstance(workspace, dict):
                errors.append(f"{SOURCE} risk_closure_workspaces must be a table")
                continue
            authoritative_arena_bytes = _positive_integer(workspace.get("arena_bytes"))
            authoritative_slot_bytes = _positive_integer(workspace.get("slot_bytes"))
            if authoritative_arena_bytes is None:
                errors.append(
                    f"{SOURCE} risk_closure_workspaces.arena_bytes must be a positive integer"
                )
            if authoritative_slot_bytes is None:
                errors.append(
                    f"{SOURCE} risk_closure_workspaces.slot_bytes must be a positive integer"
                )
    expected_authority = [(SOURCE, ("risk_closure_workspaces",))]
    if authorities != expected_authority:
        rendered_authorities = [
            f"{path}::{_render_toml_key_path(key_path)}"
            for path, key_path in authorities
        ]
        errors.append(
            "risk_closure_workspaces geometry must have exactly one TOML authority at "
            f"{SOURCE}::risk_closure_workspaces; found {rendered_authorities}"
        )
    if authoritative_arena_bytes is None or authoritative_slot_bytes is None:
        return errors
    authoritative_sizes = {authoritative_arena_bytes, authoritative_slot_bytes}

    try:
        owner_text = (root / OWNER).read_text(encoding="utf-8")
        generated_text = (root / GENERATED).read_text(encoding="utf-8")
        ledger_text = (root / LEDGER).read_text(encoding="utf-8")
        lib_text = (root / LIB).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        errors.append(f"cannot inspect private workspace authority surface: {error}")
        return errors
    owner_code = strip_rust_comments_and_literals(owner_text)
    owner_production_code = strip_rust_comments_and_literals(production_text(owner_text))
    generated_code = strip_rust_comments_and_literals(generated_text)
    ledger_code = strip_rust_comments_and_literals(ledger_text)
    lib_code = strip_rust_comments_and_literals(lib_text)
    if re.search(r"\bpub\s+(?:\([^)]*\)\s+)?struct\s+RiskClosureWorkspaceConfig\b", owner_code):
        errors.append(f"workspace configuration type must remain private to {OWNER}")
    if re.search(r"\bpub\s+(?:\([^)]*\)\s+)?const\s+RISK_CLOSURE_WORKSPACE_CONFIG\b", generated_code):
        errors.append(f"generated workspace configuration must remain private to {OWNER}")
    if "const RISK_CLOSURE_WORKSPACE_CONFIG" not in generated_code:
        errors.append(f"generated workspace configuration is missing from {GENERATED}")
    raw_definitions = list(
        re.finditer(
            r"(?P<attributes>(?:#\[[^\]]+\]\s*)*)"
            r"(?P<visibility>pub(?:\([^)]*\))?)?\s*struct\s+"
            r"(?:r#)?RiskClosureWorkspaceAuthority\b",
            owner_production_code,
        )
    )
    if (
        len(raw_definitions) != 1
        or raw_definitions[0].group("visibility") != "pub(super)"
    ):
        errors.append(f"raw workspace authority must remain ledger-private in {OWNER}")
    raw_authority_names = _protected_type_names(
        owner_production_code, "RiskClosureWorkspaceAuthority"
    )
    raw_definition_attributes = (
        raw_definitions[0].group("attributes") if len(raw_definitions) == 1 else ""
    )
    direct_clone = bool(
        re.search(r"#\[derive\([^\]]*\bClone\b[^\]]*\)\]", raw_definition_attributes)
    )
    conditional_clone = any(
        re.search(r"\bderive\s*\([^)]*\bClone\b", payload)
        for payload in _production_cfg_attr_payloads(raw_definition_attributes)
    )
    if (
        direct_clone
        or conditional_clone
        or _has_protected_trait_impl(
            owner_production_code, raw_authority_names, "Clone"
        )
    ):
        errors.append("raw workspace authority must not implement Clone")
    authority_constructors = _constructor_definitions(
        owner_code, "RiskClosureWorkspaceAuthority"
    )
    if authority_constructors != [
        ("for_disabled_application_resource_ledger", True, "pub(super)"),
        ("with_config", True, ""),
    ]:
        errors.append(
            f"{OWNER} must contain exactly the two test-only constructor definitions; "
            f"found {authority_constructors}"
        )
    child_declarations = list(
        RAW_AUTHORITY_CHILD_MODULE_DECLARATION.finditer(ledger_code)
    )
    if len(child_declarations) != 1 or child_declarations[0].group("visibility"):
        errors.append(f"{LEDGER} must privately own the raw workspace authority module")
    if re.search(r"\bpub\s+mod\s+bolt_v3_risk_closure_workspace\s*;", lib_code):
        errors.append("raw workspace authority module must not be public")
    if lib_code.count("pub mod bolt_v3_application_resource_ledger;") != 1:
        errors.append(f"{LIB} must expose exactly one application resource ledger module")
    if ledger_code.count(RAW_AUTHORITY_CONSTRUCTION) != 1:
        errors.append(f"{LEDGER} must contain exactly one raw authority construction call")
    ledger_constructors = _constructor_definitions(ledger_code, "ApplicationResourceLedger")
    if ledger_constructors != [("new_disabled", True, "")]:
        errors.append(
            f"{LEDGER} must contain exactly one test-only constructor definition; "
            f"found {ledger_constructors}"
        )

    ledger_production_code = strip_rust_comments_and_literals(production_text(ledger_text))
    expected_public_surface = [
        ("pub", "", "new_risk_workspace_handle", "&self", "NewRiskWorkspaceHandle"),
        ("pub", "", "recovery_workspace_handle", "&self", "RecoveryWorkspaceHandle"),
        (
            "pub",
            "",
            "reserve_new_risk_workspace",
            "&self",
            "Result<RiskClosureWorkspaceReservation,RiskClosureWorkspaceError>",
        ),
        (
            "pub",
            "",
            "checkout_retained_recovery_workspace",
            "&self,closure_identity:&ClosureIdentity",
            "Result<RiskClosureWorkspaceLease,RiskClosureWorkspaceError>",
        ),
    ]
    public_surface = _public_function_surface(ledger_production_code)
    if public_surface != expected_public_surface:
        errors.append(
            f"{LEDGER} must expose the exact public capability surface; found {public_surface}"
        )
    protected_ledger_names = set().union(
        *(
            _protected_type_names(ledger_production_code, type_name)
            for type_name in (
                "ApplicationResourceLedger",
                "NewRiskWorkspaceHandle",
                "RecoveryWorkspaceHandle",
            )
        )
    )
    if _has_protected_trait_impl(ledger_production_code, protected_ledger_names):
        errors.append(
            "application ledger and capability handles must not implement construction or "
            "conversion traits"
        )
    if re.search(
        r"\bpub(?:\([^)]*\))?\s+(?:use|type)\b[^;\n]*"
        r"\bRiskClosureWorkspaceAuthority\b",
        ledger_production_code,
    ):
        errors.append("raw workspace authority must not appear in the public ledger surface")

    raw_authority_source_loaders: list[pathlib.Path] = []
    ledger_source_loaders: list[pathlib.Path] = []
    raw_authority_child_module_declarations: list[tuple[pathlib.Path, str]] = []
    legacy_raw_authority_module_declarations: list[pathlib.Path] = []
    ledger_module_declarations: list[pathlib.Path] = []
    for path in _production_rust_sources(root):
        relative = path.relative_to(root)
        if relative == GENERATED:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect {relative}: {error}")
            continue
        active_text = production_text(text)
        code_text = strip_rust_comments_and_literals(active_text)
        loader_targets = _source_loader_targets(active_text)
        raw_authority_source_loaders.extend(
            relative
            for target in loader_targets
            if target == RAW_AUTHORITY_SOURCE_PATH
        )
        ledger_source_loaders.extend(
            relative
            for target in loader_targets
            if target == LEDGER_SOURCE_PATH
        )
        raw_authority_child_module_declarations.extend(
            (relative, (match.group("visibility") or "").strip())
            for match in RAW_AUTHORITY_CHILD_MODULE_DECLARATION.finditer(code_text)
        )
        legacy_raw_authority_module_declarations.extend(
            relative
            for _ in LEGACY_RAW_AUTHORITY_MODULE_DECLARATION.finditer(code_text)
        )
        ledger_module_declarations.extend(
            relative for _ in LEDGER_MODULE_DECLARATION.finditer(code_text)
        )
        if relative != OWNER and re.search(
            r"\b(?:RiskClosureWorkspaceConfig|RISK_CLOSURE_WORKSPACE_CONFIG)\b", code_text
        ):
            errors.append(f"private workspace configuration referenced outside {OWNER}: {relative}")
        if relative not in {OWNER, LEDGER} and re.search(
            r"\bRiskClosureWorkspaceAuthority\b", code_text
        ):
            errors.append(f"raw workspace authority referenced outside ledger: {relative}")
        if relative not in {OWNER, LEDGER} and RAW_CHECKOUT.search(code_text):
            errors.append(f"raw workspace checkout bypass outside ledger: {relative}")
        if relative != LEDGER and LEDGER_DISTRIBUTION.search(code_text):
            errors.append(
                f"production ledger construction or distribution call site outside {LEDGER}: "
                f"{relative}"
            )
        if any(
            _integer_value(match.group()) in authoritative_sizes
            for match in INTEGER_LITERAL.finditer(text)
        ):
            errors.append(f"runtime workspace-size literal found outside generated Rust: {relative}")
        symbolic_names = (match.group(1) for match in SYMBOLIC_AUTHORITY.finditer(text))
        if any(
            "CLOSURE" in name
            and ("WORKSPACE" in name or "SLOT" in name or "ARENA" in name)
            and ("BYTES" in name or "SIZE" in name)
            for name in symbolic_names
        ):
            errors.append(f"symbolic workspace-size authority found outside generated Rust: {relative}")
    if raw_authority_source_loaders:
        errors.append(
            "raw authority source must not have alternate source loaders; "
            f"found {raw_authority_source_loaders}"
        )
    if ledger_source_loaders:
        errors.append(
            "application ledger source must not have alternate module loaders; "
            f"found {ledger_source_loaders}"
        )
    if raw_authority_child_module_declarations != [(LEDGER, "")]:
        errors.append(
            "raw authority must have exactly one private child module declaration at "
            f"{LEDGER}; found {raw_authority_child_module_declarations}"
        )
    if legacy_raw_authority_module_declarations:
        errors.append(
            "raw authority source must not have a legacy top-level module declaration; "
            f"found {legacy_raw_authority_module_declarations}"
        )
    if ledger_module_declarations != [LIB]:
        errors.append(
            f"application ledger must have exactly one top-level module declaration at {LIB}; "
            f"found {ledger_module_declarations}"
        )
    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = authority_errors(root)
    generation = subprocess.run(
        [
            sys.executable,
            str(root / "scripts" / "generate_risk_closure_workspace_config.py"),
            "--source",
            str(root / SOURCE),
            "--output",
            str(root / GENERATED),
            "--check",
        ],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if generation.returncode != 0:
        errors.append(generation.stderr.strip() or "generated Rust configuration is stale")
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: risk-closure workspace geometry has one TOML authority.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
