#!/usr/bin/env python3
"""Verify Bolt-v3 one-way dependency direction.

The decomposition architecture contract
(`specs/522-decompose-strategy-monolith/architecture-contract.md` §2) requires a
one-way dependency: the strategy may use shared (`bolt_v3_*`) and family
(`bolt_v3_market_families/*`) modules, but those shared/family modules must NEVER
reference the strategy layer (`crate::strategies`). The three existing fences
(naming, provider-leaks, core-boundary) do not catch a shared module reaching
into `crate::strategies`; this fence does.

A first version scanned only `use` statements with a regex. Three independent
reviewers showed that is evadable: a shared module can depend on the strategy
layer through a *fully-qualified inline path* (a type annotation, function call,
macro argument, attribute, or turbofish) with no `use` at all, e.g.

    let p: crate::strategies::registry::FeeProvider = ...;   // no `use`
    crate::strategies::registry::register(...);
    foo!(crate::strategies::binary_oracle_edge_taker::KEY);

and that a hand-rolled comment/string stripper mis-handles raw strings, char
literals, lifetimes, and `#[attr]`-prefixed import segments — producing both
false negatives (a real reference slips through) and false positives (a literal
that merely contains the text `crate::strategies`).

So this fence LEXES each file with a small but correct Rust tokenizer that
discards comments (`//`, nested `/* */`), string/byte/raw-string literals,
char/byte-char literals, and lifetimes, then detects strategy references over the
token stream. It catches BOTH spellings in one pass:

  * `use` trees — including grouped/nested/multi-line imports, `as` aliases, glob
    imports, and `#[cfg(...)]`-prefixed members inside a brace group; and
  * inline fully-qualified paths anywhere in code.

It also fail-closes on source forms that could hide the dependency from a source
scanner: `include!`, `#[path]`/`cfg_attr(path = ...)`, crate-root namespace
aliases such as `use crate as alias`, `super::super as alias` from an inline
module that reaches the crate root, and `extern crate self as alias`.

For every detected path it resolves `crate::`, `self::`, and `super::` roots to
absolute crate-rooted module paths before checking whether the path enters
`crate::strategies`. Resolution uses the LEXICAL module stack — inline
`mod NAME { ... }` blocks deepen the module — so `super::super::strategies` from
inside `mod tests {}` in a top-level `bolt_v3_*` file resolves to the crate root
and is flagged, while `super::strategies` from a nested file module is not. A
bare root (`strategies::...`) is an external crate and is never flagged.

Each violation is keyed by (file, the absolute strategy path it reaches) — NOT by
a surrounding `use` block or a line — so the allowlist is stable against
unrelated edits and a newly-added strategy symbol is caught even inside an
already-coupled `use` block.

Current code already contains pre-existing back-references (tracked under #446 and
the #522 decomposition). They are captured in `FINDING_ALLOWANCES` so the fence is
GREEN on today's code while FAILING on every NEW back-reference. The allowlist may
only SHRINK:

- a stale allowance (one that no longer matches any reference) FAILS, forcing its
  removal once the underlying reference is relocated to a shared module;
- ADDING an allowance is rejected mechanically: the separate
  `--check-shrink-only-vs-main` mode (run in CI via `just source-fence`) fails
  unless the in-tree allowlist is a subset of the one on `origin/main`. A new
  back-reference is a bug to fix, not to allow. (This is a no-op on the PR that
  first introduces the fence — there is no mainline baseline yet — and active on
  every PR thereafter.)

KNOWN LIMITATION (documented, not a bug): the fence reads source text, so it
cannot see a path that only exists *after macro expansion* — i.e. a strategy path
synthesized from tokens by a macro at compile time. No source-level scanner can.
The contract requires manual review for macro-generated cross-layer references.
Like the sibling fences, it also does not resolve aliased re-exports that launder
a strategy type through a third module under a new name.
"""

from __future__ import annotations

import ast
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from git_remote_utils import fetchable_remote_url  # noqa: E402

REPO_ROOT = SCRIPT_DIR.parent
MAX_SCAN_FILE_BYTES = 1024 * 1024

# Shared/family layer = everything under `src/bolt_v3_*` (top-level files and
# their subdirectories, including `bolt_v3_market_families/`). The strategy layer
# (`src/strategies/**`) is intentionally NOT scanned — it MAY use shared/family.
SCAN_PREFIX = "src/bolt_v3_"

# The forbidden destination: the crate-root `strategies` module.
STRATEGY_ROOT = "strategies"

MESSAGE = (
    "shared/family module references the strategy layer (crate::strategies); "
    "violates one-way dependency (contract §2)"
)


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    strategy_path: str

    def render(self, prefix: str) -> str:
        return f"{prefix}: {self.path}:{self.line}: {MESSAGE}: references {self.strategy_path}"


@dataclass(frozen=True)
class FindingAllowance:
    """A FROZEN pre-existing back-reference, keyed by file + absolute strategy path.

    The allowlist may only shrink — never add entries.
    """

    path: str
    strategy_path: str


class PolicyError(Exception):
    """Raised when the dependency-direction policy cannot be evaluated safely."""


# Pre-existing strategy back-references in the shared/family layer, frozen from
# the verifier's own output on the current mainline tree this fence is rebased
# onto. Each entry is removed when its underlying reference is relocated to a
# shared module. Do not add entries for branch-local references — a new
# back-reference is a bug to fix.
#
# All of these resolve under the #522 decomposition + #446 (relocate the shared
# FeeProvider trait and the strategy-owned entry-decision evidence types out of
# `strategies::` into shared modules).
FINDING_ALLOWANCES: tuple[FindingAllowance, ...] = (
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::binary_oracle_edge_taker::KEY"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::production_strategy_registry"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::registry::StrategyBuildContext"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::registry::StrategyBuilder"),
    FindingAllowance("src/bolt_v3_providers/mod.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_providers/hyperliquid.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_providers/polymarket.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_providers/polymarket/fees.rs", "strategies::registry::FeeProvider"),
)


# --------------------------------------------------------------------------- #
# Rust lexer
#
# We do not need a full parser — only a tokenizer that reliably DISCARDS the
# lexical contexts in which the text `crate::strategies` must be ignored
# (comments and every string/char/lifetime form), and emits identifiers and the
# handful of punctuation tokens (`::`, `{`, `}`, `,`, `;`, `#`, `[`, `]`, `*`,
# `as`) that path/use-tree detection needs. Anything else is emitted as a generic
# single-character punctuation token and ignored by the detectors.
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class Token:
    kind: str  # "IDENT" or "PUNCT"
    value: str
    line: int
    raw: bool = False  # raw identifier (`r#ident`); never treated as a keyword


def _is_ident_start(ch: str) -> bool:
    return ch.isalpha() or ch == "_"


def _is_ident_continue(ch: str) -> bool:
    return ch.isalnum() or ch == "_"


def tokenize(text: str) -> list[Token]:
    tokens: list[Token] = []
    i = 0
    n = len(text)
    line = 1
    while i < n:
        ch = text[i]

        if ch == "\n":
            line += 1
            i += 1
            continue
        if ch in " \t\r\f\v":
            i += 1
            continue

        # Line comment.
        if ch == "/" and text[i + 1 : i + 2] == "/":
            j = i + 2
            while j < n and text[j] != "\n":
                j += 1
            i = j
            continue

        # Block comment (Rust block comments nest).
        if ch == "/" and text[i + 1 : i + 2] == "*":
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                pair = text[j : j + 2]
                if pair == "/*":
                    depth += 1
                    j += 2
                elif pair == "*/":
                    depth -= 1
                    j += 2
                else:
                    if text[j] == "\n":
                        line += 1
                    j += 1
            i = j
            continue

        # Raw string literal, optionally byte-prefixed: r"..", r#"..."#, br#"..."#
        raw_end = _scan_raw_string(text, i)
        if raw_end is not None:
            end, newlines = raw_end
            line += newlines
            i = end
            continue

        # Normal / byte string literal: "..." or b"..."
        str_end = _scan_string(text, i)
        if str_end is not None:
            end, newlines = str_end
            line += newlines
            i = end
            continue

        # Char / byte-char literal, or lifetime.
        lit = _scan_char_or_lifetime(text, i)
        if lit is not None:
            end, newlines = lit
            line += newlines
            i = end
            continue

        # Raw identifier: r#ident (only when not a raw string, handled above).
        if (
            ch == "r"
            and text[i + 1 : i + 2] == "#"
            and i + 2 < n
            and _is_ident_start(text[i + 2])
        ):
            j = i + 2
            while j < n and _is_ident_continue(text[j]):
                j += 1
            tokens.append(Token("IDENT", text[i + 2 : j], line, raw=True))
            i = j
            continue

        # Identifier / keyword.
        if _is_ident_start(ch):
            j = i
            while j < n and _is_ident_continue(text[j]):
                j += 1
            tokens.append(Token("IDENT", text[i:j], line))
            i = j
            continue

        # Path separator.
        if ch == ":" and text[i + 1 : i + 2] == ":":
            tokens.append(Token("PUNCT", "::", line))
            i += 2
            continue

        # Any other single character of punctuation.
        tokens.append(Token("PUNCT", ch, line))
        i += 1

    return tokens


def _scan_raw_string(text: str, i: int) -> tuple[int, int] | None:
    """If a raw string literal starts at `i`, return (end_index, newlines)."""

    n = len(text)
    k = i
    if k < n and text[k] == "b":  # byte raw string prefix
        k += 1
    if k >= n or text[k] != "r":
        return None
    k += 1
    hashes = 0
    while k < n and text[k] == "#":
        hashes += 1
        k += 1
    if k >= n or text[k] != '"':
        return None  # e.g. a raw identifier `r#foo` or a bare `r`/`b` ident
    k += 1
    closer = '"' + "#" * hashes
    newlines = 0
    while k < n:
        if text[k] == '"' and text[k + 1 : k + 1 + hashes] == "#" * hashes:
            return k + 1 + hashes, newlines
        if text[k] == "\n":
            newlines += 1
        k += 1
    return k, newlines  # unterminated; consume to EOF


def _scan_string(text: str, i: int) -> tuple[int, int] | None:
    """If a normal/byte string literal starts at `i`, return (end_index, newlines)."""

    n = len(text)
    k = i
    if k < n and text[k] == "b":  # byte string prefix
        k += 1
    if k >= n or text[k] != '"':
        return None
    k += 1
    newlines = 0
    while k < n:
        c = text[k]
        if c == "\\":
            if text[k + 1 : k + 2] == "\n":
                newlines += 1
            k += 2
            continue
        if c == '"':
            return k + 1, newlines
        if c == "\n":
            newlines += 1
        k += 1
    return k, newlines  # unterminated; consume to EOF


def _scan_char_or_lifetime(text: str, i: int) -> tuple[int, int] | None:
    """If a char/byte-char literal or a lifetime starts at `i`, return
    (end_index, newlines). Lifetimes (`'a`, `'static`, `'_`) carry no `::` path,
    so they are simply consumed and discarded like literals."""

    n = len(text)
    k = i
    if k < n and text[k] == "b" and text[k + 1 : k + 2] == "'":  # byte char b'x'
        k += 1
    if k >= n or text[k] != "'":
        return None
    nxt = text[k + 1 : k + 2]
    after = text[k + 2 : k + 3]
    # Lifetime: `'` then an ident char, NOT immediately closed by `'`, and not an
    # escape. e.g. `'a`, `'static`, `'_` (but `'a'`, `'_'` are char literals).
    if nxt and _is_ident_start(nxt) and after != "'":
        k += 1
        while k < n and _is_ident_continue(text[k]):
            k += 1
        return k, 0
    # Char literal (possibly escaped, possibly multi-char like `'\u{1F600}'`).
    k += 1  # opening quote
    newlines = 0
    while k < n and text[k] != "'":
        if text[k] == "\\":
            if text[k + 1 : k + 2] == "\n":
                newlines += 1
            k += 2
            continue
        if text[k] == "\n":
            newlines += 1
        k += 1
    if k < n:
        k += 1  # closing quote
    return k, newlines


# --------------------------------------------------------------------------- #
# Path resolution
# --------------------------------------------------------------------------- #


def module_parts_for(rel: str) -> list[str]:
    """Module path components (under the crate root) for a `src/...rs` file."""

    stem = rel[len("src/") : -len(".rs")]
    parts = stem.split("/")
    if parts and parts[-1] == "mod":
        parts = parts[:-1]
    return parts


def resolve_to_absolute(segments: list[str], module_parts: list[str]) -> list[str] | None:
    """Resolve a path's segments to absolute crate-rooted components.

    Returns None for external-crate paths (which cannot reach the local strategy
    layer) or unresolvable relative paths.
    """

    if not segments:
        return None
    head = segments[0]
    if head == "crate":
        return segments[1:]
    if head == "self":
        return module_parts + segments[1:]
    if head == "super":
        ups = 0
        while ups < len(segments) and segments[ups] == "super":
            ups += 1
        if ups > len(module_parts):
            return None
        base = module_parts[: len(module_parts) - ups]
        return base + segments[ups:]
    # Bare root (e.g. `std`, an external crate) — not the local strategy layer.
    return None


def resolved_strategy_path(segments: list[str], module_parts: list[str]) -> str | None:
    """If the path `segments` enters the local strategy layer, return its absolute
    `::`-joined path (e.g. ``strategies::registry::FeeProvider``); else None."""

    cleaned = [seg for seg in segments if seg and seg != "self"]
    absolute = resolve_to_absolute(cleaned, module_parts)
    if absolute and absolute[0] == STRATEGY_ROOT:
        return "::".join(absolute)
    return None


def resolves_to_crate_root(segments: list[str], module_parts: list[str]) -> bool:
    """Return True when `segments` names the crate root itself."""

    cleaned = [seg for seg in segments if seg and seg != "self"]
    return resolve_to_absolute(cleaned, module_parts) == []


# --------------------------------------------------------------------------- #
# Detection over the token stream
# --------------------------------------------------------------------------- #


def _skip_attributes(tokens: list[Token], i: int) -> int:
    """Skip any leading `#[ ... ]` (outer) or `#![ ... ]` (inner) attribute groups,
    returning the next index."""

    n = len(tokens)
    while i < n and tokens[i].kind == "PUNCT" and tokens[i].value == "#":
        # An inner attribute is `#` `!` `[ ... ]`; tolerate the optional `!`.
        bracket = i + 1
        if bracket < n and tokens[bracket].kind == "PUNCT" and tokens[bracket].value == "!":
            bracket += 1
        if bracket < n and tokens[bracket].kind == "PUNCT" and tokens[bracket].value == "[":
            depth = 0
            j = bracket
            while j < n:
                tok = tokens[j]
                if tok.kind == "PUNCT" and tok.value == "[":
                    depth += 1
                elif tok.kind == "PUNCT" and tok.value == "]":
                    depth -= 1
                    if depth == 0:
                        j += 1
                        break
                j += 1
            i = j
        else:
            i += 1
    return i


def _parse_use_tree(
    tokens: list[Token], i: int
) -> tuple[list[list[str]], list[list[str]], int]:
    """Parse one use-tree node at `i`, returning
    (leaf segment-lists, aliased segment-lists, next index)."""

    n = len(tokens)
    prefix: list[str] = []
    while i < n:
        i = _skip_attributes(tokens, i)
        if i >= n:
            break
        tok = tokens[i]
        if tok.kind == "PUNCT" and tok.value == "{":
            i += 1
            result: list[list[str]] = []
            aliases: list[list[str]] = []
            while i < n and not (tokens[i].kind == "PUNCT" and tokens[i].value == "}"):
                i = _skip_attributes(tokens, i)
                if i >= n:
                    break
                if tokens[i].kind == "PUNCT" and tokens[i].value == ",":
                    i += 1
                    continue
                if tokens[i].kind == "PUNCT" and tokens[i].value == "}":
                    break
                sub, sub_aliases, i = _parse_use_tree(tokens, i)
                for leaf in sub:
                    result.append(prefix + leaf)
                for alias in sub_aliases:
                    aliases.append(prefix + alias)
                if i < n and tokens[i].kind == "PUNCT" and tokens[i].value == ",":
                    i += 1
            if i < n:
                i += 1  # consume `}`
            return result, aliases, i
        if tok.kind == "IDENT" or (tok.kind == "PUNCT" and tok.value == "*"):
            seg = tok.value
            nxt = tokens[i + 1] if i + 1 < n else None
            if nxt and nxt.kind == "PUNCT" and nxt.value == "::":
                prefix.append(seg)
                i += 2
                continue
            i += 1
            full = prefix + [seg]
            aliases = []
            if (
                i < n
                and tokens[i].kind == "IDENT"
                and tokens[i].value == "as"
                and not tokens[i].raw
            ):
                aliases.append(full)
                i += 2  # skip `as <alias>`
            return [full], aliases, i
        # Unexpected token (e.g. `;`); stop.
        return ([prefix] if prefix else []), [], i
    return ([prefix] if prefix else []), [], i


def _scan_use_statement(
    tokens: list[Token], i: int
) -> tuple[list[list[str]], list[list[str]], int]:
    """Parse `use <tree> ;` starting just after the `use` keyword; return the leaf
    segment-lists, aliased segment-lists, and the index just past the terminating
    `;`."""

    n = len(tokens)
    paths, aliases, i = _parse_use_tree(tokens, i)
    while i < n and not (tokens[i].kind == "PUNCT" and tokens[i].value == ";"):
        i += 1
    if i < n:
        i += 1  # consume `;`
    return paths, aliases, i


def _skip_turbofish(tokens: list[Token], i: int) -> int:
    """Return the index after a `::<...>` generic argument group starting at `<`."""

    if i >= len(tokens) or tokens[i].kind != "PUNCT" or tokens[i].value != "<":
        return i
    depth = 0
    while i < len(tokens):
        tok = tokens[i]
        if tok.kind == "PUNCT" and tok.value == "<":
            depth += 1
        elif tok.kind == "PUNCT" and tok.value == ">":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return i


def _collect_inline_path_segments(tokens: list[Token], i: int) -> list[str]:
    """Collect an inline rooted path, skipping turbofish generic arguments while
    preserving trailing path segments such as `Type::<T>::method`."""

    segs = [tokens[i].value]
    j = i + 2
    while j < len(tokens) and tokens[j].kind == "IDENT":
        segs.append(tokens[j].value)
        j += 1
        if not (
            j < len(tokens)
            and tokens[j].kind == "PUNCT"
            and tokens[j].value == "::"
        ):
            break
        j += 1
        if j < len(tokens) and tokens[j].kind == "PUNCT" and tokens[j].value == "<":
            j = _skip_turbofish(tokens, j)
            if (
                j < len(tokens)
                and tokens[j].kind == "PUNCT"
                and tokens[j].value == "::"
            ):
                j += 1
                continue
            break
    return segs


def detect_strategy_paths(
    tokens: list[Token], module_parts: list[str], rel: str | None = None
) -> list[tuple[str, int]]:
    """Return (absolute strategy path, line) for every strategy reference — both
    `use` imports and inline fully-qualified paths.

    `self`/`super` resolution uses the LEXICAL module stack, not just the file
    path: inline `mod NAME { ... }` blocks deepen the module so that, e.g.,
    `super::super::strategies` from inside `mod tests {}` in a top-level
    `src/bolt_v3_foo.rs` correctly resolves to the crate root and is flagged.
    """

    out: list[tuple[str, int]] = []
    n = len(tokens)
    i = 0
    brace_depth = 0
    mod_stack: list[tuple[str, int]] = []  # (inline module name, body brace depth)
    while i < n:
        tok = tokens[i]

        # Inline module `mod NAME { ... }` (any leading `pub`/attrs are separate,
        # ignorable tokens). `mod NAME ;` is an external file module — not pushed.
        if (
            tok.kind == "IDENT"
            and tok.value == "mod"
            and not tok.raw
            and i + 2 < n
            and tokens[i + 1].kind == "IDENT"
            and tokens[i + 2].kind == "PUNCT"
            and tokens[i + 2].value == "{"
        ):
            brace_depth += 1
            mod_stack.append((tokens[i + 1].value, brace_depth))
            i += 3
            continue

        if tok.kind == "PUNCT" and tok.value == "{":
            brace_depth += 1
            i += 1
            continue
        if tok.kind == "PUNCT" and tok.value == "}":
            brace_depth -= 1
            while mod_stack and mod_stack[-1][1] > brace_depth:
                mod_stack.pop()
            i += 1
            continue

        effective = module_parts + [name for name, _ in mod_stack]

        # `use` statement (the keyword can never be a normal identifier). Its own
        # `{ }` grouped-import braces are consumed inside `_scan_use_statement`, so
        # they never reach the module-brace counter above.
        if tok.kind == "IDENT" and tok.value == "use" and not tok.raw:
            line = tok.line
            paths, aliases, i = _scan_use_statement(tokens, i + 1)
            for segs in aliases:
                if resolves_to_crate_root(segs, effective):
                    location = f"{rel}:{line}" if rel is not None else f"line {line}"
                    raise PolicyError(
                        f"{location}: crate-root alias is forbidden in "
                        "shared/family modules"
                    )
            for segs in paths:
                strategy_path = resolved_strategy_path(segs, effective)
                if strategy_path is not None:
                    out.append((strategy_path, line))
            continue

        # Inline fully-qualified path rooted at crate/self/super.
        if (
            tok.kind == "IDENT"
            and not tok.raw
            and tok.value in ("crate", "self", "super")
        ):
            prev = tokens[i - 1] if i > 0 else None
            nxt = tokens[i + 1] if i + 1 < n else None
            prev_is_sep = prev is not None and prev.kind == "PUNCT" and prev.value == "::"
            nxt_is_sep = nxt is not None and nxt.kind == "PUNCT" and nxt.value == "::"
            if not prev_is_sep and nxt_is_sep:
                segs = _collect_inline_path_segments(tokens, i)
                strategy_path = resolved_strategy_path(segs, effective)
                if strategy_path is not None:
                    out.append((strategy_path, tok.line))
                i += 1
                continue

        i += 1
    return out


def reject_forbidden_source_inclusion(tokens: list[Token], rel: str) -> None:
    for i, tok in enumerate(tokens):
        nxt = tokens[i + 1] if i + 1 < len(tokens) else None
        if tok.kind == "IDENT" and tok.value == "extern" and not tok.raw:
            crate_tok = tokens[i + 1] if i + 1 < len(tokens) else None
            self_tok = tokens[i + 2] if i + 2 < len(tokens) else None
            if (
                crate_tok is not None
                and crate_tok.kind == "IDENT"
                and crate_tok.value == "crate"
                and not crate_tok.raw
                and self_tok is not None
                and self_tok.kind == "IDENT"
                and self_tok.value == "self"
                and not self_tok.raw
            ):
                raise PolicyError(
                    f"{rel}:{tok.line}: extern crate self is forbidden in "
                    "shared/family modules"
                )
        if (
            tok.kind == "IDENT"
            and tok.value == "include"
            and nxt is not None
            and nxt.kind == "PUNCT"
            and nxt.value == "!"
        ):
            raise PolicyError(
                f"{rel}:{tok.line}: include! source inclusion is forbidden "
                "in shared/family modules"
            )
        if tok.kind == "PUNCT" and tok.value == "#":
            bracket = i + 1
            if (
                bracket < len(tokens)
                and tokens[bracket].kind == "PUNCT"
                and tokens[bracket].value == "!"
            ):
                bracket += 1
            if not (
                bracket < len(tokens)
                and tokens[bracket].kind == "PUNCT"
                and tokens[bracket].value == "["
            ):
                continue
            depth = 0
            j = bracket
            while j < len(tokens):
                attr_tok = tokens[j]
                if attr_tok.kind == "PUNCT" and attr_tok.value == "[":
                    depth += 1
                elif attr_tok.kind == "PUNCT" and attr_tok.value == "]":
                    depth -= 1
                    if depth == 0:
                        break
                elif attr_tok.kind == "IDENT" and attr_tok.value == "path":
                    after = tokens[j + 1] if j + 1 < len(tokens) else None
                    if after is not None and after.kind == "PUNCT" and after.value == "=":
                        raise PolicyError(
                            f"{rel}:{tok.line}: #[path] source inclusion is forbidden "
                            "in shared/family modules"
                        )
                j += 1


# --------------------------------------------------------------------------- #
# File walking and policy
# --------------------------------------------------------------------------- #


def scan_files(root: Path) -> tuple[Path, ...]:
    src = root / "src"
    if not src.exists():
        return ()
    for path in sorted(src.rglob("*")):
        rel = path.relative_to(root).as_posix()
        if rel.startswith(SCAN_PREFIX) and path.is_symlink():
            raise PolicyError(
                f"{rel} is a symlink; scanned source paths must be regular files/directories"
            )
    files: list[Path] = []
    for path in sorted(src.rglob("*.rs")):
        rel = path.relative_to(root).as_posix()
        if not rel.startswith(SCAN_PREFIX):
            continue
        if path.is_file():
            files.append(path)
    return tuple(files)


def read_policy_source(path: Path, rel: str) -> str:
    try:
        if path.stat().st_size > MAX_SCAN_FILE_BYTES:
            raise PolicyError(f"{rel} exceeds 1 MiB limit")
        return path.read_text(encoding="utf-8")
    except PolicyError:
        raise
    except OSError as error:
        raise PolicyError(f"failed to read {rel}: {error}") from error
    except UnicodeDecodeError as error:
        raise PolicyError(f"failed to decode {rel}: {error}") from error


def find_violations(root: Path) -> list[Finding]:
    # Key by (file, absolute strategy path); keep the earliest line so output is
    # deterministic and a reference imported AND used inline is reported once.
    earliest: dict[tuple[str, str], int] = {}
    for path in scan_files(root):
        rel = path.relative_to(root).as_posix()
        module_parts = module_parts_for(rel)
        text = read_policy_source(path, rel)
        tokens = tokenize(text)
        reject_forbidden_source_inclusion(tokens, rel)
        for strategy_path, line in detect_strategy_paths(tokens, module_parts, rel=rel):
            key = (rel, strategy_path)
            if key not in earliest or line < earliest[key]:
                earliest[key] = line
    return [
        Finding(path=rel, line=line, strategy_path=strategy_path)
        for (rel, strategy_path), line in sorted(
            earliest.items(), key=lambda item: (item[0][0], item[1], item[0][1])
        )
    ]


def is_allowed(finding: Finding) -> bool:
    return any(
        allowance.path == finding.path
        and allowance.strategy_path == finding.strategy_path
        for allowance in FINDING_ALLOWANCES
    )


# --------------------------------------------------------------------------- #
# Shrink-only enforcement
#
# The in-tree allowlist alone cannot guarantee "may only shrink": a single PR
# could add a new `crate::strategies` reference AND a matching allowance and stay
# green. The only trust anchor that resists that is the protected mainline. So a
# separate `--check-shrink-only-vs-main` mode asserts the current allowlist is a
# SUBSET of the allowlist on `origin/main`. The fence's own source is read from
# the baseline ref and its FINDING_ALLOWANCES parsed via AST (never executed).
# Before the fence exists on main (the PR that introduces it) there is no
# baseline to compare to, so the check is a documented no-op until the first
# merge; after that, every PR is checked.
# --------------------------------------------------------------------------- #

# The baseline is the protected mainline and ONLY the mainline. `FETCH_HEAD`, a
# local `main`, or a stale checkout `origin/main` are deliberately NOT accepted:
# those can point at the branch itself or lag the protected branch. The verifier
# fetches the baseline into an isolated temporary Git database before reading it.
BASELINE_REL = "scripts/verify_bolt_v3_dependency_direction.py"
BASELINE_REMOTE = "origin"
BASELINE_BRANCH = "main"
BASELINE_REF = f"{BASELINE_REMOTE}/{BASELINE_BRANCH}"
BASELINE_TEMP_REF = f"refs/dependency-direction-baseline/{BASELINE_BRANCH}"


def _git(args: list[str], *, cwd: Path, check: bool = False) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
    )
    if check and result.returncode != 0:
        raise PolicyError(f"git {' '.join(args)} failed: {result.stderr}")
    return result


def git_failure_details(result: subprocess.CompletedProcess[str]) -> str:
    details = "\n".join(part.strip() for part in (result.stderr, result.stdout) if part.strip())
    return f"\n{details}" if details else ""


def parse_allowances_from_source(text: str) -> set[tuple[str, str]]:
    """Extract FINDING_ALLOWANCES (path, strategy_path) pairs from fence source via
    AST, WITHOUT executing it. Handles both annotated and plain assignment.

    Only a SINGLE MODULE-LEVEL `FINDING_ALLOWANCES` assignment is accepted. We scan
    `tree.body` (module scope) rather than `ast.walk` (whole tree) on purpose: at
    runtime the imported constant follows Python's last-assignment-wins, so unioning
    every assignment anywhere in the file — a top-level duplicate, or a reassignment
    nested in a function/class — would make the parsed baseline diverge from the
    runtime constant and silently INFLATE the shrink-only baseline (fail-open).
    Zero or more-than-one module-level assignment is therefore an error (fail-closed)."""

    tree = ast.parse(text)
    values: list[ast.expr] = []
    for node in tree.body:  # module scope only — see docstring
        names: list[str] = []
        value = None
        if isinstance(node, ast.Assign):
            names = [t.id for t in node.targets if isinstance(t, ast.Name)]
            value = node.value
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names = [node.target.id]
            value = node.value
        if "FINDING_ALLOWANCES" in names and value is not None:
            values.append(value)

    if not values:
        raise PolicyError("baseline source has no module-level FINDING_ALLOWANCES assignment")
    if len(values) > 1:
        raise PolicyError(
            f"baseline source has {len(values)} module-level FINDING_ALLOWANCES "
            "assignments; expected exactly 1 (ambiguous baseline — fail closed)"
        )

    pairs: set[tuple[str, str]] = set()
    elts = values[0].elts if isinstance(values[0], (ast.Tuple, ast.List)) else []
    for elt in elts:
        if not isinstance(elt, ast.Call):
            continue
        path = strat = None
        positional = [a.value for a in elt.args if isinstance(a, ast.Constant)]
        if len(positional) >= 2:
            path, strat = positional[0], positional[1]
        for kw in elt.keywords:
            if isinstance(kw.value, ast.Constant):
                if kw.arg == "path":
                    path = kw.value.value
                elif kw.arg == "strategy_path":
                    strat = kw.value.value
        if isinstance(path, str) and isinstance(strat, str):
            pairs.add((path, strat))
    return pairs


def _read_baseline_source() -> str | None:
    """Return the fence source from the mainline baseline, or None if it is not yet
    present there. Raises PolicyError if the baseline cannot be resolved."""

    remote = _git(["remote", "get-url", BASELINE_REMOTE], cwd=REPO_ROOT)
    remote_url = remote.stdout.strip()
    if remote.returncode != 0 or not remote_url:
        raise PolicyError(
            f"cannot resolve baseline remote {BASELINE_REMOTE} "
            f"to enforce allowlist shrink-only{git_failure_details(remote)}"
        )
    remote_url = fetchable_remote_url(remote_url, REPO_ROOT)
    with tempfile.TemporaryDirectory(prefix="dependency-direction-baseline-") as tmp:
        git_dir = Path(tmp) / "repo.git"
        _git(["init", "--bare", str(git_dir)], cwd=Path(tmp), check=True)
        if not git_dir.is_dir():
            raise PolicyError(f"Git directory {git_dir} does not exist")
        fetch = _git(
            [
                "fetch",
                "--quiet",
                "--no-tags",
                "--no-write-fetch-head",
                "--refmap=",
                remote_url,
                f"refs/heads/{BASELINE_BRANCH}:{BASELINE_TEMP_REF}",
            ],
            cwd=git_dir,
        )
        if fetch.returncode != 0:
            raise PolicyError(
                f"cannot resolve baseline ref {BASELINE_REF} "
                f"to enforce allowlist shrink-only{git_failure_details(fetch)}"
            )
        rev = _git(
            ["rev-parse", "--verify", "--quiet", f"{BASELINE_TEMP_REF}^{{commit}}"],
            cwd=git_dir,
        )
        if rev.returncode != 0:
            raise PolicyError(
                f"cannot resolve baseline ref {BASELINE_REF} "
                f"to enforce allowlist shrink-only{git_failure_details(rev)}"
            )
        show = _git(["show", f"{BASELINE_TEMP_REF}:{BASELINE_REL}"], cwd=git_dir)
        if show.returncode == 0:
            return show.stdout
    return None  # ref resolves but the fence is not on it yet (introducing PR)


def check_allowlist_shrink_only() -> int:
    current = {(a.path, a.strategy_path) for a in FINDING_ALLOWANCES}
    try:
        baseline_source = _read_baseline_source()
    except PolicyError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1

    if baseline_source is None:
        print(
            "OK: dependency fence not yet on the mainline baseline (introducing "
            "PR); allowlist shrink-only is enforced on every PR after merge."
        )
        return 0

    try:
        baseline = parse_allowances_from_source(baseline_source)
    except (PolicyError, SyntaxError) as error:
        print(f"FAIL: cannot parse mainline baseline allowlist: {error}", file=sys.stderr)
        return 1

    added = sorted(current - baseline)
    if added:
        for path, strat in added:
            print(
                f"FAIL: {path}: allowlist may only shrink; allowance is not present "
                f"on the mainline baseline (a new back-reference must be fixed, not "
                f"allowed): references {strat}",
                file=sys.stderr,
            )
        return 1

    print(
        "OK: dependency allowlist is a subset of the mainline baseline "
        f"({len(current)} current, {len(baseline)} baseline)."
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--check-shrink-only-vs-main" in argv:
        return check_allowlist_shrink_only()

    try:
        findings = find_violations(REPO_ROOT)
    except PolicyError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1

    matched: set[tuple[str, str]] = set()
    real: list[Finding] = []
    for finding in findings:
        if is_allowed(finding):
            matched.add((finding.path, finding.strategy_path))
        else:
            real.append(finding)

    stale = [
        allowance
        for allowance in FINDING_ALLOWANCES
        if (allowance.path, allowance.strategy_path) not in matched
    ]

    failed = False
    for finding in real:
        print(finding.render("FAIL"), file=sys.stderr)
        failed = True
    for allowance in stale:
        print(
            f"FAIL: {allowance.path}: stale allowance no longer matches any reference; "
            f"remove it (allowlist may only shrink): references {allowance.strategy_path}",
            file=sys.stderr,
        )
        failed = True

    if failed:
        return 1

    print(
        "OK: Bolt-v3 dependency-direction verifier passed "
        f"({len(FINDING_ALLOWANCES)} frozen pre-existing back-reference(s))."
    )
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
