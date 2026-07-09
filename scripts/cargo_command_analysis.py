#!/usr/bin/env python3
"""Cargo and shell command analyzer helpers relocated from verify_ci_workflow_hygiene."""

from __future__ import annotations

import functools
import pathlib
import re
import shlex
import tomllib
from typing import Any

from command_understanding import (
    CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT,
    cargo_args_for_target_routing_scan,
    cargo_subcommand,
    python_inline_command_payloads,
)
from workflow_expression_analysis import strip_comment

CI_SOURCE_BUILD_TOOLS = ("cargo-deny", "cargo-nextest", "cargo-zigbuild")
CI_INSTALL_ACTION_COMMANDS = {
    "deny": "just deny",
    "advisories": "just deny-advisories",
    "test-archive": 'just test-archive "$NEXTEST_ARCHIVE_PATH"',
    "build": "just build",
}
# Static-only option consumption keeps this local constant intentionally; the
# shared scanner has broader Cargo CLI coverage while preserving scan parity.
CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT = {"--frozen", "--locked", "--offline", "--quiet", "-q", "--verbose", "-v"}
SHELL_ASSIGNMENT_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)(?:\+)?=[\s\S]*$")
def shell_assignment_name(token: str) -> str | None:
    match = SHELL_ASSIGNMENT_RE.match(token)
    return match.group(1) if match else None
def shell_assignment_word(token: str) -> bool:
    return shell_assignment_name(token) is not None


def shell_name_word(token: str) -> bool:
    return re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", storage_strip_quotes(token)) is not None


SUDO_OPTIONS_WITH_ARGUMENT = {
    "-a",
    "-C",
    "-c",
    "-D",
    "-g",
    "-h",
    "-p",
    "-R",
    "-r",
    "-T",
    "-t",
    "-U",
    "-u",
    "--auth-type",
    "--chdir",
    "--close-from",
    "--command-timeout",
    "--group",
    "--host",
    "--login-class",
    "--prompt",
    "--role",
    "--type",
    "--user",
}
SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT = {
    "--preserve-env",
}
SUDO_OPTIONS_WITHOUT_ARGUMENT = {
    "-A",
    "-b",
    "-E",
    "-e",
    "-H",
    "-i",
    "-K",
    "-k",
    "-l",
    "-n",
    "-P",
    "-S",
    "-s",
    "-V",
    "-v",
    "--askpass",
    "--background",
    "--bell",
    "--edit",
    "--help",
    "--ignore-ticket",
    "--list",
    "--login",
    "--non-interactive",
    "--remove-timestamp",
    "--reset-timestamp",
    "--stdin",
    "--validate",
    "--version",
}
ENV_OPTIONS_WITH_ARGUMENT = {
    "-a",
    "-S",
    "-u",
    "-C",
    "--argv0",
    "--split-string",
    "--unset",
    "--chdir",
}
ENV_SIGNAL_OPTIONS = {"--block-signal", "--default-signal", "--ignore-signal"}
ENV_OPTIONS_WITHOUT_ARGUMENT = {
    "-0",
    "-i",
    "-v",
    "--debug",
    "--ignore-environment",
    "--null",
}
SU_SG_OPTIONS_WITH_ARGUMENT = {
    "-g",
    "-G",
    "-s",
    "-w",
    "--group",
    "--shell",
    "--supp-group",
    "--whitelist-environment",
}
SU_SG_OPTIONS_WITHOUT_ARGUMENT = {
    "-l",
    "-m",
    "-M",
    "-p",
    "-P",
    "--fast",
    "--login",
    "--preserve-environment",
    "--pty",
}
SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS = {"m", "M", "p", "P", "l"}
FLOCK_OPTIONS_WITH_ARGUMENT = {"-E", "-w", "--conflict-exit-code", "--wait", "--timeout"}
FLOCK_OPTIONS_WITHOUT_ARGUMENT = {
    "-F",
    "-n",
    "-o",
    "-s",
    "-u",
    "-x",
    "--close",
    "--exclusive",
    "--no-fork",
    "--nonblock",
    "--shared",
    "--unlock",
    "--verbose",
}
FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS = {"s", "x", "n", "u", "o", "F"}
TIME_OPTIONS_WITH_ARGUMENT = {"-f", "-o", "--format", "--output"}
TIME_OPTIONS_WITHOUT_ARGUMENT = {"-a", "-p", "-v", "--append", "--portability", "--verbose"}
SHELL_PUNCTUATION_CHARS = ";&|(){}!<>"
SHELL_COMMAND_BOUNDARIES = {";", "&", "&&", "||", "|", "if", "elif", "then", "else", "while", "until", "do", "!", "(", "{", ")", "}"}
SHELL_REDIRECTION_OPERATORS = {">", ">>", "<", "<<", "<>", ">|", ">&", "<&", "&>", "&>>", "<<<"}
SHELL_PUNCTUATION_OPERATORS = {
    "&>>",
    "&&",
    "||",
    ">>",
    "<<",
    "<>",
    ">|",
    ">&",
    "<&",
    "&>",
    "<<<",
}
SHELL_PUNCTUATION_OPERATORS_BY_LENGTH = tuple(sorted(SHELL_PUNCTUATION_OPERATORS, key=len, reverse=True))
RECURSIVE_WRAPPER_EXECUTABLES = {
    "catchsegv",
    "chrt",
    "command",
    "chroot",
    "doas",
    "docker",
    "env",
    "exec",
    "flock",
    "ionice",
    "nice",
    "nohup",
    "podman",
    "runuser",
    "rustup",
    "setsid",
    "sg",
    "stdbuf",
    "su",
    "sudo",
    "taskset",
    "time",
    "timeout",
    "xargs",
}
CARGO_PROCESS_SUBCOMMANDS = {
    "bench",
    "build",
    "check",
    "clean",
    "clippy",
    "doc",
    "fetch",
    "fmt",
    "install",
    "nextest",
    "run",
    "rustc",
    "test",
    "zigbuild",
}
def consume_assignment_words(tokens: list[str], index: int) -> int:
    while index < len(tokens) and shell_assignment_word(tokens[index]):
        index += 1
    return index


def consume_option_prefix(
    tokens: list[str],
    index: int,
    options_with_argument: set[str],
    options_without_argument: set[str],
    options_with_optional_argument: set[str] | None = None,
) -> int | None:
    options_with_optional_argument = options_with_optional_argument or set()
    short_options_with_argument = {option for option in options_with_argument if re.match(r"^-[A-Za-z0-9]$", option)}
    short_options_without_argument = {option for option in options_without_argument if re.match(r"^-[A-Za-z0-9]$", option)}
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in options_with_argument:
            if index + 1 >= len(tokens):
                return None
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in options_with_optional_argument if option.startswith("--")):
            index += 1
            continue
        if token in options_with_optional_argument:
            index += 1
            continue
        if token in options_without_argument:
            index += 1
            continue
        if len(token) > 2 and token.startswith("-") and not token.startswith("--"):
            offset = 1
            while offset < len(token):
                option = f"-{token[offset]}"
                if option in short_options_without_argument:
                    offset += 1
                    continue
                if option in short_options_with_argument:
                    if offset + 1 < len(token):
                        index += 1
                    elif index + 1 < len(tokens):
                        index += 2
                    else:
                        return None
                    break
                return None
            else:
                index += 1
            continue
        break
    return index


def command_prefix_allows_cargo(prefix: list[str]) -> bool:
    prefix = strip_shell_redirections(prefix)
    index = consume_assignment_words(prefix, 0)
    while index < len(prefix):
        token = prefix[index]
        if token == "command":
            index += 1
        elif token == "time":
            index = consume_option_prefix(prefix, index + 1, TIME_OPTIONS_WITH_ARGUMENT, TIME_OPTIONS_WITHOUT_ARGUMENT)
        elif token == "nice":
            index = nice_command_index(prefix, index + 1)
        elif token == "sudo":
            index = consume_option_prefix(
                prefix,
                index + 1,
                SUDO_OPTIONS_WITH_ARGUMENT,
                SUDO_OPTIONS_WITHOUT_ARGUMENT,
                SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
            )
        elif token == "doas":
            index = consume_option_prefix(prefix, index + 1, SUDO_OPTIONS_WITH_ARGUMENT, SUDO_OPTIONS_WITHOUT_ARGUMENT)
        elif token == "env":
            index = env_command_prefix_index(prefix, index + 1)
        elif token == "flock":
            inner = flock_inner_tokens(prefix[index:])
            if inner is not None:
                index = len(prefix) - len(inner)
            else:
                return False
        elif token == "eval":
            index += 1
            if index < len(prefix) and prefix[index] == "--":
                index += 1
        elif token in {"catchsegv", "chrt", "exec", "ionice", "nohup", "setsid", "stdbuf", "taskset", "timeout", "xargs"}:
            inner = wrapper_inner_tokens(prefix[index:])
            if inner is None:
                return False
            index = len(prefix) - len(inner)
        else:
            return False
        if index is None:
            return False
        index = consume_assignment_words(prefix, index)
    return True


def cargo_token_is_command(tokens: list[str], index: int) -> bool:
    cursor = index - 1
    while cursor >= 0 and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        cursor -= 1
    prefix = tokens[cursor + 1 : index]
    return command_prefix_allows_cargo(prefix)


def split_shell_punctuation_tokens(tokens: list[str]) -> list[str]:
    split_tokens: list[str] = []
    for token in tokens:
        if not token or any(char not in SHELL_PUNCTUATION_CHARS for char in token):
            split_tokens.append(token)
            continue
        cursor = 0
        while cursor < len(token):
            operator = next(
                (candidate for candidate in SHELL_PUNCTUATION_OPERATORS_BY_LENGTH if token.startswith(candidate, cursor)),
                None,
            )
            if operator is not None:
                split_tokens.append(operator)
                cursor += len(operator)
                continue
            split_tokens.append(token[cursor])
            cursor += 1
    return split_tokens


def strip_shell_redirections(tokens: list[str]) -> list[str]:
    stripped: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        operator_index = index
        if (
            token.isdigit()
            and index + 1 < len(tokens)
            and tokens[index + 1] in SHELL_REDIRECTION_OPERATORS
        ):
            operator_index = index + 1
        if tokens[operator_index] in SHELL_REDIRECTION_OPERATORS:
            index = operator_index + 1
            if index < len(tokens) and tokens[index] not in SHELL_COMMAND_BOUNDARIES:
                index += 1
            continue
        stripped.append(token)
        index += 1
    return stripped


# Pure shlex parse of a single command string; memoized because verify_text
# re-tokenizes the same strings thousands of times. Cache an immutable tuple and
# copy on return so callers that mutate the list cannot corrupt the cache.
@functools.cache
def _command_tokens_cached(command: str) -> tuple[str, ...]:
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=SHELL_PUNCTUATION_CHARS)
        lexer.whitespace_split = True
        return tuple(split_shell_punctuation_tokens(list(lexer)))
    except ValueError:
        return tuple(command.split())


def command_tokens(command: str) -> list[str]:
    return list(_command_tokens_cached(command))


def command_tokens_with_line_boundaries(command: str) -> list[str]:
    tokens: list[str] = []
    for line in shell_logical_lines(command):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        line_tokens = command_tokens(stripped)
        if not line_tokens:
            continue
        if tokens and tokens[-1] not in {"|", "&&", "||"}:
            tokens.append(";")
        tokens.extend(line_tokens)
    return tokens


def backtick_command_payloads(tokens: list[str]) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        start = token.find("`")
        if start < 0:
            index += 1
            continue
        payload_parts: list[str] = []
        remainder = token[start + 1 :]
        end = remainder.find("`")
        if end >= 0:
            payload = remainder[:end].strip()
            if payload:
                payloads.append(command_tokens(payload))
            index += 1
            continue
        if remainder:
            payload_parts.append(remainder)
        cursor = index + 1
        while cursor < len(tokens):
            part = tokens[cursor]
            end = part.find("`")
            if end >= 0:
                if end:
                    payload_parts.append(part[:end])
                break
            payload_parts.append(part)
            cursor += 1
        if cursor < len(tokens):
            payload = " ".join(payload_parts).strip()
            if payload:
                payloads.append(command_tokens(payload))
            index = cursor + 1
            continue
        index += 1
    return payloads


def inline_command_substitution_payloads(token: str) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 0
    while index + 1 < len(token):
        if token[index : index + 2] not in {"$(", "<("}:
            index += 1
            continue
        cursor = index + 2
        depth = 1
        payload_chars: list[str] = []
        while cursor < len(token) and depth:
            char = token[cursor]
            if char == "(":
                depth += 1
                payload_chars.append(char)
            elif char == ")":
                depth -= 1
                if depth:
                    payload_chars.append(char)
            else:
                payload_chars.append(char)
            cursor += 1
        if depth == 0:
            payload = "".join(payload_chars).strip()
            if payload:
                payloads.append(command_tokens(payload))
            index = cursor
            continue
        index += 1
    return payloads


def shell_command_substitution_payloads(tokens: list[str]) -> list[list[str]]:
    payloads = backtick_command_payloads(tokens)
    for token in tokens:
        payloads.extend(inline_command_substitution_payloads(token))
    index = 0
    while index + 1 < len(tokens):
        token = tokens[index]
        if (token == "$" or token.endswith("$") or token == "<") and tokens[index + 1] == "(":
            cursor = index + 2
            depth = 1
            payload: list[str] = []
            while cursor < len(tokens) and depth:
                current = tokens[cursor]
                if current == "(":
                    depth += 1
                    payload.append(current)
                elif current == ")":
                    depth -= 1
                    if depth:
                        payload.append(current)
                else:
                    payload.append(current)
                cursor += 1
            if depth == 0:
                if payload:
                    payloads.append(payload)
                index = cursor
                continue
        index += 1
    return payloads


def shell_quotes_are_balanced(text: str) -> bool:
    quote: str | None = None
    escaped = False
    for char in text:
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
    return quote is None


def shell_logical_lines(text: str) -> list[str]:
    lines: list[str] = []
    pending = ""
    normalized = text.replace("\\\r\n", " ").replace("\\\n", " ")
    for line in normalized.splitlines():
        pending = f"{pending}\n{line}" if pending else line
        balance_text = "\n".join(strip_comment(pending_line) for pending_line in pending.splitlines())
        if shell_quotes_are_balanced(balance_text):
            lines.append(pending)
            pending = ""
    if pending:
        lines.append(pending)
    return lines


def shell_command(tokens: list[str]) -> str | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if index + 1 < len(tokens):
            if token == "-c":
                return tokens[index + 1]
            if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
                return tokens[index + 1]
        index += 1
    return None


def source_build_tool_from_token(token: str) -> str | None:
    token = token.rstrip("/")
    lower_token = token.lower()
    for tool in CI_SOURCE_BUILD_TOOLS:
        lower_tool = tool.lower()
        if lower_token == lower_tool or lower_token.startswith(f"{lower_tool}@"):
            return tool
        if lower_token.endswith(f"/{lower_tool}") or lower_token.endswith(f"/{lower_tool}.git"):
            return tool
    return None


def normalized_source_path(token: str) -> str:
    return token.rstrip("/")


def source_build_tool_for_path(
    token: str,
    source_path_tools: dict[str, str] | None,
    cwd_source_tool: str | None = None,
) -> str | None:
    normalized = normalized_source_path(token)
    if normalized == "." and cwd_source_tool is not None:
        return cwd_source_tool
    if source_path_tools and normalized in source_path_tools:
        return source_path_tools[normalized]
    return source_build_tool_from_token(token)


def executable_name(token: str) -> str:
    return pathlib.Path(token).name


def cargo_install_source_build_tools(
    tokens: list[str],
    command_index: int,
    source_path_tools: dict[str, str] | None = None,
    cwd_source_tool: str | None = None,
) -> set[str]:
    tools: set[str] = set()
    for payload in shell_command_substitution_payloads(tokens[command_index + 1 :]):
        for token in payload:
            tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
    index = command_index + 1
    while index < len(tokens) and tokens[index] not in SHELL_COMMAND_BOUNDARIES:
        token = tokens[index]
        if token in ("--package", "-p") and index + 1 < len(tokens):
            tool = source_build_tool_for_path(tokens[index + 1], source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 2
            continue
        if token.startswith("--package="):
            tool = source_build_tool_for_path(token.removeprefix("--package="), source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 1
            continue
        if token == "--path" and index + 1 < len(tokens):
            tool = source_build_tool_for_path(tokens[index + 1], source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 2
            continue
        if token.startswith("--path="):
            tool = source_build_tool_for_path(token.removeprefix("--path="), source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 1
            continue
        tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
        if tool is not None:
            tools.add(tool)
        index += 1
    return tools


def source_build_tools_from_depth_exceeded_tokens(
    tokens: list[str],
    source_path_tools: dict[str, str] | None,
    cwd_source_tool: str | None,
) -> set[str]:
    if "install" not in tokens:
        return set()
    tools: set[str] = set()
    for token in tokens:
        tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
        if tool is not None:
            tools.add(tool)
    return tools


def cd_source_tool(tokens: list[str], source_path_tools: dict[str, str] | None) -> tuple[bool, str | None]:
    if not tokens or tokens[0] != "cd":
        return False, None
    index = 1
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return True, None
    return True, source_build_tool_for_path(tokens[index], source_path_tools)


def cargo_install_source_build_tools_from_tokens(
    tokens: list[str],
    *,
    depth: int = 0,
    source_path_tools: dict[str, str] | None = None,
    cwd_source_tool: str | None = None,
) -> set[str]:
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return set()
    if depth > 6:
        return source_build_tools_from_depth_exceeded_tokens(tokens, source_path_tools, cwd_source_tool)
    tools: set[str] = set()
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        segment_cwd_source_tool = cwd_source_tool
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                tools.update(
                    cargo_install_source_build_tools_from_tokens(
                        segment,
                        depth=depth + 1,
                        source_path_tools=source_path_tools,
                        cwd_source_tool=segment_cwd_source_tool,
                    )
                )
                changed, cd_tool = cd_source_tool(segment, source_path_tools)
                if changed:
                    segment_cwd_source_tool = cd_tool
                segment = []
                continue
            segment.append(token)
        tools.update(
            cargo_install_source_build_tools_from_tokens(
                segment,
                depth=depth + 1,
                source_path_tools=source_path_tools,
                cwd_source_tool=segment_cwd_source_tool,
            )
        )
        return tools
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return cargo_install_source_build_tools_from_tokens(
            tokens[assignment_index:],
            depth=depth + 1,
            source_path_tools=source_path_tools,
            cwd_source_tool=cwd_source_tool,
        )
    executable = pathlib.Path(tokens[0]).name
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(tokens)
        if nested is None:
            return tools
        return cargo_install_source_build_tools_from_tokens(
            command_tokens(nested),
            depth=depth + 1,
            source_path_tools=source_path_tools,
            cwd_source_tool=cwd_source_tool,
        )
    if executable.startswith("python"):
        for payload in python_inline_command_payloads(tokens):
            tools.update(
                cargo_install_source_build_tools_from_tokens(
                    command_tokens(payload),
                    depth=depth + 1,
                    source_path_tools=source_path_tools,
                    cwd_source_tool=cwd_source_tool,
                )
            )
        return tools
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return cargo_install_source_build_tools_from_tokens(
                inner,
                depth=depth + 1,
                source_path_tools=source_path_tools,
                cwd_source_tool=cwd_source_tool,
            )
        return tools
    if executable == "cargo":
        command_index = consume_cargo_global_options(tokens, 1)
        if command_index < len(tokens) and tokens[command_index] == "install":
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
    elif path_invocation_may_have_cargo_subcommand(tokens):
        command_index = consume_cargo_global_options(tokens, 1)
        if command_index < len(tokens) and tokens[command_index] == "install":
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
    return tools


def source_build_clone_path_tools(text: str) -> dict[str, str]:
    path_tools: dict[str, str] = {}
    for line in text.replace("\\\n", " ").splitlines():
        tokens = command_tokens(line)
        for index, token in enumerate(tokens[:-2]):
            if executable_name(token) != "git" or tokens[index + 1] != "clone":
                continue
            cursor = index + 2
            while cursor < len(tokens) and tokens[cursor].startswith("-"):
                if cursor + 1 < len(tokens) and not tokens[cursor + 1].startswith("-"):
                    cursor += 2
                else:
                    cursor += 1
            if cursor >= len(tokens):
                continue
            tool = source_build_tool_from_token(tokens[cursor])
            if tool is None:
                continue
            if cursor + 1 < len(tokens) and tokens[cursor + 1] not in SHELL_COMMAND_BOUNDARIES:
                path_tools[normalized_source_path(tokens[cursor + 1])] = tool
    return path_tools


def cargo_install_source_build_tools_in_text(text: str) -> set[str]:
    tools: set[str] = set()
    source_path_tools = source_build_clone_path_tools(text)
    cwd_source_tool: str | None = None
    for line in text.replace("\\\n", " ").splitlines():
        lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        try:
            tokens = list(lexer)
        except ValueError:
            continue
        if "install" in line:
            tools.update(
                cargo_install_source_build_tools_from_tokens(
                    tokens,
                    source_path_tools=source_path_tools,
                    cwd_source_tool=cwd_source_tool,
                )
            )
        for index, token in enumerate(tokens[:-1]):
            if executable_name(token) != "cargo":
                continue
            if not cargo_token_is_command(tokens, index):
                continue
            command_index = consume_cargo_global_options(tokens, index + 1)
            if command_index >= len(tokens) or tokens[command_index] != "install":
                continue
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
        changed, cd_tool = cd_source_tool(tokens, source_path_tools)
        if changed:
            cwd_source_tool = cd_tool
    return tools


def python_rust_verification_script_index(tokens: list[str]) -> int | None:
    if not tokens or not pathlib.Path(tokens[0]).name.startswith("python"):
        return None
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in {"-B", "-E", "-I", "-O", "-OO", "-S", "-s", "-u"}:
            index += 1
            continue
        if token in {"-W", "-X"} and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("-W", "-X")) and token not in {"-W", "-X"}:
            index += 1
            continue
        break
    if index < len(tokens) and pathlib.Path(tokens[index]).name == "rust_verification.py":
        return index
    return None


def managed_rust_verification_command_tokens(tokens: list[str], *, depth: int = 0) -> list[str] | None:
    if depth > 6:
        return None
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return None
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return managed_rust_verification_command_tokens(tokens[assignment_index:], depth=depth + 1)
    executable = pathlib.Path(tokens[0]).name
    if executable == "env":
        inner = env_inner_tokens(tokens)
        return managed_rust_verification_command_tokens(inner, depth=depth + 1) if inner is not None else None
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        return managed_rust_verification_command_tokens(inner, depth=depth + 1) if inner is not None else None
    script_index = python_rust_verification_script_index(tokens)
    if script_index is None or script_index + 1 >= len(tokens):
        return None
    command = tokens[script_index + 1]
    if command not in {"cargo", "run"}:
        return None
    return [tokens[0], tokens[script_index], *tokens[script_index + 1 :]]


def managed_rust_verification_tokens(tokens: list[str]) -> bool:
    return managed_rust_verification_command_tokens(tokens) is not None


def consume_rust_verification_repo_option(tokens: list[str], index: int) -> int:
    if index >= len(tokens):
        return index
    token = tokens[index]
    if token == "--repo" and index + 1 < len(tokens):
        return index + 2
    if token.startswith("--repo="):
        return index + 1
    return index


def managed_rust_verification_cargo_args(tokens: list[str]) -> list[str] | None:
    normalized_tokens = managed_rust_verification_command_tokens(tokens)
    if normalized_tokens is None:
        return None
    command = normalized_tokens[2]
    tail = normalized_tokens[3:]
    index = 0
    while index < len(tail):
        if tail[index] == "--":
            index += 1
            break
        next_index = consume_rust_verification_repo_option(tail, index)
        if next_index == index:
            break
        index = next_index
    if command == "cargo":
        return tail[index:]
    if index >= len(tail):
        return []
    managed_command = tail[index]
    managed_args = tail[index + 1 :]
    return [managed_command, *managed_args]


def target_routing_cargo_args(tokens: list[str]) -> list[str] | None:
    tokens = strip_shell_redirections(tokens)
    managed_args = managed_rust_verification_cargo_args(tokens)
    if managed_args is not None:
        return managed_args
    if not tokens:
        return None
    executable = pathlib.Path(tokens[0]).name
    if executable == "cargo" or path_invocation_may_have_cargo_subcommand(tokens):
        return tokens[1:]
    return None


def cargo_target_routing_scan_tokens(tokens: list[str]) -> list[str]:
    cargo_args = target_routing_cargo_args(tokens)
    if cargo_args is None:
        return []
    return cargo_args_for_target_routing_scan(cargo_args)


def tokens_have_target_routing_override(tokens: list[str]) -> bool:
    env_prefixes = (
        "BOLT_MANAGED_JUST=",
        "CARGO_BUILD_RUSTFLAGS=",
        "CARGO_BUILD_TARGET_DIR=",
        "CARGO_ENCODED_RUSTFLAGS=",
        "CARGO_HOME=",
        "CARGO_INCREMENTAL=",
        "CARGO_INSTALL_ROOT=",
        "CARGO_TARGET_DIR=",
        "CARGO_TARGET_TMPDIR=",
        "RUSTFLAGS=",
        "RUSTUP_HOME=",
    )
    value_options = {"--artifact-dir", "--out-dir", "--root", "--target-dir"}
    for token in tokens:
        if token.startswith(env_prefixes):
            return True
    scan_tokens = cargo_target_routing_scan_tokens(tokens)
    for index, token in enumerate(scan_tokens):
        if token in value_options:
            return True
        if any(token.startswith(f"{option}=") for option in value_options):
            return True
        if token == "--config" and index + 1 < len(scan_tokens) and cargo_config_has_storage_override(scan_tokens[index + 1]):
            return True
        if token.startswith("--config=") and cargo_config_has_storage_override(token.split("=", 1)[1]):
            return True
    return False


def cargo_config_has_storage_override(config: str) -> bool:
    if cargo_config_looks_like_path(config):
        return True
    scan_config = decode_toml_unicode_escapes(config)
    if "target-dir" in scan_config and ("build" in scan_config or "[build]" in scan_config):
        return True
    return "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config)


def decode_toml_unicode_escapes(value: str) -> str:
    def replace(match: re.Match[str]) -> str:
        digits = match.group(1) or match.group(2)
        return chr(int(digits, 16))

    return re.sub(r"\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})", lambda match: replace(match), value)


def cargo_config_looks_like_path(config: str) -> bool:
    stripped = config.strip()
    if not stripped:
        return False
    if stripped.startswith(("[", "{")):
        return False
    if "=" not in stripped:
        return True
    key_prefix = stripped.split("=", 1)[0]
    return "/" in key_prefix or "\\" in key_prefix or key_prefix.endswith(".toml")


def rustup_run_inner_tokens(tokens: list[str]) -> list[str]:
    index = 2
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return []
    index += 1
    while index < len(tokens) and tokens[index] == "--":
        index += 1
    return tokens[index:]


def exec_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return tokens[index + 1 :]
        if token == "-a" and index + 1 < len(tokens):
            index += 2
            continue
        if token in {"-c", "-l"}:
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            if set(cluster) <= {"c", "l"}:
                index += 1
                continue
            if cluster.endswith("a") and set(cluster[:-1]) <= {"c", "l"} and index + 1 < len(tokens):
                index += 2
                continue
        return tokens[index:]
    return []


def container_rust_payload_from_tokens(tokens: list[str], start: int) -> list[str] | None:
    for index in range(start, len(tokens)):
        token = tokens[index]
        executable = pathlib.Path(token).name
        if (
            raw_rust_tool_token(executable)
            or path_executable_looks_like_cargo(token)
            or path_executable_looks_like_rustc(token)
            or path_name_looks_like_renamed_cargo(executable)
            or path_name_looks_like_renamed_rustc(executable)
        ):
            return tokens[index:]
    return None


def container_inner_tokens(tokens: list[str]) -> list[str] | None:
    if len(tokens) < 3:
        return None
    executable = pathlib.Path(tokens[0]).name
    if executable not in {"docker", "podman"}:
        return None
    command = tokens[1]
    options_with_argument = {
        "--add-host",
        "--cpus",
        "--entrypoint",
        "--env",
        "--env-file",
        "--hostname",
        "--mount",
        "--name",
        "--network",
        "--platform",
        "--user",
        "--volume",
        "--workdir",
        "-e",
        "-h",
        "-m",
        "-u",
        "-v",
        "-w",
    }
    index = 2
    entrypoint: str | None = None
    uncertain_options = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token in options_with_argument and index + 1 < len(tokens):
            if token == "--entrypoint":
                entrypoint = tokens[index + 1]
            index += 2
            continue
        if token.startswith("--entrypoint="):
            entrypoint = token.split("=", 1)[1]
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-"):
            uncertain_options = True
            index += 1
            continue
        break
    if command == "run":
        if index >= len(tokens):
            return []
        tail = tokens[index + 1 :]
        if entrypoint is not None:
            return [entrypoint, *tail]
        if uncertain_options:
            fallback = container_rust_payload_from_tokens(tokens, 2)
            if fallback is not None:
                return fallback
        return tail
    if command == "exec":
        if index >= len(tokens):
            return []
        tail = tokens[index + 1 :]
        if uncertain_options:
            fallback = container_rust_payload_from_tokens(tokens, 2)
            if fallback is not None:
                return fallback
        return tail
    return None


def chroot_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token.startswith("--userspec=") or token.startswith("--groups="):
            index += 1
            continue
        if token in {"--userspec", "--groups"} and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    return tokens[index + 1 :] if index < len(tokens) else []


def short_cluster_consumes_option_argument(
    tokens: list[str],
    index: int,
    argument_flags: set[str],
    no_argument_flags: set[str],
) -> int | None:
    token = tokens[index]
    if not token.startswith("-") or token.startswith("--"):
        return None
    offset = 1
    while offset < len(token):
        flag = token[offset]
        if flag in no_argument_flags:
            offset += 1
            continue
        if flag in argument_flags:
            return index + 1 if offset + 1 < len(token) or index + 1 >= len(tokens) else index + 2
        return None
    return index + 1


def su_sg_command_option_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in SU_SG_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in SU_SG_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token in SU_SG_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in {"-c", "--command"} and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            return command_tokens(token[2:])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
            prefix, suffix = token[1:].split("c", 1)
            if set(prefix) <= SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS:
                if suffix:
                    return command_tokens(suffix)
                if index + 1 < len(tokens):
                    return command_tokens(tokens[index + 1])
        next_index = short_cluster_consumes_option_argument(
            tokens,
            index,
            {"g", "G", "s", "w"},
            SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
        )
        if next_index is not None:
            index = next_index
            continue
        index += 1
    return None


def wrapper_inner_tokens(tokens: list[str]) -> list[str] | None:
    executable = pathlib.Path(tokens[0]).name if tokens else ""
    if executable == "command":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token == "-p":
                index += 1
                continue
            if token in ("-v", "-V"):
                return []
            return tokens[index:]
        return []
    if executable in {"sudo", "doas"}:
        index = consume_option_prefix(
            tokens,
            1,
            SUDO_OPTIONS_WITH_ARGUMENT,
            SUDO_OPTIONS_WITHOUT_ARGUMENT,
            SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT if executable == "sudo" else None,
        )
        return tokens[index:] if index is not None else None
    if executable == "flock":
        return flock_inner_tokens(tokens)
    if executable == "timeout":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                continue
            if token in ("-k", "--kill-after", "-s", "--signal") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--kill-after=", "--signal=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index + 1 :]
        return []
    if executable == "stdbuf":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-i", "-o", "-e", "--input", "--output", "--error") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--input=", "--output=", "--error=")):
                index += 1
                continue
            if re.fullmatch(r"-[ioe].+", token):
                index += 1
                continue
            return tokens[index:]
        return []
    if executable == "env":
        return env_inner_tokens(tokens)
    if executable == "nice":
        index = nice_command_index(tokens, 1)
        return tokens[index:] if index is not None else None
    if executable == "rustup" and len(tokens) >= 3 and tokens[1] == "run":
        return rustup_run_inner_tokens(tokens)
    if executable == "exec":
        return exec_inner_tokens(tokens)
    if executable in {"docker", "podman"}:
        return container_inner_tokens(tokens)
    if executable == "chroot":
        return chroot_inner_tokens(tokens)
    if executable in {"catchsegv", "nohup"}:
        return tokens[1:]
    if executable == "time":
        index = consume_option_prefix(tokens, 1, TIME_OPTIONS_WITH_ARGUMENT, TIME_OPTIONS_WITHOUT_ARGUMENT)
        return tokens[index:] if index is not None else None
    if executable == "setsid":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--ctty", "-f", "--fork", "-w", "--wait"):
                index += 1
                continue
            if token.startswith("-") and not token.startswith("--") and set(token[1:]) <= {"c", "f", "w"}:
                index += 1
                continue
            return tokens[index:]
        return []
    if executable == "taskset":
        index = 1
        cpu_list_mode = False
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                continue
            if token in ("-c", "--cpu-list") and index + 1 < len(tokens):
                index += 2
                cpu_list_mode = True
                continue
            if token.startswith("--cpu-list=") or re.fullmatch(r"-c.+", token):
                index += 1
                cpu_list_mode = True
                continue
            if token in ("-a", "--all-tasks"):
                index += 1
                continue
            if token in ("-p", "--pid"):
                return []
            if token.startswith("-"):
                index += 1
                continue
            if not cpu_list_mode:
                index += 1
            return tokens[index:]
        return []
    if executable == "ionice":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--class", "-n", "--classdata") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--class=", "--classdata=")) or re.fullmatch(r"-[cn].+", token):
                index += 1
                continue
            if token in ("-p", "--pid"):
                return []
            if token in ("-t", "--ignore"):
                index += 1
                continue
            if token.startswith("-") and not token.startswith("--"):
                cluster = token[1:]
                if cluster and (set(cluster) <= {"t"} or re.fullmatch(r"t*[cn].+", cluster)):
                    index += 1
                    continue
            return tokens[index:]
        return []
    if executable == "chrt":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                break
            if token in ("-p", "--pid"):
                return []
            if token in ("-T", "--sched-runtime", "-P", "--sched-period", "-D", "--sched-deadline") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--sched-runtime=", "--sched-period=", "--sched-deadline=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            break
        if index < len(tokens) and re.fullmatch(r"-?\d+", tokens[index]):
            index += 1
        return tokens[index:]
    if executable == "xargs":
        options_with_argument = {
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "-I",
            "-L",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
        }
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in options_with_argument and index + 1 < len(tokens):
                index += 2
                continue
            if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
                index += 1
                continue
            if re.fullmatch(r"-(?:a|d|E|I|L|n|P|s).+", token):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index:]
        return []
    if executable in {"su", "sg"}:
        return su_sg_command_option_tokens(tokens)
    if executable == "runuser":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in {"-u", "--user", "-g", "--group", "-G", "--supp-group", "-s", "--shell"} and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--user=", "--group=", "--supp-group=", "--shell=")):
                index += 1
                continue
            if token in {"-c", "--command"} and index + 1 < len(tokens):
                return command_tokens(tokens[index + 1])
            if token.startswith("--command="):
                return command_tokens(token.split("=", 1)[1])
            if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
                return command_tokens(token[2:])
            if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
                prefix, suffix = token[1:].split("c", 1)
                if set(prefix) <= {"m", "M", "p", "P", "l"}:
                    if suffix:
                        return command_tokens(suffix)
                    if index + 1 < len(tokens):
                        return command_tokens(tokens[index + 1])
            next_index = short_cluster_consumes_option_argument(
                tokens,
                index,
                {"G", "g", "s", "u"},
                SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
            )
            if next_index is not None:
                index = next_index
                continue
            if token.startswith("-"):
                index += 1
                continue
            command_index = index + 1
            while command_index < len(tokens):
                candidate = tokens[command_index]
                if candidate in {"-u", "--user", "-g", "--group", "-G", "--supp-group", "-s", "--shell"} and command_index + 1 < len(tokens):
                    command_index += 2
                    continue
                if candidate.startswith(("--user=", "--group=", "--supp-group=", "--shell=")):
                    command_index += 1
                    continue
                if candidate in {"-c", "--command"} and command_index + 1 < len(tokens):
                    return command_tokens(tokens[command_index + 1])
                if candidate.startswith("--command="):
                    return command_tokens(candidate.split("=", 1)[1])
                if candidate.startswith("-c") and not candidate.startswith("--") and len(candidate) > 2:
                    return command_tokens(candidate[2:])
                next_command_index = short_cluster_consumes_option_argument(
                    tokens,
                    command_index,
                    {"G", "g", "s", "u"},
                    SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
                )
                if next_command_index is not None:
                    command_index = next_command_index
                    continue
                command_index += 1
            return tokens[index:]
        return None
    starters = {
        "bash",
        "catchsegv",
        "cargo",
        "cargo-clippy",
        "cargo-fmt",
        "cargo-nextest",
        "env",
        "flock",
        "nice",
        "python",
        "python3",
        "rustup",
        "sh",
        "stdbuf",
        "time",
        "zsh",
    }
    for index, token in enumerate(tokens[1:], start=1):
        if pathlib.Path(token).name in starters:
            return tokens[index:]
    return None


def find_exec_payloads(tokens: list[str]) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 1
    while index < len(tokens):
        if tokens[index] not in {"-exec", "-execdir"}:
            index += 1
            continue
        index += 1
        payload: list[str] = []
        while index < len(tokens) and tokens[index] not in {";", "+"}:
            payload.append(tokens[index])
            index += 1
        if payload:
            payloads.append(payload)
    return payloads


def shell_command_substitution_at(tokens: list[str], index: int) -> tuple[list[str], int] | None:
    if index + 1 >= len(tokens) or not (tokens[index] == "$" or tokens[index].endswith("$")) or tokens[index + 1] != "(":
        return None
    cursor = index + 2
    depth = 1
    payload: list[str] = []
    while cursor < len(tokens) and depth:
        token = tokens[cursor]
        if token == "(":
            depth += 1
            payload.append(token)
        elif token == ")":
            depth -= 1
            if depth:
                payload.append(token)
        else:
            payload.append(token)
        cursor += 1
    return (payload, cursor) if depth == 0 else None


def env_short_cluster_next_index(tokens: list[str], index: int, cluster: str) -> int | None:
    offset = 0
    while offset < len(cluster):
        option = cluster[offset]
        if option in "i0v":
            offset += 1
            continue
        if option in "uC":
            if offset + 1 < len(cluster):
                return index + 1
            if index + 1 < len(tokens):
                return index + 2
            return index + 1
        return None
    return index + 1


def env_short_split_tokens(tokens: list[str], index: int) -> list[str] | None:
    token = tokens[index]
    if not token.startswith("-") or token.startswith("--"):
        return None
    cluster = token[1:]
    if "S" not in cluster:
        return None
    suffix = cluster.split("S", 1)[1]
    if suffix:
        return command_tokens(" ".join([suffix, *tokens[index + 1 :]]))
    if index + 1 < len(tokens):
        return command_tokens(tokens[index + 1]) + tokens[index + 2 :]
    return []


def env_assignment_argument(token: str) -> bool:
    return "=" in token and not token.startswith("-")


def env_command_prefix_index(tokens: list[str], index: int) -> int | None:
    while index < len(tokens):
        token = tokens[index]
        redirection_index = shell_redirection_next_index(tokens, index)
        if redirection_index is not None:
            index = redirection_index
            continue
        if token == "--":
            return index + 1
        if token in ENV_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in ENV_SIGNAL_OPTIONS:
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
            index += 1
            continue
        if token in ENV_OPTIONS_WITH_ARGUMENT:
            if index + 1 >= len(tokens):
                return None
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            if "S" in token[1:]:
                return index
            parsed_index = env_short_cluster_next_index(tokens, index, token[1:])
            if parsed_index is not None:
                index = parsed_index
                continue
        if env_assignment_argument(token):
            index += 1
            continue
        return index
    return index


def shell_redirection_next_index(tokens: list[str], index: int) -> int | None:
    token = tokens[index]
    if token in SHELL_REDIRECTION_OPERATORS:
        return min(index + 2, len(tokens))
    if re.match(r"^(?:\d?(?:>>?|<<?|<>|>\||>&|<&)|&>>?|<<<).+", token):
        return index + 1
    return None


def env_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        redirection_index = shell_redirection_next_index(tokens, index)
        if redirection_index is not None:
            index = redirection_index
            continue
        if token == "--":
            return tokens[index + 1 :]
        if token in ENV_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in ENV_SIGNAL_OPTIONS:
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
            index += 1
            continue
        if token in ("-S", "--split-string") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1]) + tokens[index + 2 :]
        if token.startswith("--split-string="):
            return command_tokens(token.split("=", 1)[1]) + tokens[index + 1 :]
        if token in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            split_tokens = env_short_split_tokens(tokens, index)
            if split_tokens is not None:
                return split_tokens
            parsed_index = env_short_cluster_next_index(tokens, index, token[1:])
            if parsed_index is not None:
                index = parsed_index
                continue
        if env_assignment_argument(token):
            index += 1
            continue
        return tokens[index:]
    return []


def nice_command_index(tokens: list[str], index: int) -> int | None:
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token == "-n" and index + 1 < len(tokens):
            index += 2
            continue
        if token == "--adjustment" and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("--adjustment="):
            index += 1
            continue
        if re.fullmatch(r"-n-?\d+", token) or re.fullmatch(r"-?\d+", token):
            index += 1
            continue
        return index
    return index


def flock_inner_tokens(tokens: list[str]) -> list[str] | None:
    command_option_tokens = flock_command_option_tokens(tokens)
    if command_option_tokens is not None:
        return command_option_tokens
    index = 1
    separator_seen = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            separator_seen = True
            break
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            return command_tokens(token[2:])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
            prefix, suffix = token[1:].split("c", 1)
            if set(prefix) <= FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS:
                if suffix:
                    return command_tokens(suffix)
                if index + 1 < len(tokens):
                    return command_tokens(tokens[index + 1])
        if token in FLOCK_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--conflict-exit-code=", "--wait=", "--timeout=")):
            index += 1
            continue
        if token in FLOCK_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        next_index = short_cluster_consumes_option_argument(
            tokens,
            index,
            {"E", "w"},
            FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS,
        )
        if next_index is not None:
            index = next_index
            continue
        if token.startswith("-"):
            index += 1
            continue
        return tokens[index + 1 :]
    if separator_seen and index < len(tokens):
        return tokens[index + 1 :]
    return tokens[index:]


def flock_command_option_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return None
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            return command_tokens(token[2:])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
            prefix, suffix = token[1:].split("c", 1)
            if set(prefix) <= FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS:
                if suffix:
                    return command_tokens(suffix)
                if index + 1 < len(tokens):
                    return command_tokens(tokens[index + 1])
        if token in FLOCK_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--conflict-exit-code=", "--wait=", "--timeout=")):
            index += 1
            continue
        if token in FLOCK_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        next_index = short_cluster_consumes_option_argument(
            tokens,
            index,
            {"E", "w"},
            FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS,
        )
        if next_index is not None:
            index = next_index
            continue
        index += 1
    return None


def simple_cargo_aliases(tokens: list[str], known_aliases: set[str] | None = None) -> set[str]:
    known_aliases = known_aliases or set()
    aliases: set[str] = set()
    for name, value in shell_alias_payloads(tokens).items():
        value_tokens = command_tokens(value)
        value_names = {pathlib.Path(value_token).name for value_token in value_tokens}
        if any(raw_rust_tool_token(value_name) or value_name in known_aliases for value_name in value_names):
            aliases.add(name)
    return aliases


def shell_alias_payloads(tokens: list[str]) -> dict[str, str]:
    if not tokens or pathlib.Path(tokens[0]).name != "alias":
        return {}
    payloads: dict[str, str] = {}
    for token in tokens[1:]:
        name, separator, value = token.partition("=")
        name = name.strip("\"'")
        if separator and re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name):
            payloads[name] = value.strip()
    return payloads


def expand_cargo_aliases(tokens: list[str], aliases: set[str]) -> list[str]:
    if not aliases:
        return tokens
    return ["cargo" if token in aliases else token for token in tokens]


def no_mistakes_inner_tokens(tokens: list[str]) -> list[str] | None:
    for index, token in enumerate(tokens):
        if token == "--":
            return tokens[index + 1 :]
    return None


def rust_tool_name_has_script_extension(name: str) -> bool:
    return pathlib.Path(name).suffix.lower() in {".bash", ".fish", ".ksh", ".ps1", ".py", ".rb", ".sh", ".zsh"}


def raw_rust_tool_token(name: str) -> bool:
    if rust_tool_name_has_script_extension(name):
        return False
    return name in {"cargo", "clippy", "nextest", "rustc", "rustdoc"} or name.startswith(
        ("cargo-", "clippy-", "rust-")
    )


def path_name_looks_like_renamed_cargo(name: str) -> bool:
    return name == "c" or raw_rust_tool_token(name) or (name.endswith("cargo") and "_" not in name)


def path_executable_looks_like_cargo(token: str) -> bool:
    if "/" not in token:
        return False
    path = pathlib.Path(token)
    if path_name_looks_like_renamed_cargo(path.name):
        return True
    return False


def path_name_looks_like_renamed_rustc(name: str) -> bool:
    return name == "r" or name == "rustc" or (name.endswith("rustc") and "_" not in name)


def path_executable_looks_like_rustc(token: str) -> bool:
    if "/" not in token:
        return False
    path = pathlib.Path(token)
    if path_name_looks_like_renamed_rustc(path.name):
        return True
    return False


def path_invocation_has_cargo_subcommand(tokens: list[str]) -> bool:
    if not tokens:
        return False
    executable = pathlib.Path(tokens[0]).name
    if "/" in tokens[0]:
        if not path_executable_looks_like_cargo(tokens[0]):
            return False
    elif not path_name_looks_like_renamed_cargo(executable):
        return False
    command_index = consume_cargo_global_options(tokens, 1)
    return command_index < len(tokens) and tokens[command_index] in CARGO_PROCESS_SUBCOMMANDS


def path_invocation_may_have_cargo_subcommand(tokens: list[str]) -> bool:
    if not tokens:
        return False
    executable = pathlib.Path(tokens[0]).name
    if "/" not in tokens[0] and not path_name_looks_like_renamed_cargo(executable):
        return False
    command_index = consume_cargo_global_options(tokens, 1)
    return command_index < len(tokens) and tokens[command_index] in CARGO_PROCESS_SUBCOMMANDS


def shell_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    assignments: dict[str, str] = {}
    cursor = 0
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def export_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    if not tokens or pathlib.Path(tokens[0]).name != "export":
        return {}, 0
    assignments: dict[str, str] = {}
    cursor = 1
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "--" or token.startswith("-"):
            cursor += 1
            continue
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            if shell_name_word(token):
                cursor += 1
                continue
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def shell_declaration_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    if not tokens or pathlib.Path(tokens[0]).name not in {"declare", "local", "typeset"}:
        return {}, 0
    assignments: dict[str, str] = {}
    cursor = 1
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "--":
            cursor += 1
            continue
        if token.startswith("-") or token.startswith("+"):
            cursor += 1
            continue
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            if shell_name_word(token):
                cursor += 1
                continue
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def persistent_shell_assignment_values(tokens: list[str]) -> tuple[dict[str, str], bool]:
    assignments, assignment_index = shell_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = export_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = shell_declaration_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = shell_array_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    return {}, False


def shell_array_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    assignments: dict[str, str] = {}
    cursor = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if not shell_assignment_word(token) or not token.endswith("="):
            break
        name, value = token.split("=", 1)
        if value:
            break
        cursor += 1
        if cursor >= len(tokens) or tokens[cursor] != "(":
            break
        cursor += 1
        depth = 1
        parts: list[str] = []
        while cursor < len(tokens) and depth:
            current = tokens[cursor]
            if current == "(":
                depth += 1
                parts.append(current)
            elif current == ")":
                depth -= 1
                if depth:
                    parts.append(current)
            else:
                parts.append(current)
            cursor += 1
        if depth:
            break
        assignments[name] = " ".join(parts)
    return assignments, cursor


def shell_variable_reference_token(token: str) -> str | None:
    clean = storage_strip_quotes(token)
    match = re.fullmatch(r"\$([A-Za-z_][A-Za-z0-9_]*)", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\[(?:@|\*)\]\}", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=].*)\}", clean)
    if match:
        return match.group(1)
    return None


def expand_known_shell_variables(tokens: list[str], variables: dict[str, str]) -> list[str]:
    expanded: list[str] = []
    for token in tokens:
        variable = shell_variable_reference_token(token)
        if variable is not None and variable in variables:
            expanded.extend(command_tokens(variables[variable]))
        else:
            expanded.append(token)
    return expanded


def shell_identifier_fragment(value: str) -> str | None:
    clean = storage_strip_quotes(value)
    return clean if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", clean) else None


def expand_known_shell_assignment_name(name: str, variables: dict[str, str]) -> str:
    def replace_reference(match: re.Match[str]) -> str:
        variable = match.group("bare") or match.group("braced")
        if variable is None or variable not in variables:
            return match.group(0)
        fragment = shell_identifier_fragment(variables[variable])
        return fragment if fragment is not None else match.group(0)

    return re.sub(
        r"\$(?P<bare>[A-Za-z_][A-Za-z0-9_]*)|\$\{(?P<braced>[A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=][^}]*)?\}",
        replace_reference,
        name,
    )


def expand_known_shell_assignment_value(value: str, variables: dict[str, str]) -> str:
    clean = storage_strip_quotes(value)

    def replace_reference(match: re.Match[str]) -> str:
        variable = match.group("bare") or match.group("braced")
        if variable is None or variable not in variables:
            return match.group(0)
        return variables[variable]

    return re.sub(
        r"\$(?P<bare>[A-Za-z_][A-Za-z0-9_]*)|\$\{(?P<braced>[A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=][^}]*)?\}",
        replace_reference,
        clean,
    )


def merge_split_shell_parameter_assignment_tokens(tokens: list[str]) -> list[str]:
    merged: list[str] = []
    index = 0
    while index < len(tokens):
        if tokens[index] == "$" and index + 3 < len(tokens) and tokens[index + 1] == "{":
            close = index + 2
            while close < len(tokens) and tokens[close] != "}":
                close += 1
            if close + 1 < len(tokens) and "=" in tokens[close + 1]:
                variable = "".join(tokens[index + 2 : close])
                merged.append("${" + variable + "}" + tokens[close + 1])
                index = close + 2
                continue
        merged.append(tokens[index])
        index += 1
    return merged


def expand_known_shell_assignment_names(tokens: list[str], variables: dict[str, str]) -> list[str]:
    expanded: list[str] = []
    for token in merge_split_shell_parameter_assignment_tokens(tokens):
        if "=" not in token:
            expanded.append(token)
            continue
        name, value = token.split("=", 1)
        expanded_name = expand_known_shell_assignment_name(name, variables)
        if expanded_name != name and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", expanded_name):
            expanded.append(f"{expanded_name}={value}")
            continue
        expanded.append(token)
    return expanded


def expand_known_shell_command_variables(tokens: list[str], variables: dict[str, str]) -> list[str]:
    if not tokens:
        return tokens
    executable = pathlib.Path(tokens[0]).name
    if executable == "eval":
        return [tokens[0], *expand_known_shell_variables(tokens[1:], variables)]
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        expanded = list(tokens)
        index = 1
        while index + 1 < len(expanded):
            token = expanded[index]
            if token == "-c" or (token.startswith("-") and not token.startswith("--") and "c" in token[1:]):
                variable = shell_variable_reference_token(expanded[index + 1])
                if variable is not None and variable in variables:
                    expanded[index + 1] = variables[variable]
                return expanded
            index += 1
        return expanded
    variable = shell_variable_reference_token(tokens[0])
    if variable is not None and variable in variables:
        return [*command_tokens(variables[variable]), *tokens[1:]]
    return tokens


def tokens_have_raw_cargo(
    tokens: list[str],
    *,
    depth: int = 0,
    allow_storage_only: bool = True,
    variables: dict[str, str] | None = None,
) -> bool:
    if not tokens:
        return False
    variables = variables or {}
    if variables:
        tokens = merge_split_shell_parameter_assignment_tokens(tokens)
        tokens = expand_known_shell_assignment_names(tokens, variables)
        tokens = expand_known_shell_command_variables(tokens, variables)
        if not tokens:
            return False
    if depth > 6:
        return True
    if allow_storage_only and tokens_have_target_routing_override(tokens):
        return True
    for payload in shell_command_substitution_payloads(tokens):
        if tokens_have_raw_cargo(
            payload,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        ):
            return True
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return False
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        cargo_aliases: set[str] = set()
        cargo_alias_payloads: dict[str, str] = {}
        shell_variables: dict[str, str] = dict(variables)
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    alias_payloads = shell_alias_payloads(segment)
                    for payload in alias_payloads.values():
                        if tokens_have_raw_cargo(
                            command_tokens(payload),
                            depth=depth + 1,
                            allow_storage_only=allow_storage_only,
                            variables=shell_variables,
                        ):
                            return True
                    cargo_alias_payloads.update(alias_payloads)
                    cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
                    segment = []
                    continue
                shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    shell_variables.update(shell_assignments)
                    segment = []
                    continue
                segment = expand_known_shell_assignment_names(segment, shell_variables)
                segment = expand_known_shell_command_variables(segment, shell_variables)
                if segment and segment[0] in cargo_alias_payloads:
                    alias_tokens = command_tokens(cargo_alias_payloads[segment[0]]) + segment[1:]
                    if tokens_have_raw_cargo(
                        alias_tokens,
                        depth=depth + 1,
                        allow_storage_only=allow_storage_only,
                        variables=shell_variables,
                    ):
                        return True
                segment = expand_cargo_aliases(segment, cargo_aliases)
                if segment and tokens_have_raw_cargo(
                    segment,
                    depth=depth + 1,
                    allow_storage_only=allow_storage_only,
                    variables=shell_variables,
                ):
                    return True
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            return any(
                tokens_have_raw_cargo(
                    command_tokens(payload),
                    depth=depth + 1,
                    allow_storage_only=allow_storage_only,
                    variables=shell_variables,
                )
                for payload in shell_alias_payloads(segment).values()
            )
        shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
        if is_persistent_assignment:
            return False
        segment = expand_known_shell_assignment_names(segment, shell_variables)
        segment = expand_known_shell_command_variables(segment, shell_variables)
        if segment and segment[0] in cargo_alias_payloads:
            alias_tokens = command_tokens(cargo_alias_payloads[segment[0]]) + segment[1:]
            return tokens_have_raw_cargo(
                alias_tokens,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=shell_variables,
            )
        segment = expand_cargo_aliases(segment, cargo_aliases)
        return bool(segment) and tokens_have_raw_cargo(
            segment,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=shell_variables,
        )
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        prefix_assignments, _assignment_cursor = shell_assignment_values_from_tokens(tokens[:assignment_index])
        local_variables = {**variables, **prefix_assignments}
        return assignment_index < len(tokens) and tokens_have_raw_cargo(
            tokens[assignment_index:],
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=local_variables,
        )
    if managed_rust_verification_tokens(tokens):
        return tokens_have_target_routing_override(tokens)
    executable = pathlib.Path(tokens[0]).name
    if path_invocation_has_cargo_subcommand(tokens):
        return True
    if path_executable_looks_like_rustc(tokens[0]) and any(
        token in {"--crate-name", "--emit", "--out-dir", "--artifact-dir"}
        or token.startswith(("--emit=", "--out-dir=", "--artifact-dir="))
        for token in tokens[1:]
    ):
        return True
    if path_name_looks_like_renamed_rustc(executable) and any(
        token in {"--crate-name", "--emit", "--out-dir", "--artifact-dir"}
        or token.startswith(("--emit=", "--out-dir=", "--artifact-dir="))
        for token in tokens[1:]
    ):
        return True
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(tokens)
        return nested is not None and tokens_have_raw_cargo(
            command_tokens(nested),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "eval":
        inner = tokens[1:]
        if inner and inner[0] == "--":
            inner = inner[1:]
        return bool(inner) and tokens_have_raw_cargo(
            command_tokens(" ".join(inner)),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "no-mistakes":
        inner = no_mistakes_inner_tokens(tokens)
        if inner is None:
            return False
        if inner and raw_rust_tool_token(pathlib.Path(inner[0]).name):
            return True
        return tokens_have_raw_cargo(
            inner,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "env":
        inner = env_inner_tokens(tokens)
        return inner is not None and tokens_have_raw_cargo(
            inner,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "rustup" and len(tokens) >= 3 and tokens[1] == "run":
        return tokens_have_raw_cargo(
            rustup_run_inner_tokens(tokens),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable.startswith("python"):
        return any(
            tokens_have_raw_cargo(
                command_tokens(payload),
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
            for payload in python_inline_command_payloads(tokens)
        )
    if executable == "flock":
        inner = flock_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_raw_cargo(
                inner,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
    if executable == "find":
        return any(
            tokens_have_raw_cargo(
                payload,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
            for payload in find_exec_payloads(tokens)
        )
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_raw_cargo(
                inner,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
    for index, token in enumerate(tokens):
        name = pathlib.Path(token).name
        if name == "cargo" and cargo_token_is_command(tokens, index):
            return True
        if name in {"clippy", "nextest", "rustc", "rustdoc"} and command_prefix_allows_cargo(tokens[:index]):
            return True
        if name != "cargo" and raw_rust_tool_token(name) and command_prefix_allows_cargo(tokens[:index]):
            return True
    return False


def command_has_raw_cargo(command: str) -> bool:
    return tokens_have_raw_cargo(command_tokens(command))


def tokens_have_raw_cargo_launch(tokens: list[str], *, variables: dict[str, str] | None = None) -> bool:
    return tokens_have_raw_cargo(tokens, allow_storage_only=False, variables=variables)
def cargo_config_storage_override_message(tokens: list[str]) -> str | None:
    for index, token in enumerate(tokens):
        if token == "--config" and index + 1 < len(tokens) and cargo_config_has_storage_override(tokens[index + 1]):
            if cargo_config_looks_like_path(tokens[index + 1]):
                return "cargo --config file raw target override must be classified"
            scan_config = decode_toml_unicode_escapes(tokens[index + 1])
            if "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config):
                return "cargo --config build.rustflags raw output override must be classified"
            return "cargo --config build.target-dir raw target override must be classified"
        if token.startswith("--config="):
            config = token.split("=", 1)[1]
            if cargo_config_has_storage_override(config):
                if cargo_config_looks_like_path(config):
                    return "cargo --config file raw target override must be classified"
                scan_config = decode_toml_unicode_escapes(config)
                if "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config):
                    return "cargo --config build.rustflags raw output override must be classified"
                return "cargo --config build.target-dir raw target override must be classified"
    return None


def direct_raw_cargo_storage_override_messages(tokens: list[str]) -> set[str]:
    messages: set[str] = set()
    cargo_args = target_routing_cargo_args(tokens)
    cargo_scan_tokens = cargo_args_for_target_routing_scan(cargo_args) if cargo_args is not None else []
    cargo_command = cargo_subcommand(cargo_args) if cargo_args is not None else None
    if any(token == "--target-dir" or token.startswith("--target-dir=") for token in cargo_scan_tokens):
        messages.add("cargo --target-dir raw target override must be classified")
    if cargo_command == "rustc":
        if any(token == "--out-dir" or token.startswith("--out-dir=") for token in cargo_scan_tokens):
            messages.add("cargo rustc --out-dir raw output override must be classified")
        if any(token == "--artifact-dir" or token.startswith("--artifact-dir=") for token in cargo_scan_tokens):
            messages.add("cargo rustc --artifact-dir raw output override must be classified")
    if tokens and (
        pathlib.Path(tokens[0]).name == "rustc"
        or path_executable_looks_like_rustc(tokens[0])
        or path_name_looks_like_renamed_rustc(pathlib.Path(tokens[0]).name)
    ):
        if any(token == "--out-dir" or token.startswith("--out-dir=") for token in tokens):
            messages.add("rustc --out-dir raw output override must be classified")
        if any(token == "--artifact-dir" or token.startswith("--artifact-dir=") for token in tokens):
            messages.add("rustc --artifact-dir raw output override must be classified")
    config_message = cargo_config_storage_override_message(cargo_scan_tokens)
    if config_message is not None:
        messages.add(config_message)
    if cargo_command == "install":
        has_target_dir = any(token == "--target-dir" or token.startswith("--target-dir=") for token in cargo_scan_tokens)
        has_root = any(token == "--root" or token.startswith("--root=") for token in cargo_scan_tokens)
        if has_target_dir and has_root:
            messages.add("cargo install build target and install root ownership must be classified separately")
        if any(
            token == "--root"
            and index + 1 < len(cargo_scan_tokens)
            and cargo_scan_tokens[index + 1].startswith("s3://")
            for index, token in enumerate(cargo_scan_tokens)
        ):
            messages.add("cargo install S3 install root must be classified")
        if any(token.startswith("--root=s3://") for token in cargo_scan_tokens):
            messages.add("cargo install S3 install root must be classified")
    return messages


def raw_cargo_storage_override_messages_from_tokens(
    tokens: list[str],
    *,
    aliases: set[str] | None = None,
    variables: dict[str, str] | None = None,
    depth: int = 0,
) -> set[str]:
    if not tokens:
        return set()
    aliases = aliases or set()
    variables = variables or {}
    expanded = merge_split_shell_parameter_assignment_tokens(tokens)
    expanded = expand_known_shell_assignment_names(expanded, variables)
    expanded = expand_known_shell_command_variables(expanded, variables)
    expanded = expand_known_shell_variables(expanded, variables)
    expanded = expand_cargo_aliases(expanded, aliases)
    if not expanded:
        return set()
    if depth > 6:
        if tokens_have_raw_cargo_launch(expanded):
            return direct_raw_cargo_storage_override_messages(expanded)
        return set()
    messages: set[str] = set()
    if tokens_have_top_level_shell_boundary(expanded):
        segment: list[str] = []
        segment_aliases = set(aliases)
        segment_variables = dict(variables)
        substitution_depth = 0
        index = 0
        while index < len(expanded):
            token = expanded[index]
            if token == "$" and index + 1 < len(expanded) and expanded[index + 1] == "(":
                segment.extend([token, expanded[index + 1]])
                substitution_depth += 1
                index += 2
                continue
            if token == "(" and substitution_depth:
                substitution_depth += 1
            elif token == ")" and substitution_depth:
                substitution_depth -= 1
            elif token in SHELL_COMMAND_BOUNDARIES:
                shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    segment_variables.update(shell_assignments)
                    segment = []
                    index += 1
                    continue
                messages.update(
                    raw_cargo_storage_override_messages_from_tokens(
                        segment,
                        aliases=segment_aliases,
                        variables=segment_variables,
                        depth=depth + 1,
                    )
                )
                if segment and segment[0] == "alias":
                    segment_aliases.update(simple_cargo_aliases(segment, segment_aliases))
                segment = []
                index += 1
                continue
            segment.append(token)
            index += 1
        messages.update(
            raw_cargo_storage_override_messages_from_tokens(
                segment,
                aliases=segment_aliases,
                variables=segment_variables,
                depth=depth + 1,
            )
        )
        return messages
    if expanded[0] == "alias":
        return messages
    shell_assignments, assignment_index = shell_assignment_values_from_tokens(expanded)
    if assignment_index:
        local_variables = dict(variables)
        local_variables.update(shell_assignments)
        return raw_cargo_storage_override_messages_from_tokens(
            expanded[assignment_index:],
            aliases=aliases,
            variables=local_variables,
            depth=depth + 1,
        )
    executable = pathlib.Path(expanded[0]).name
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(expanded)
        if nested is not None:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(nested),
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if executable == "eval":
        inner = expanded[1:]
        if inner and inner[0] == "--":
            inner = inner[1:]
        if inner:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(" ".join(inner)),
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if executable.startswith("python"):
        for payload in python_inline_command_payloads(expanded):
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(payload),
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(expanded)
        if inner is not None:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    inner,
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if not tokens_have_raw_cargo_launch(expanded) and not tokens_have_target_routing_override(expanded):
        return messages
    messages.update(direct_raw_cargo_storage_override_messages(expanded))
    return messages
def text_has_path_style_cargo_config(text: str) -> bool:
    for match in re.finditer(r"\bcargo\b[^\n;&|]*", text):
        tokens = command_tokens(match.group(0))
        for index, token in enumerate(tokens):
            if pathlib.Path(token).name != "cargo":
                continue
            cursor = index + 1
            while cursor < len(tokens) and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
                option = tokens[cursor]
                if option == "--config" and cursor + 1 < len(tokens):
                    if cargo_config_looks_like_path(tokens[cursor + 1]):
                        return True
                    cursor += 2
                    continue
                if option.startswith("--config=") and cargo_config_looks_like_path(option.split("=", 1)[1]):
                    return True
                cursor += 1
    return False
def storage_strip_quotes(value: str) -> str:
    return value.strip().strip("\"'")
def shell_assignment_from_tokens(tokens: list[str], index: int) -> tuple[str, str, int] | None:
    if index >= len(tokens) or not shell_assignment_word(tokens[index]):
        return None
    name, value = tokens[index].split("=", 1)
    cursor = index + 1
    if value == "$" and cursor < len(tokens) and tokens[cursor] == "(":
        depth = 1
        parts = [value, tokens[cursor]]
        cursor += 1
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            parts.append(token)
            if token == "(":
                depth += 1
            elif token == ")":
                depth -= 1
            cursor += 1
        value = " ".join(parts)
    elif value == "$" and cursor < len(tokens) and tokens[cursor] == "{":
        depth = 1
        parts = [value, tokens[cursor]]
        cursor += 1
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            parts.append(token)
            if token == "{":
                depth += 1
            elif token == "}":
                depth -= 1
            cursor += 1
        value = " ".join(parts)
    elif value.startswith("`") and not value.endswith("`"):
        parts = [value]
        while cursor < len(tokens):
            token = tokens[cursor]
            parts.append(token)
            cursor += 1
            if token.endswith("`"):
                break
        value = " ".join(parts)
    return name, value, cursor
def tokens_have_top_level_shell_boundary(tokens: list[str]) -> bool:
    substitution_depth = 0
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if (token == "$" or token.endswith("$")) and index + 1 < len(tokens) and tokens[index + 1] == "(":
            substitution_depth += 1
            index += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            return True
        index += 1
    return False
def consume_cargo_global_options(tokens: list[str], index: int) -> int:
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("+"):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    return index
