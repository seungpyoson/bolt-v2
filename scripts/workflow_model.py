"""Shared parser helpers for repository workflow models."""

from __future__ import annotations

import functools
import re


YAML_ANCHOR_PATTERN = r"&[A-Za-z0-9_.-]+"
YAML_KEY_PATTERN = r"""(?:[A-Za-z0-9_.-]+|'[^']*(?:''[^']*)*'|"(?:[^"\\]|\\.)*")"""
YAML_STEP_ITEM_RE = re.compile(rf"^-\s+(?:{YAML_ANCHOR_PATTERN}(?:\s+|$))?")
YAML_RUN_LINE_RE = re.compile(rf"^(\s*)(?:-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?)?run:\s*(.*?)\s*$")


# verify_text re-parses the same shell strings tens of thousands of times across
# a run (e.g. `fi`, `exit 1`); these helpers are pure functions of a single str,
# so memoize. An unbounded cache is safe: the distinct-string set is bounded by
# the workflow corpus and the process is a short-lived CLI/test invocation.
@functools.cache
def strip_comment(line: str) -> str:
    quote: str | None = None
    escaped = False
    for index, char in enumerate(line):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\" and quote == '"':
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
            continue
        if char == "#" and (index == 0 or line[index - 1].isspace()):
            return line[:index].rstrip()
    return line.rstrip()


def parse_jobs(workflow_text: str) -> dict[str, list[str]]:
    """Parse this repo's strict GitHub Actions job subset.

    Top-level job ids must be indented by exactly two spaces under `jobs:`.
    The verifier reports required job ids that drift to another indentation.
    """

    lines = workflow_text.splitlines()
    jobs: dict[str, list[str]] = {}
    in_jobs = False
    current: str | None = None

    for line in lines:
        clean = strip_comment(line)
        if clean == "jobs:":
            in_jobs = True
            current = None
            continue
        if not in_jobs:
            continue
        if clean and not clean.startswith((" ", "\t")):
            break
        match = re.match(r"^  ([^ \t:#][^:#]*):(?:\s+&[A-Za-z0-9_.-]+)?\s*$", clean)
        if match:
            current = match.group(1).strip().strip("'\"")
            jobs[current] = []
            continue
        if current is not None:
            jobs[current].append(clean)
    return jobs


def step_blocks(job_lines: list[str]) -> list[list[str]]:
    blocks: list[list[str]] = []
    current: list[str] | None = None
    in_steps = False
    steps_indent: int | None = None
    step_indent: int | None = None

    for line in job_lines:
        clean = strip_comment(line)
        stripped = clean.lstrip()
        if not in_steps:
            if re.match(r"^\s*steps:\s*$", clean):
                in_steps = True
                steps_indent = len(clean) - len(stripped)
            continue
        if not stripped:
            if current is not None:
                current.append(line)
            continue
        indent = len(clean) - len(stripped)
        is_step_item = YAML_STEP_ITEM_RE.match(stripped) is not None
        if steps_indent is not None and indent <= steps_indent and not (
            indent == steps_indent and is_step_item
        ):
            break
        if step_indent is None and is_step_item:
            step_indent = indent
        if step_indent is not None and indent == step_indent and is_step_item:
            if current is not None:
                blocks.append(current)
            current = [line]
            continue
        if current is not None:
            current.append(line)
    if current is not None:
        blocks.append(current)
    return blocks


def uncommented_text(lines: list[str]) -> str:
    return "\n".join(strip_comment(line) for line in lines)


def block_run_body_lines(block: list[str]) -> list[str]:
    for index, line in enumerate(block):
        clean = strip_comment(line).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            continue
        value = match.group(2).strip().strip("'\"")
        if value not in {"|", ">"}:
            return [value] if value else []
        run_indent = len(clean) - len(clean.lstrip(" "))
        body_indent: int | None = None
        body: list[str] = []
        for nested in block[index + 1:]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                body.append("")
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(" "))
            if indent <= run_indent:
                break
            if body_indent is None:
                body_indent = indent
            body.append(nested_clean[body_indent:] if indent >= body_indent else nested_clean.lstrip())
        return body
    return []


def line_indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def unquote_yaml_scalar(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


def normalize_script_text(text: str) -> str:
    text = re.sub(r"\\\s*\n\s*", " ", text)
    lines = [line.rstrip() for line in text.strip("\n").splitlines()]
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    indents = [len(line) - len(line.lstrip(" ")) for line in lines if line.strip()]
    margin = min(indents) if indents else 0
    normalized_lines = [line[margin:] if line.strip() else "" for line in lines]
    return "\n".join(re.sub(r"(?<=\S) {2,}(?=\S)", " ", line) for line in normalized_lines)


def block_run_body(block: list[str]) -> str:
    for index, line in enumerate(block):
        clean = strip_comment(line).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            continue
        scalar = match.group(2).strip()
        if not scalar.startswith(("|", ">")):
            return unquote_yaml_scalar(scalar)
        run_indent = len(match.group(1))
        body_lines: list[str] = []
        for nested in block[index + 1 :]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                body_lines.append("")
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(" "))
            if indent <= run_indent:
                break
            body_lines.append(nested_clean)
        return normalize_script_text("\n".join(body_lines))
    return ""


def block_run_body_matches(block: list[str], expected: str) -> bool:
    return normalize_script_text(block_run_body(block)) == normalize_script_text(expected)


def block_step_property_indent(block: list[str]) -> int | None:
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        match = re.match(
            rf"^(\s*)-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?{YAML_KEY_PATTERN}\s*:\s*.*$",
            clean,
        )
        if match is None:
            return None
        return len(match.group(1)) + 2
    return None


def block_top_level_items(block: list[str]) -> dict[str, str] | None:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return None
    step_item_indent = property_indent - 2
    items: dict[str, str] = {}
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        step_match = re.match(
            rf"^(\s*)-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$",
            clean,
        )
        if step_match is not None:
            if len(step_match.group(1)) != step_item_indent:
                continue
            key = unquote_yaml_scalar(step_match.group(2))
            value = step_match.group(3)
        else:
            indent = len(clean) - len(clean.lstrip(" "))
            if indent != property_indent:
                continue
            item_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
            if item_match is None:
                return None
            key = unquote_yaml_scalar(item_match.group(1))
            value = item_match.group(2)
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(value)
    return items


def block_nested_mapping_items(block: list[str], parent_key: str) -> dict[str, str] | None:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return None
    parent_indent: int | None = None
    item_indent: int | None = None
    items: dict[str, str] = {}
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if parent_indent is None:
            parent_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
            if (
                parent_match is not None
                and indent == property_indent
                and unquote_yaml_scalar(parent_match.group(1)) == parent_key
                and unquote_yaml_scalar(parent_match.group(2)) == ""
            ):
                parent_indent = indent
            continue
        if indent <= parent_indent:
            break
        if item_indent is None:
            item_indent = indent
        if indent != item_indent:
            continue
        item_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items


def top_level_mapping_items(workflow_text: str, top_key: str) -> dict[str, str] | None:
    lines = workflow_text.splitlines()
    top_index: int | None = None
    for index, line in enumerate(lines):
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if indent != 0:
            continue
        top_match = re.match(rf"^({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if top_match is None:
            continue
        if unquote_yaml_scalar(top_match.group(1)) != top_key:
            continue
        if top_index is not None or unquote_yaml_scalar(top_match.group(2)) != "":
            return None
        top_index = index
    if top_index is None:
        return None

    item_indent: int | None = None
    items: dict[str, str] = {}
    for line in lines[top_index + 1 :]:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if indent == 0:
            break
        if item_indent is None:
            item_indent = indent
        if indent != item_indent:
            return None
        item_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items


def job_top_level_items(job_lines: list[str]) -> dict[str, str] | None:
    items: dict[str, str] = {}
    for line in job_lines:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if indent != 4:
            continue
        item_match = re.match(rf"^\s{{4}}({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items


def named_step_block(lines: list[str], step_name: str) -> list[str] | None:
    name_re = re.compile(rf"^\s*(?:-\s*)?name:\s*{re.escape(step_name)}\s*$")
    for block in step_blocks(lines):
        if any(name_re.match(strip_comment(line)) for line in block):
            return block
    return None
