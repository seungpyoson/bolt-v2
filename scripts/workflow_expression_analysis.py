#!/usr/bin/env python3
"""Shared lexical helpers for workflow and shell analysis."""

from __future__ import annotations

import functools


YAML_ANCHOR_PATTERN = r"&[A-Za-z0-9_.-]+"
YAML_KEY_PATTERN = r"""(?:[A-Za-z0-9_.-]+|'[^']*(?:''[^']*)*'|"(?:[^"\\]|\\.)*")"""


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


def unquote_yaml_scalar(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value
