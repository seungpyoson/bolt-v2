#!/usr/bin/env python3
"""Shared Rust source scanning helpers for CI verifier scripts."""

from __future__ import annotations


def blank_preserving_newlines(text: str) -> str:
    return "".join("\n" if char == "\n" else " " for char in text)


def raw_string_end(text: str, start: int) -> int | None:
    i = start
    if i < len(text) and text[i] in {"b", "c"}:
        i += 1
    if i >= len(text) or text[i] != "r":
        return None

    i += 1
    hash_start = i
    while i < len(text) and text[i] == "#":
        i += 1
    if i >= len(text) or text[i] != '"':
        return None

    delimiter = '"' + text[hash_start:i]
    end = text.find(delimiter, i + 1)
    if end == -1:
        return len(text)
    return end + len(delimiter)


def quoted_literal_end(text: str, start: int, quote: str) -> int:
    i = start + 1
    while i < len(text):
        char = text[i]
        if char == "\\":
            i += 2
            continue
        if char == quote:
            return i + 1
        if char == "\n" and quote == "'":
            return start + 1
        i += 1
    return len(text)


def char_literal_end(text: str, start: int) -> int | None:
    i = start + 1
    if i >= len(text) or text[i] in {"'", "\n", "\r"}:
        return None

    if text[i] == "\\":
        i += 1
        if i >= len(text):
            return None
        if text.startswith("u{", i):
            end = text.find("}", i + 2)
            if end == -1:
                return None
            i = end + 1
        elif text[i] == "x" and i + 2 < len(text):
            i += 3
        else:
            i += 1
    else:
        i += 1

    if i < len(text) and text[i] == "'":
        return i + 1
    return None


def strip_rust_comments_and_literals(text: str) -> str:
    output: list[str] = []
    i = 0
    while i < len(text):
        raw_end = raw_string_end(text, i)
        if raw_end is not None:
            output.append(blank_preserving_newlines(text[i:raw_end]))
            i = raw_end
            continue

        if text.startswith("//", i):
            end = text.find("\n", i)
            if end == -1:
                end = len(text)
            output.append(blank_preserving_newlines(text[i:end]))
            i = end
            continue

        if text.startswith("/*", i):
            depth = 1
            j = i + 2
            while j < len(text) and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            output.append(blank_preserving_newlines(text[i:j]))
            i = j
            continue

        if text[i] in {"b", "c"} and i + 1 < len(text) and text[i + 1] == '"':
            end = quoted_literal_end(text, i + 1, '"')
            output.append(blank_preserving_newlines(text[i:end]))
            i = end
            continue

        if text[i] == '"':
            end = quoted_literal_end(text, i, '"')
            output.append(blank_preserving_newlines(text[i:end]))
            i = end
            continue

        if text[i] == "'":
            end = char_literal_end(text, i)
            if end is not None:
                output.append(blank_preserving_newlines(text[i:end]))
                i = end
                continue

        output.append(text[i])
        i += 1

    return "".join(output)
