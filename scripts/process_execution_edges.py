#!/usr/bin/env python3
"""Authoritative Python process-API and command-prefix grammar for repo fences."""

from __future__ import annotations

from typing import NamedTuple


class CommandPrefixSpec(NamedTuple):
    """Declarative grammar for a command-prefix executable."""

    flag_options: frozenset[str]
    value_options: frozenset[str]
    attached_value_prefixes: frozenset[str]
    numeric_options: bool
    leading_operands: int
    default_command: tuple[str, ...] | None = None

PROCESS_API_SPECS = {
    "subprocess": {
        "run": (0, "args"),
        "call": (0, "args"),
        "check_call": (0, "args"),
        "check_output": (0, "args"),
        "Popen": (0, "args"),
        "getoutput": (0, "cmd"),
        "getstatusoutput": (0, "cmd"),
    },
    "asyncio": {
        "create_subprocess_exec": (0, "program"),
        "create_subprocess_shell": (0, "cmd"),
    },
    "os": {
        "system": (0, "command"),
        "popen": (0, "cmd"),
        "execl": (0, "path"),
        "execle": (0, "path"),
        "execlp": (0, "file"),
        "execlpe": (0, "file"),
        "execv": (0, "path"),
        "execve": (0, "path"),
        "execvp": (0, "file"),
        "execvpe": (0, "file"),
        "spawnl": (1, "path"),
        "spawnle": (1, "path"),
        "spawnlp": (1, "file"),
        "spawnlpe": (1, "file"),
        "spawnv": (1, "path"),
        "spawnve": (1, "path"),
        "spawnvp": (1, "file"),
        "spawnvpe": (1, "file"),
        "posix_spawn": (0, "path"),
        "posix_spawnp": (0, "path"),
    },
    "pty": {"spawn": (0, "argv")},
}

OS_EXEC_FUNCTIONS = frozenset(
    name for name in PROCESS_API_SPECS["os"] if name.startswith("exec")
)
OS_SPAWN_FUNCTIONS = frozenset(
    name
    for name in PROCESS_API_SPECS["os"]
    if name.startswith("spawn") and not name.startswith("posix_spawn")
)
SUBPROCESS_CALLS = frozenset(
    frozenset(PROCESS_API_SPECS["subprocess"]) - {"getoutput", "getstatusoutput"}
)
ASYNCIO_EXEC_CALLS = frozenset({"create_subprocess_exec"})
ASYNCIO_SHELL_CALLS = frozenset({"create_subprocess_shell"})
COMMAND_PREFIX_SPECS = {
    "command": CommandPrefixSpec(
        frozenset({"-p", "-v", "-V"}), frozenset(), frozenset(), False, 0
    ),
    "nice": CommandPrefixSpec(
        frozenset({"--help", "--version"}),
        frozenset({"-n", "--adjustment"}),
        frozenset({"-n"}),
        True,
        0,
    ),
    "nohup": CommandPrefixSpec(
        frozenset({"--help", "--version"}), frozenset(), frozenset(), False, 0
    ),
    "stdbuf": CommandPrefixSpec(
        frozenset({"--help", "--version"}),
        frozenset({"-i", "--input", "-o", "--output", "-e", "--error"}),
        frozenset({"-i", "-o", "-e"}),
        False,
        0,
    ),
    "timeout": CommandPrefixSpec(
        frozenset(
            {
                "--foreground",
                "--preserve-status",
                "--verbose",
                "--help",
                "--version",
            }
        ),
        frozenset({"-k", "--kill-after", "-s", "--signal"}),
        frozenset({"-k", "-s"}),
        False,
        1,
    ),
    "time": CommandPrefixSpec(
        frozenset(
            {
                "-a",
                "--append",
                "-p",
                "--portability",
                "-v",
                "--verbose",
                "--help",
                "--version",
            }
        ),
        frozenset({"-f", "--format", "-o", "--output"}),
        frozenset({"-f", "-o"}),
        False,
        0,
    ),
    "xargs": CommandPrefixSpec(
        frozenset(
            {
                "-0",
                "--null",
                "-p",
                "--interactive",
                "-r",
                "--no-run-if-empty",
                "-t",
                "--verbose",
                "-x",
                "--exit",
                "--help",
                "--version",
            }
        ),
        frozenset(
            {
                "-a",
                "--arg-file",
                "-d",
                "--delimiter",
                "-E",
                "--eof",
                "-I",
                "--replace",
                "-L",
                "--max-lines",
                "-n",
                "--max-args",
                "-P",
                "--max-procs",
                "-s",
                "--max-chars",
            }
        ),
        frozenset({"-d", "-E", "-I", "-L", "-n", "-P", "-s"}),
        False,
        0,
        ("echo",),
    ),
}
COMMAND_PREFIX_WRAPPERS = frozenset(COMMAND_PREFIX_SPECS)
SHELL_INTERPRETERS = frozenset({"bash", "dash", "sh", "zsh"})
DYNAMIC_BUILTINS = frozenset({"eval", "exec"})
