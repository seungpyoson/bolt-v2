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

For every detected path it resolves `crate::`, `self::`, and `super::` roots to
absolute crate-rooted module paths before checking whether the path enters
`crate::strategies`. A bare root (`strategies::...`) is an external crate and is
not flagged; `super::strategies` is flagged only from a top-level `bolt_v3_*`
module (where `super` == the crate root) and correctly ignored from a nested one.

Each violation is keyed by (file, the absolute strategy path it reaches) — NOT by
a surrounding `use` block or a line — so the allowlist is stable against
unrelated edits and a newly-added strategy symbol is caught even inside an
already-coupled `use` block.

Current code already contains pre-existing back-references (tracked under #446 and
the #522 decomposition). They are captured in `FINDING_ALLOWANCES` so the fence is
GREEN on today's code while FAILING on every NEW back-reference. The allowlist may
only SHRINK:

- adding a new allowance is forbidden — a new back-reference is a bug to fix, not
  to allow;
- a stale allowance (one that no longer matches any reference) FAILS, forcing its
  removal once the underlying reference is relocated to a shared module.

KNOWN LIMITATION (documented, not a bug): the fence reads source text, so it
cannot see a path that only exists *after macro expansion* — i.e. a strategy path
synthesized from tokens by a macro at compile time. No source-level scanner can.
The contract requires manual review for macro-generated cross-layer references.
Like the sibling fences, it also does not resolve aliased re-exports that launder
a strategy type through a third module under a new name.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
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
    FindingAllowance("src/bolt_v3_canary_proof_executor.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryBookSideSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryBooksSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryDecisionEvidenceSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryFeeSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryRealizedVolatilitySource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryReferenceQuoteSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleReferenceQuoteObservationSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::derive_entry_reference_proofs_from_quote_observations"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::record_entry_decision_evidence_from_source"),
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


# --------------------------------------------------------------------------- #
# Detection over the token stream
# --------------------------------------------------------------------------- #


def _skip_attributes(tokens: list[Token], i: int) -> int:
    """Skip any leading `#[ ... ]` attribute groups, returning the next index."""

    n = len(tokens)
    while i < n and tokens[i].kind == "PUNCT" and tokens[i].value == "#":
        if i + 1 < n and tokens[i + 1].kind == "PUNCT" and tokens[i + 1].value == "[":
            depth = 0
            j = i + 1
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


def _parse_use_tree(tokens: list[Token], i: int) -> tuple[list[list[str]], int]:
    """Parse one use-tree node at `i`, returning (leaf segment-lists, next index)."""

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
            while i < n and not (tokens[i].kind == "PUNCT" and tokens[i].value == "}"):
                i = _skip_attributes(tokens, i)
                if i >= n:
                    break
                if tokens[i].kind == "PUNCT" and tokens[i].value == ",":
                    i += 1
                    continue
                if tokens[i].kind == "PUNCT" and tokens[i].value == "}":
                    break
                sub, i = _parse_use_tree(tokens, i)
                for leaf in sub:
                    result.append(prefix + leaf)
                if i < n and tokens[i].kind == "PUNCT" and tokens[i].value == ",":
                    i += 1
            if i < n:
                i += 1  # consume `}`
            return result, i
        if tok.kind == "IDENT" or (tok.kind == "PUNCT" and tok.value == "*"):
            seg = tok.value
            nxt = tokens[i + 1] if i + 1 < n else None
            if nxt and nxt.kind == "PUNCT" and nxt.value == "::":
                prefix.append(seg)
                i += 2
                continue
            i += 1
            if i < n and tokens[i].kind == "IDENT" and tokens[i].value == "as":
                i += 2  # skip `as <alias>`
            return [prefix + [seg]], i
        # Unexpected token (e.g. `;`); stop.
        return ([prefix] if prefix else []), i
    return ([prefix] if prefix else []), i


def _scan_use_statement(tokens: list[Token], i: int) -> tuple[list[list[str]], int]:
    """Parse `use <tree> ;` starting just after the `use` keyword; return the leaf
    segment-lists and the index just past the terminating `;`."""

    n = len(tokens)
    paths, i = _parse_use_tree(tokens, i)
    while i < n and not (tokens[i].kind == "PUNCT" and tokens[i].value == ";"):
        i += 1
    if i < n:
        i += 1  # consume `;`
    return paths, i


def detect_strategy_paths(
    tokens: list[Token], module_parts: list[str]
) -> list[tuple[str, int]]:
    """Return (absolute strategy path, line) for every strategy reference — both
    `use` imports and inline fully-qualified paths."""

    out: list[tuple[str, int]] = []
    n = len(tokens)
    i = 0
    while i < n:
        tok = tokens[i]

        # `use` statement (the keyword can never be a normal identifier).
        if tok.kind == "IDENT" and tok.value == "use" and not tok.raw:
            line = tok.line
            paths, i = _scan_use_statement(tokens, i + 1)
            for segs in paths:
                strategy_path = resolved_strategy_path(segs, module_parts)
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
                segs = [tok.value]
                j = i + 2
                while j < n and tokens[j].kind == "IDENT":
                    segs.append(tokens[j].value)
                    j += 1
                    if j < n and tokens[j].kind == "PUNCT" and tokens[j].value == "::":
                        j += 1
                        continue
                    break
                strategy_path = resolved_strategy_path(segs, module_parts)
                if strategy_path is not None:
                    out.append((strategy_path, tok.line))
                i = j
                continue

        i += 1
    return out


# --------------------------------------------------------------------------- #
# File walking and policy
# --------------------------------------------------------------------------- #


def scan_files(root: Path) -> tuple[Path, ...]:
    src = root / "src"
    if not src.exists():
        return ()
    return tuple(
        sorted(
            path
            for path in src.rglob("*.rs")
            if path.is_file()
            and path.relative_to(root).as_posix().startswith(SCAN_PREFIX)
        )
    )


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
        for strategy_path, line in detect_strategy_paths(tokens, module_parts):
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


def main() -> int:
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
    raise SystemExit(main())
