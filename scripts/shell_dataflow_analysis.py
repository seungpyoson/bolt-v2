#!/usr/bin/env python3
"""Shell dataflow analyzer helpers relocated from verify_ci_workflow_hygiene."""

from __future__ import annotations

import pathlib
import re
import shlex

from cargo_command_analysis import (
    ENV_OPTIONS_WITHOUT_ARGUMENT,
    ENV_OPTIONS_WITH_ARGUMENT,
    ENV_SIGNAL_OPTIONS,
    RECURSIVE_WRAPPER_EXECUTABLES,
    SHELL_COMMAND_BOUNDARIES,
    SUDO_OPTIONS_WITHOUT_ARGUMENT,
    SUDO_OPTIONS_WITH_ARGUMENT,
    SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
    command_tokens,
    command_tokens_with_line_boundaries,
    consume_option_prefix,
    env_assignment_argument,
    env_command_prefix_index,
    env_short_cluster_next_index,
    env_short_split_tokens,
    executable_name,
    expand_known_shell_assignment_name,
    expand_known_shell_assignment_names,
    expand_known_shell_assignment_value,
    expand_known_shell_variables,
    merge_split_shell_parameter_assignment_tokens,
    persistent_shell_assignment_values,
    shell_alias_payloads,
    shell_assignment_from_tokens,
    shell_assignment_word,
    shell_command,
    shell_command_substitution_payloads,
    shell_identifier_fragment,
    shell_logical_lines,
    shell_quotes_are_balanced,
    shell_redirection_next_index,
    storage_strip_quotes,
    wrapper_inner_tokens,
)
from workflow_expression_analysis import strip_comment, unquote_yaml_scalar

S3_ACTIVE_TARGET_CACHE_MESSAGE = "S3 active mutable target cache must be rejected"
STORAGE_ROLE_S3 = "s3"
STORAGE_ROLE_ACTIVE_TARGET = "active_target"
AWS_S3_TRANSFER_COMMANDS = {"cp", "mv", "sync"}
ACTIVE_TARGET_STDOUT_COMMANDS = {
    "awk",
    "base64",
    "bzcat",
    "cat",
    "egrep",
    "fgrep",
    "grep",
    "gzip",
    "head",
    "sed",
    "tail",
    "tar",
    "xzcat",
    "zcat",
}
AWS_S3_OPTIONS_WITH_ARGUMENT = {
    "--acl",
    "--cache-control",
    "--content-disposition",
    "--content-encoding",
    "--content-language",
    "--content-type",
    "--copy-props",
    "--exclude",
    "--expires",
    "--expected-size",
    "--include",
    "--metadata",
    "--metadata-directive",
    "--page-size",
    "--profile",
    "--region",
    "--request-payer",
    "--sse",
    "--sse-c",
    "--sse-c-copy-source",
    "--sse-c-copy-source-key",
    "--sse-c-key",
    "--sse-kms-key-id",
    "--storage-class",
    "--website-redirect",
}
def storage_without_trailing_current_dir(value: str) -> str:
    normalized = storage_strip_quotes(value).replace('"', "").replace("'", "")
    while normalized.endswith("/.") or normalized.endswith("/"):
        normalized = normalized[:-2] if normalized.endswith("/.") else normalized[:-1]
    return normalized


def storage_variable_names(value: str) -> set[str]:
    names = {match.group(1) for match in re.finditer(r"\$([A-Za-z_][A-Za-z0-9_]*)\b", value)}
    names.update(match.group(1) for match in re.finditer(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:[^}]*)\}", value))
    names.update(match.group(1) for match in re.finditer(r"\$\{\{\s*env\.([A-Za-z_][A-Za-z0-9_]*)\s*\}\}", value))
    return names


def storage_command_substitution_has_target(value: str) -> bool:
    compact = storage_strip_quotes(value).replace('"', "").replace("'", "")
    for payload in shell_command_substitution_payloads(command_tokens(compact)):
        if any(storage_value_has_target_component(token) for token in payload):
            return True
    if ("`" in compact or "$" in compact) and storage_value_has_target_component(storage_value_without_substitutions(compact)):
        return True
    return False


def storage_value_without_substitutions(value: str) -> str:
    compact = re.sub(r"`[^`]*`", "", value)
    tokens = command_tokens(compact)
    output: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if (token == "$" or token.endswith("$")) and index + 1 < len(tokens) and tokens[index + 1] == "(":
            prefix = token[:-1] if token != "$" and token.endswith("$") else ""
            if prefix:
                output.append(prefix)
            index += 2
            depth = 1
            while index < len(tokens) and depth:
                if tokens[index] == "(":
                    depth += 1
                elif tokens[index] == ")":
                    depth -= 1
                index += 1
            continue
        output.append(token)
        index += 1
    return " ".join(output)


def storage_value_has_target_component(value: str) -> bool:
    normalized = storage_strip_quotes(value).replace('"', "").replace("'", "").lstrip("<>")
    if not normalized or normalized.startswith("s3://"):
        return False
    parts = [part for part in re.split(r"[\\/]+", normalized) if part and part not in {".", ".."}]
    return "target" in parts


def storage_value_roles(
    value: str,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool = False,
    active_paths: set[str] | None = None,
) -> set[str]:
    compact = storage_strip_quotes(value).replace('"', "").replace("'", "")
    root_compact = storage_without_trailing_current_dir(value)
    roles: set[str] = set()
    if active_paths is not None and storage_path_is_inside_active_path(root_compact, active_paths):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if "s3://" in compact:
        roles.add(STORAGE_ROLE_S3)
    if "rust_verification.py" in compact and "target-dir" in compact:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if storage_command_substitution_has_target(compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    for payload in shell_command_substitution_payloads(command_tokens(compact)):
        if any(storage_value_has_target_component(token) for token in payload):
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    for variable in storage_variable_names(compact):
        if variable in {"CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_TARGET_TMPDIR"}:
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        if variable in {"GITHUB_WORKSPACE", "PWD"} and root_compact in {
            f"${variable}",
            f"${{{variable}}}",
        }:
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        roles.update(variable_roles.get(variable, set()))
    if re.search(r"\$\{\{\s*(?:env\.CARGO_TARGET_DIR|steps\.setup\.outputs\.managed_target_dir(?:_relative)?)\s*\}\}", compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if re.search(r"\$\{\{\s*github\.workspace\s*\}\}", root_compact) and (
        re.fullmatch(r"\$\{\{\s*github\.workspace\s*\}\}", root_compact.strip()) is not None
        or storage_value_has_target_component(compact)
    ):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if root_compact in {".", "*", "$PWD", "${PWD}", "$GITHUB_WORKSPACE", "${GITHUB_WORKSPACE}"}:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if storage_value_has_target_component(compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if cwd_is_active_target and compact and not compact.startswith("-") and STORAGE_ROLE_S3 not in roles:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    return roles


def storage_path_key(value: str) -> str:
    return storage_without_trailing_current_dir(value).replace('"', "").replace("'", "")


def storage_path_is_inside_active_path(value: str, active_paths: set[str]) -> bool:
    key = storage_path_key(value)
    return any(key == active_path or key.startswith(f"{active_path}/") for active_path in active_paths if active_path)


def command_tail_until_boundary(tokens: list[str], start: int) -> list[str]:
    tail: list[str] = []
    cursor = start
    substitution_depth = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "$" and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            tail.extend([token, tokens[cursor + 1]])
            substitution_depth += 1
            cursor += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            break
        tail.append(token)
        cursor += 1
    return tail


def command_operand_roles(
    operand: str,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
    active_paths: set[str],
) -> set[str]:
    return storage_value_roles(
        operand,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )


def operand_has_s3_path_role(operand: str, s3_paths: set[str]) -> bool:
    return storage_path_is_inside_active_path(storage_without_trailing_current_dir(operand), s3_paths)


def local_transfer_operands(tokens: list[str], index: int) -> tuple[list[str], str] | None:
    tail = command_tail_until_boundary(tokens, index + 1)
    operands: list[str] = []
    target_directory: str | None = None
    cluster_prefix_flags_without_argument = {"a", "d", "f", "H", "i", "L", "l", "n", "P", "p", "R", "r", "s", "u", "v", "x", "Z"}
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token == "--":
            cursor += 1
            continue
        if token in {"-t", "--target-directory"} and cursor + 1 < len(tail):
            target_directory = tail[cursor + 1]
            cursor += 2
            continue
        if token.startswith("-t") and not token.startswith("--") and len(token) > 2:
            target_directory = token[2:]
            cursor += 1
            continue
        if token.startswith("-") and not token.startswith("--") and "t" in token[1:]:
            prefix, suffix = token[1:].split("t", 1)
            if set(prefix) <= cluster_prefix_flags_without_argument:
                if suffix:
                    target_directory = suffix
                    cursor += 1
                elif cursor + 1 < len(tail):
                    target_directory = tail[cursor + 1]
                    cursor += 2
                else:
                    cursor += 1
                continue
        if token.startswith("--target-directory="):
            target_directory = token.split("=", 1)[1]
            cursor += 1
            continue
        if token.startswith("-"):
            cursor += 1
            continue
        operands.append(token)
        cursor += 1
    if target_directory is not None:
        return (operands, target_directory) if operands else None
    if len(operands) < 2:
        return None
    return operands[:-1], operands[-1]


def command_copies_s3_path_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    operands = local_transfer_operands(tokens, index)
    if operands is None:
        return False
    sources, destination = operands
    if not any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        return False
    return STORAGE_ROLE_ACTIVE_TARGET in storage_value_roles(
        destination,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )


def record_local_transfer_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    operands = local_transfer_operands(tokens, index)
    if operands is None:
        return
    sources, destination = operands
    if any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        active_paths.add(storage_path_key(destination))
    if any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        s3_paths.add(storage_path_key(destination))


TAR_SHORT_OPTION_CLUSTER_FLAGS = set("AacdtruxvzjJfCOpPsSMWUmhk")
TAR_SHORT_OPTIONS_WITH_ARGUMENT = {"C", "f"}


def tar_cluster_looks_like_options(cluster: str) -> bool:
    return bool(cluster) and set(cluster) <= TAR_SHORT_OPTION_CLUSTER_FLAGS


def tar_option_parts(token: str, tail: list[str], index: int) -> tuple[set[str], dict[str, str], int, bool]:
    flags: set[str] = set()
    arguments: dict[str, str] = {}
    consumed = 0
    if token == "--":
        return flags, arguments, consumed, True
    if token in {"c", "-c", "--create"}:
        flags.add("c")
        return flags, arguments, consumed, True
    if token in {"x", "-x", "--extract", "--get"}:
        flags.add("x")
        return flags, arguments, consumed, True
    if token in {"-f", "--file"}:
        if index + 1 < len(tail):
            arguments["f"] = tail[index + 1]
            consumed = 1
        return flags, arguments, consumed, True
    if token.startswith("--file="):
        arguments["f"] = token.split("=", 1)[1]
        return flags, arguments, consumed, True
    if token in {"-C", "--directory"}:
        if index + 1 < len(tail):
            arguments["C"] = tail[index + 1]
            consumed = 1
        return flags, arguments, consumed, True
    if token.startswith("--directory="):
        arguments["C"] = token.split("=", 1)[1]
        return flags, arguments, consumed, True
    if token.startswith("--"):
        return flags, arguments, consumed, True

    traditional_cluster = False
    cluster: str | None = None
    if token.startswith("-") and len(token) > 1:
        cluster = token[1:]
    elif tar_cluster_looks_like_options(token):
        cluster = token
        traditional_cluster = True
    if cluster is None:
        return flags, arguments, consumed, False

    argument_offset = 1
    position = 0
    while position < len(cluster):
        flag = cluster[position]
        if flag == "c":
            flags.add("c")
        elif flag == "x":
            flags.add("x")
        if flag in TAR_SHORT_OPTIONS_WITH_ARGUMENT:
            suffix = cluster[position + 1 :]
            if suffix and not (traditional_cluster or tar_cluster_looks_like_options(suffix)):
                arguments[flag] = suffix
                break
            if index + argument_offset < len(tail):
                arguments[flag] = tail[index + argument_offset]
                consumed = max(consumed, argument_offset)
                argument_offset += 1
            position += 1
            continue
        position += 1
    return flags, arguments, consumed, True


def tar_writes_archive_to_stdout(tail: list[str]) -> bool:
    creates_archive = False
    skip_count = 0
    for index, token in enumerate(tail):
        if skip_count:
            skip_count -= 1
            continue
        flags, arguments, consumed, _option_like = tar_option_parts(token, tail, index)
        skip_count = consumed
        if "c" in flags:
            creates_archive = True
        if "f" in arguments:
            return arguments["f"] == "-"
    return creates_archive


def tar_archive_creation(tail: list[str]) -> tuple[str | None, list[str]]:
    creates_archive = False
    archive: str | None = None
    sources: list[str] = []
    skip_count = 0
    for index, token in enumerate(tail):
        if skip_count:
            skip_count -= 1
            continue
        flags, arguments, consumed, option_like = tar_option_parts(token, tail, index)
        skip_count = consumed
        if "c" in flags:
            creates_archive = True
        if "f" in arguments:
            archive = arguments["f"]
        if option_like:
            continue
        sources.append(token)
    return (archive, sources) if creates_archive else (None, [])


def tar_archive_inputs(tail: list[str]) -> list[str]:
    archives: list[str] = []
    skip_count = 0
    for index, token in enumerate(tail):
        if skip_count:
            skip_count -= 1
            continue
        _flags, arguments, consumed, option_like = tar_option_parts(token, tail, index)
        skip_count = consumed
        if "f" in arguments and arguments["f"] != "-":
            archives.append(arguments["f"])
            continue
        if option_like:
            continue
        archives.append(token)
    return archives


def record_tar_archive_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    archive, sources = tar_archive_creation(command_tail_until_boundary(tokens, index + 1))
    if archive is None or archive == "-":
        return
    if any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        active_paths.add(storage_path_key(archive))
    if any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        s3_paths.add(storage_path_key(archive))


def tar_extracts_s3_archive_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    if not tar_extracts_to_active_target(
        tokens,
        index,
        variable_roles,
        active_paths,
        cwd_is_active_target=cwd_is_active_target,
    ):
        return False
    return any(operand_has_s3_path_role(archive, s3_paths) for archive in tar_archive_inputs(tail))


def zip_archive_operands(tokens: list[str], index: int) -> tuple[str, list[str]] | None:
    tail = command_tail_until_boundary(tokens, index + 1)
    operands: list[str] = []
    options_with_argument = {
        "-b",
        "-i",
        "-n",
        "-O",
        "-P",
        "-t",
        "-x",
        "--before-date",
        "--exclude",
        "--from-date",
        "--include",
        "--out",
        "--output-file",
        "--password",
        "--suffixes",
        "--temp-path",
    }
    short_options_with_argument = {"b", "i", "n", "O", "P", "t", "x"}
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token == "--":
            cursor += 1
            continue
        if token in options_with_argument and cursor + 1 < len(tail):
            cursor += 2
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            cursor += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            argument_consumed = False
            for position, flag in enumerate(cluster):
                if flag not in short_options_with_argument:
                    continue
                if position + 1 < len(cluster):
                    cursor += 1
                elif cursor + 1 < len(tail):
                    cursor += 2
                else:
                    cursor += 1
                argument_consumed = True
                break
            if argument_consumed:
                continue
        if token.startswith("-"):
            cursor += 1
            continue
        operands.append(token)
        cursor += 1
    if len(operands) < 2:
        return None
    return operands[0], operands[1:]


def record_zip_archive_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    operands = zip_archive_operands(tokens, index)
    if operands is None:
        return
    archive, sources = operands
    if any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        active_paths.add(storage_path_key(archive))
    if any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        s3_paths.add(storage_path_key(archive))


def unzip_extracts_s3_archive_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    archives: list[str] = []
    members: list[str] = []
    destination: str | None = None
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token in {"-d", "--directory"} and cursor + 1 < len(tail):
            destination = tail[cursor + 1]
            cursor += 2
            continue
        if token.startswith("--directory="):
            destination = token.split("=", 1)[1]
            cursor += 1
            continue
        if token in {"-x", "--exclude", "-P", "--password"} and cursor + 1 < len(tail):
            cursor += 2
            continue
        if token.startswith(("--exclude=", "--password=")):
            cursor += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            argument_consumed = False
            for position, flag in enumerate(cluster):
                if flag == "d":
                    if position + 1 < len(cluster):
                        destination = cluster[position + 1 :]
                        cursor += 1
                    elif cursor + 1 < len(tail):
                        destination = tail[cursor + 1]
                        cursor += 2
                    else:
                        cursor += 1
                    argument_consumed = True
                    break
                if flag in {"x", "P"}:
                    if position + 1 < len(cluster):
                        cursor += 1
                    elif cursor + 1 < len(tail):
                        cursor += 2
                    else:
                        cursor += 1
                    argument_consumed = True
                    break
            if argument_consumed:
                continue
        if token == "--" or token.startswith("-"):
            cursor += 1
            continue
        if archives:
            members.append(token)
        else:
            archives.append(token)
        cursor += 1
    if not any(operand_has_s3_path_role(archive, s3_paths) for archive in archives):
        return False
    if cwd_is_active_target and destination is None:
        return True
    destination_is_active = destination is not None and STORAGE_ROLE_ACTIVE_TARGET in storage_value_roles(
        destination,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )
    return destination_is_active or any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            member,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for member in members
    )


def command_streams_active_target_to_stdout(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
    command_name: str,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    if command_name == "tar" and not tar_writes_archive_to_stdout(tail):
        return False
    return any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            token,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for token in tail
        if token != "-" and not token.startswith("-")
    )


def output_redirection_targets(tokens: list[str], index: int) -> list[str]:
    targets: list[str] = []
    tail = command_tail_until_boundary(tokens, index + 1)
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token in {">", ">>", "<>", ">|", ">&", "&>", "&>>"}:
            if cursor + 1 < len(tail):
                targets.append(tail[cursor + 1])
            cursor += 2
            continue
        match = re.match(r"^(?:\d?(?:>>?|<>|>\||>&)|&>>?)(.+)$", token)
        if match is not None:
            targets.append(match.group(1))
        cursor += 1
    return targets


def command_output_redirects_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    return any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            target,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for target in output_redirection_targets(tokens, index)
    )


def tar_extracts_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    extracts = False
    directories: list[str] = []
    members: list[str] = []
    skip_count = 0
    cursor = 0
    while cursor < len(tail):
        if skip_count:
            skip_count -= 1
            cursor += 1
            continue
        token = tail[cursor]
        flags, arguments, consumed, option_like = tar_option_parts(token, tail, cursor)
        skip_count = consumed
        if "x" in flags:
            extracts = True
        if "C" in arguments:
            directories.append(arguments["C"])
        if option_like:
            cursor += 1
            continue
        if token != "--":
            members.append(token)
        cursor += 1
    if not extracts:
        return False
    if cwd_is_active_target:
        return True
    return any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            directory,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for directory in directories
    ) or any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            member,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for member in members
    )


def command_writes_s3_stdin_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
    command_name: str,
) -> bool:
    if command_output_redirects_to_active_target(
        tokens,
        index,
        variable_roles,
        active_paths,
        cwd_is_active_target=cwd_is_active_target,
    ):
        return True
    if command_name == "tar" and tar_extracts_to_active_target(
        tokens,
        index,
        variable_roles,
        active_paths,
        cwd_is_active_target=cwd_is_active_target,
    ):
        return True
    if command_name == "tee":
        return any(
            STORAGE_ROLE_ACTIVE_TARGET
            in storage_value_roles(
                token,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            for token in command_tail_until_boundary(tokens, index + 1)
            if token != "-" and not token.startswith("-")
        )
    return False
def storage_assignment_values(text: str) -> list[tuple[str, str]]:
    assignments: list[tuple[str, str]] = []
    tokens = command_tokens(text)
    cursor = 0
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            cursor += 1
            continue
        name, value, cursor = assignment
        assignments.append((name, value))
    for line in text.splitlines():
        clean = strip_comment(line).strip()
        match = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+?)\s*$", clean)
        if match:
            assignments.append((match.group(1), match.group(2)))
    for github_env_assignment in github_env_assignment_lines(text):
        name, value = github_env_assignment.split("=", 1)
        assignments.append((name, value))
    return assignments


def storage_variable_roles(text: str) -> dict[str, set[str]]:
    assignments = storage_assignment_values(text)
    roles: dict[str, set[str]] = {}
    for _ in range(max(1, len(assignments))):
        changed = False
        for name, value in assignments:
            new_roles = storage_value_roles(value, roles)
            if new_roles and not new_roles.issubset(roles.get(name, set())):
                roles.setdefault(name, set()).update(new_roles)
                changed = True
        if not changed:
            break
    return roles


def consume_storage_option(tokens: list[str], index: int, options_with_argument: set[str]) -> int:
    token = tokens[index]
    if token in options_with_argument and index + 1 < len(tokens):
        return index + 2
    return index + 1


def aws_service_index(tokens: list[str], start: int) -> int | None:
    cursor = start + 1
    while cursor < len(tokens) and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        token = tokens[cursor]
        if token in {"s3", "s3api"}:
            return cursor
        if token.startswith("-"):
            if (
                "=" not in token
                and cursor + 1 < len(tokens)
                and tokens[cursor + 1] not in {"s3", "s3api"}
                and not tokens[cursor + 1].startswith("-")
            ):
                cursor += 2
            else:
                cursor += 1
            continue
        cursor += 1
    return None


def aws_s3_operands(tokens: list[str]) -> list[str]:
    operands: list[str] = []
    cursor = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in SHELL_COMMAND_BOUNDARIES:
            break
        if token == "-":
            operands.append(token)
            cursor += 1
            continue
        if token.startswith("-"):
            cursor = consume_storage_option(tokens, cursor, AWS_S3_OPTIONS_WITH_ARGUMENT)
            continue
        if "`" in token:
            parts = [token]
            cursor += 1
            backtick_count = token.count("`")
            while cursor < len(tokens) and backtick_count % 2 == 1:
                parts.append(tokens[cursor])
                backtick_count += tokens[cursor].count("`")
                cursor += 1
            operands.append(" ".join(parts))
            continue
        if (token == "$" or token.endswith("$")) and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            depth = 1
            parts = [token, tokens[cursor + 1]]
            substitution_tokens: list[str] = []
            cursor += 2
            while cursor < len(tokens) and depth:
                current = tokens[cursor]
                parts.append(current)
                if current != ")" or depth > 1:
                    substitution_tokens.append(current)
                if current == "(":
                    depth += 1
                elif current == ")":
                    depth -= 1
                cursor += 1
            if (
                cursor < len(tokens)
                and not any(part in SHELL_COMMAND_BOUNDARIES for part in substitution_tokens)
                and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES
                and not tokens[cursor].startswith("-")
                and not tokens[cursor].startswith("s3://")
            ):
                parts.append(tokens[cursor])
                cursor += 1
            operands.append(" ".join(parts))
            continue
        operands.append(token)
        cursor += 1
    return operands


def aws_s3_transfer_touches_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
    active_paths: set[str],
    stdin_is_active_target: bool = False,
) -> bool:
    service_index = aws_service_index(tokens, index)
    if service_index is None:
        return False
    service = tokens[service_index]
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] in SHELL_COMMAND_BOUNDARIES:
        return False
    operation = tokens[op_index]
    tail: list[str] = []
    cursor = op_index + 1
    command_substitution_depth = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if (token == "$" or token.endswith("$")) and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            tail.extend([token, tokens[cursor + 1]])
            command_substitution_depth += 1
            cursor += 2
            continue
        if token == "(" and command_substitution_depth:
            command_substitution_depth += 1
        elif token == ")" and command_substitution_depth:
            command_substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not command_substitution_depth:
            break
        tail.append(token)
        cursor += 1
    if service == "s3api":
        return any(
            STORAGE_ROLE_ACTIVE_TARGET
            in storage_value_roles(
                token,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            for token in tail
        )
    if operation not in AWS_S3_TRANSFER_COMMANDS:
        return False
    operands = aws_s3_operands(tail)
    if len(operands) < 2:
        return False
    if stdin_is_active_target and operation == "cp" and "-" in operands:
        return True
    endpoint_roles = [
        storage_value_roles(
            endpoint,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for endpoint in operands
    ]
    return any(STORAGE_ROLE_ACTIVE_TARGET in roles for roles in endpoint_roles)


def aws_s3_transfer_streams_s3_to_stdout(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
    active_paths: set[str],
) -> bool:
    service_index = aws_service_index(tokens, index)
    if service_index is None or tokens[service_index] != "s3":
        return False
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] != "cp":
        return False
    operands = aws_s3_operands(command_tail_until_boundary(tokens, op_index + 1))
    if len(operands) < 2:
        return False
    source = operands[0]
    destination = operands[1]
    if destination != "-":
        return False
    return STORAGE_ROLE_S3 in storage_value_roles(
        source,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )


def record_aws_s3_download_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    service_index = aws_service_index(tokens, index)
    if service_index is None or tokens[service_index] != "s3":
        return
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] in SHELL_COMMAND_BOUNDARIES:
        return
    operation = tokens[op_index]
    if operation not in {"cp", "mv", "sync"}:
        return
    operands = aws_s3_operands(command_tail_until_boundary(tokens, op_index + 1))
    if len(operands) < 2:
        return
    sources = operands[:-1]
    destination = operands[-1]
    if destination == "-" or STORAGE_ROLE_S3 in storage_value_roles(
        destination,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    ):
        return
    if any(
        STORAGE_ROLE_S3
        in storage_value_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        s3_paths.add(storage_path_key(destination))


def command_prefix_before_token(tokens: list[str], index: int) -> list[str]:
    cursor = index - 1
    while cursor >= 0 and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        cursor -= 1
    return tokens[cursor + 1 : index]


def env_chdir_value(tokens: list[str]) -> str | None:
    command_index = env_command_prefix_index(tokens, 1)
    if command_index is None:
        return None
    index = 1
    while index < command_index:
        token = tokens[index]
        if token in ("-C", "--chdir") and index + 1 < command_index:
            return tokens[index + 1]
        if token.startswith("--chdir="):
            return token.split("=", 1)[1]
        if token.startswith("-") and not token.startswith("--") and "C" in token[1:]:
            offset = 1
            while offset < len(token):
                option = token[offset]
                if option in "0iv":
                    offset += 1
                    continue
                if option == "C":
                    suffix = token[offset + 1 :]
                    if suffix:
                        return suffix
                    if index + 1 < command_index:
                        return tokens[index + 1]
                    break
                if option in "Su":
                    index += 1 if offset + 1 < len(token) or index + 1 >= command_index else 2
                    break
                break
            else:
                index += 1
                continue
            if index >= command_index or token[offset] not in "Su":
                index += 1
            continue
        if token in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < command_index:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        index += 1
    return None


def sudo_chdir_value(tokens: list[str]) -> str | None:
    command_index = consume_option_prefix(
        tokens,
        1,
        SUDO_OPTIONS_WITH_ARGUMENT,
        SUDO_OPTIONS_WITHOUT_ARGUMENT,
        SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
    )
    if command_index is None:
        return None
    index = 1
    short_options_with_argument = {option[1] for option in SUDO_OPTIONS_WITH_ARGUMENT if re.match(r"^-[A-Za-z0-9]$", option)}
    short_options_without_argument = {option[1] for option in SUDO_OPTIONS_WITHOUT_ARGUMENT if re.match(r"^-[A-Za-z0-9]$", option)}
    while index < command_index:
        token = tokens[index]
        if token in ("-D", "--chdir") and index + 1 < command_index:
            return tokens[index + 1]
        if token.startswith("--chdir="):
            return token.split("=", 1)[1]
        if token.startswith("-") and not token.startswith("--") and "D" in token[1:]:
            offset = 1
            while offset < len(token):
                option = token[offset]
                if option in short_options_without_argument:
                    offset += 1
                    continue
                if option == "D":
                    suffix = token[offset + 1 :]
                    if suffix:
                        return suffix
                    if index + 1 < command_index:
                        return tokens[index + 1]
                    break
                if option in short_options_with_argument:
                    index += 1 if offset + 1 < len(token) or index + 1 >= command_index else 2
                    break
                break
            else:
                index += 1
                continue
            if index >= command_index or token[offset] not in short_options_with_argument - {"D"}:
                index += 1
            continue
        if token in SUDO_OPTIONS_WITH_ARGUMENT and index + 1 < command_index:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in SUDO_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        index += 1
    return None


def directory_wrapper_chdir_value(tokens: list[str]) -> str | None:
    if not tokens:
        return None
    executable = executable_name(tokens[0])
    if executable == "env":
        return env_chdir_value(tokens)
    if executable == "sudo":
        return sudo_chdir_value(tokens)
    return None


def cd_option_token(token: str) -> bool:
    if token in {"-L", "-P", "-e"}:
        return True
    return token.startswith("-") and not token.startswith("--") and len(token) > 1 and set(token[1:]) <= {"L", "P", "e"}


def shell_directory_change_target(tokens: list[str], cursor: int) -> tuple[str | None, int]:
    if cursor >= len(tokens):
        return None, cursor + 1
    name = executable_name(tokens[cursor])
    index = cursor + 1
    while name == "cd" and index < len(tokens) and cd_option_token(tokens[index]):
        index += 1
    while name == "pushd" and index < len(tokens) and tokens[index] == "-n":
        index += 1
    if index < len(tokens) and tokens[index] == "--":
        index += 1
    if index >= len(tokens) or tokens[index] in SHELL_COMMAND_BOUNDARIES:
        return None, index
    return tokens[index], index + 1


def shell_group_end_index(tokens: list[str], cursor: int) -> int | None:
    opener = tokens[cursor]
    closer = "}" if opener == "{" else ")"
    depth = 1
    index = cursor + 1
    while index < len(tokens):
        token = tokens[index]
        if token == opener:
            depth += 1
        elif token == closer:
            depth -= 1
            if depth == 0:
                return index
        index += 1
    return None


def skip_shell_redirections(tokens: list[str], cursor: int) -> int:
    while cursor < len(tokens):
        next_cursor = shell_redirection_next_index(tokens, cursor)
        if next_cursor is None:
            break
        cursor = next_cursor
    return cursor


def storage_stdout_roles_from_tokens(
    tokens: list[str],
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    depth: int,
    initial_cwd_is_active_target: bool,
) -> set[str]:
    if depth > 6:
        return set()
    roles: set[str] = set()
    cursor = 0
    cwd_is_active_target = initial_cwd_is_active_target
    pipe_stdin_is_active_target = False
    pipe_stdin_is_s3 = False
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is not None:
            cursor = assignment[2]
            continue
        token = tokens[cursor]
        if token in {"{", "("}:
            close_index = shell_group_end_index(tokens, cursor)
            if close_index is None:
                cursor += 1
                continue
            inner_roles = storage_stdout_roles_from_tokens(
                tokens[cursor + 1 : close_index],
                variable_roles,
                active_paths,
                depth=depth + 1,
                initial_cwd_is_active_target=cwd_is_active_target,
            )
            roles.update(inner_roles)
            cursor = skip_shell_redirections(tokens, close_index + 1)
            continue
        if token in SHELL_COMMAND_BOUNDARIES:
            if token == "|":
                pipe_stdin_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in roles
                pipe_stdin_is_s3 = STORAGE_ROLE_S3 in roles
            else:
                pipe_stdin_is_active_target = False
                pipe_stdin_is_s3 = False
            cursor += 1
            continue
        name = executable_name(token)
        if name in {"cd", "pushd"}:
            directory_target, next_cursor = shell_directory_change_target(tokens, cursor)
            if directory_target is None:
                if name == "cd":
                    cwd_is_active_target = False
                cursor = next_cursor
                continue
            target_roles = storage_value_roles(
                directory_target,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            cwd_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in target_roles
            cursor = next_cursor
            continue
        if name in ACTIVE_TARGET_STDOUT_COMMANDS and (
            pipe_stdin_is_active_target
            or command_streams_active_target_to_stdout(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                cwd_is_active_target=cwd_is_active_target,
                command_name=name,
            )
        ):
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        elif pipe_stdin_is_active_target and name != "aws":
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        if name == "aws" and aws_s3_transfer_streams_s3_to_stdout(
            tokens,
            cursor,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        ):
            roles.add(STORAGE_ROLE_S3)
        elif pipe_stdin_is_s3:
            roles.add(STORAGE_ROLE_S3)
        cursor += 1
    return roles


def storage_transfer_policy_errors_from_tokens(
    tokens: list[str],
    variable_roles: dict[str, set[str]],
    *,
    depth: int = 0,
    initial_cwd_is_active_target: bool = False,
    initial_active_paths: set[str] | None = None,
    initial_s3_paths: set[str] | None = None,
    initial_pipe_stdin_is_active_target: bool = False,
    initial_pipe_stdin_is_s3: bool = False,
) -> list[str]:
    if depth > 6:
        return []
    cursor = 0
    cwd_is_active_target = initial_cwd_is_active_target
    active_paths: set[str] = set(initial_active_paths or set())
    s3_paths: set[str] = set(initial_s3_paths or set())
    pipe_stdout_is_active_target = False
    pipe_stdin_is_active_target = initial_pipe_stdin_is_active_target
    pipe_stdout_is_s3 = False
    pipe_stdin_is_s3 = initial_pipe_stdin_is_s3
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is not None:
            cursor = assignment[2]
            continue
        token = tokens[cursor]
        if token in {"{", "("}:
            close_index = shell_group_end_index(tokens, cursor)
            if close_index is None:
                cursor += 1
                continue
            inner_tokens = tokens[cursor + 1 : close_index]
            nested_errors = storage_transfer_policy_errors_from_tokens(
                inner_tokens,
                variable_roles,
                depth=depth + 1,
                initial_cwd_is_active_target=cwd_is_active_target,
                initial_active_paths=active_paths,
                initial_s3_paths=s3_paths,
                initial_pipe_stdin_is_active_target=pipe_stdin_is_active_target,
                initial_pipe_stdin_is_s3=pipe_stdin_is_s3,
            )
            if nested_errors:
                return nested_errors
            group_stdout_roles = storage_stdout_roles_from_tokens(
                inner_tokens,
                variable_roles,
                active_paths,
                depth=depth + 1,
                initial_cwd_is_active_target=cwd_is_active_target,
            )
            if STORAGE_ROLE_S3 in group_stdout_roles and command_output_redirects_to_active_target(
                tokens,
                close_index,
                variable_roles,
                active_paths,
                cwd_is_active_target=cwd_is_active_target,
            ):
                return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
            pipe_stdout_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in group_stdout_roles
            pipe_stdout_is_s3 = STORAGE_ROLE_S3 in group_stdout_roles
            cursor = skip_shell_redirections(tokens, close_index + 1)
            continue
        if token in SHELL_COMMAND_BOUNDARIES:
            if token == "|":
                pipe_stdin_is_active_target = pipe_stdout_is_active_target
                pipe_stdin_is_s3 = pipe_stdout_is_s3
            else:
                pipe_stdin_is_active_target = False
                pipe_stdin_is_s3 = False
            pipe_stdout_is_active_target = False
            pipe_stdout_is_s3 = False
            cursor += 1
            continue
        name = executable_name(token)
        if name in {"bash", "dash", "fish", "sh", "zsh"}:
            nested = shell_command(tokens[cursor:])
            if nested is not None:
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    command_tokens(nested),
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=cwd_is_active_target,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                )
                if nested_errors:
                    return nested_errors
        if name == "eval":
            inner = tokens[cursor + 1 :]
            if inner and inner[0] == "--":
                inner = inner[1:]
            if inner:
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    command_tokens(" ".join(inner)),
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=cwd_is_active_target,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                )
                if nested_errors:
                    return nested_errors
        chdir_value = directory_wrapper_chdir_value([token] + command_tail_until_boundary(tokens, cursor + 1))
        if chdir_value is not None:
            segment = [token] + command_tail_until_boundary(tokens, cursor + 1)
            inner = wrapper_inner_tokens(segment)
            if inner:
                chdir_roles = storage_value_roles(
                    chdir_value,
                    variable_roles,
                    cwd_is_active_target=cwd_is_active_target,
                    active_paths=active_paths,
                )
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    inner,
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=STORAGE_ROLE_ACTIVE_TARGET in chdir_roles,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                )
                if nested_errors:
                    return nested_errors
        if name in RECURSIVE_WRAPPER_EXECUTABLES:
            segment = [token] + command_tail_until_boundary(tokens, cursor + 1)
            inner = wrapper_inner_tokens(segment)
            if inner:
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    inner,
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=cwd_is_active_target,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                    initial_pipe_stdin_is_active_target=pipe_stdin_is_active_target,
                    initial_pipe_stdin_is_s3=pipe_stdin_is_s3,
                )
                if nested_errors:
                    return nested_errors
        if name in {"cd", "pushd"}:
            directory_target, next_cursor = shell_directory_change_target(tokens, cursor)
            if directory_target is None:
                if name == "cd":
                    cwd_is_active_target = False
                cursor = next_cursor
                continue
            target_roles = storage_value_roles(
                directory_target,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            cwd_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in target_roles
            cursor = next_cursor
            continue
        if name in {"cp", "rsync", "mv"}:
            if command_copies_s3_path_to_active_target(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            ):
                return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
            record_local_transfer_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
        if name == "tar":
            if tar_extracts_s3_archive_to_active_target(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            ):
                return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
            record_tar_archive_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
        if name == "zip":
            record_zip_archive_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
        if name == "unzip" and unzip_extracts_s3_archive_to_active_target(
            tokens,
            cursor,
            variable_roles,
            active_paths,
            s3_paths,
            cwd_is_active_target=cwd_is_active_target,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        if name in ACTIVE_TARGET_STDOUT_COMMANDS:
            pipe_stdout_is_active_target = pipe_stdin_is_active_target or command_streams_active_target_to_stdout(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                cwd_is_active_target=cwd_is_active_target,
                command_name=name,
            )
        elif pipe_stdin_is_active_target and name != "aws":
            pipe_stdout_is_active_target = True
        if pipe_stdin_is_s3 and command_writes_s3_stdin_to_active_target(
            tokens,
            cursor,
            variable_roles,
            active_paths,
            cwd_is_active_target=cwd_is_active_target,
            command_name=name,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        if name == "aws" and aws_s3_transfer_touches_active_target(
            tokens,
            cursor,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
            stdin_is_active_target=pipe_stdin_is_active_target,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        if name == "aws":
            record_aws_s3_download_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
            pipe_stdout_is_s3 = aws_s3_transfer_streams_s3_to_stdout(
                tokens,
                cursor,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            pipe_stdin_is_active_target = False
            pipe_stdin_is_s3 = False
        elif pipe_stdin_is_s3:
            pipe_stdout_is_s3 = True
        cursor += 1
    return []


def storage_transfer_policy_errors(text: str) -> list[str]:
    variable_roles = storage_variable_roles(text)
    return storage_transfer_policy_errors_from_tokens(command_tokens_with_line_boundaries(text), variable_roles)


def target_env_key_alias(value: str, target_keys: dict[str, str]) -> str | None:
    clean = storage_strip_quotes(value)
    compact = re.sub(r"\s+", "", clean)
    if clean in target_keys:
        return clean
    for target_key in target_keys:
        if target_key not in clean:
            continue
        if compact.startswith("$(") or compact.startswith("`") or compact.startswith("${"):
            return target_key
    return None


def shell_assignment_alias_value(value: str, target_keys: dict[str, str]) -> str | None:
    target_key = target_env_key_alias(value, target_keys)
    if target_key is not None:
        return target_key
    clean = storage_strip_quotes(value)
    for pattern in (r"\$\(\s*echo\s+([A-Za-z_][A-Za-z0-9_]*)\s*\)", r"`\s*echo\s+([A-Za-z_][A-Za-z0-9_]*)\s*`"):
        match = re.fullmatch(pattern, clean)
        if match and match.group(1) in target_keys:
            return match.group(1)
    return shell_identifier_fragment(value)


def shell_assignment_tracking_value(value: str, target_keys: dict[str, str]) -> str:
    alias_value = shell_assignment_alias_value(value, target_keys)
    return alias_value if alias_value is not None else storage_strip_quotes(value)


def target_env_key_from_assignment_name(
    name: str,
    assignments: dict[str, str],
    target_keys: dict[str, str],
) -> str | None:
    clean = storage_strip_quotes(name)
    if clean in target_keys:
        return clean
    expanded = expand_known_shell_assignment_name(clean, assignments)
    if expanded in target_keys:
        return expanded
    return None


RUSTFLAGS_OUTPUT_OVERRIDE_KEYS = {
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTFLAGS",
}


def rustflags_value_has_output_override(value: str, assignments: dict[str, str] | None = None) -> bool:
    clean = expand_known_shell_assignment_value(value, assignments or {})
    return "--out-dir" in clean or "--artifact-dir" in clean


def dynamic_env_assignment_message(
    token: str,
    assignments: dict[str, str],
    target_keys: dict[str, str],
) -> str | None:
    if "=" not in token:
        return None
    name, value = token.split("=", 1)
    target_key = target_env_key_from_assignment_name(name, assignments, target_keys)
    if target_key is None:
        return None
    if target_key in RUSTFLAGS_OUTPUT_OVERRIDE_KEYS and not rustflags_value_has_output_override(value, assignments):
        return None
    return target_keys[target_key]


def dynamic_env_segment_messages(
    segment: list[str],
    assignments: dict[str, str],
    target_keys: dict[str, str],
    *,
    depth: int = 0,
) -> set[str]:
    if not segment or depth > 4:
        return set()
    messages: set[str] = set()
    expanded = merge_split_shell_parameter_assignment_tokens(segment)
    local_assignments = dict(assignments)
    cursor = 0
    while cursor < len(expanded):
        current = expand_known_shell_assignment_names([expanded[cursor]], local_assignments)[0]
        if not shell_assignment_word(current):
            break
        expanded[cursor] = current
        message = dynamic_env_assignment_message(current, local_assignments, target_keys)
        if message is not None:
            messages.add(message)
        name, value = current.split("=", 1)
        local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
        cursor += 1
    if cursor >= len(expanded):
        return messages
    expanded = expanded[:cursor] + expand_known_shell_assignment_names(expanded[cursor:], local_assignments)
    command = pathlib.Path(expanded[cursor]).name
    if command == "alias":
        for payload in shell_alias_payloads(expanded[cursor:]).values():
            messages.update(
                dynamic_env_tokens_messages(
                    command_tokens(payload),
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
        return messages
    if command == "export":
        for argument in expanded[cursor + 1 :]:
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            message = dynamic_env_assignment_message(argument, local_assignments, target_keys)
            if message is not None:
                messages.add(message)
            if shell_assignment_word(argument):
                name, value = argument.split("=", 1)
                local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
        return messages
    if command in {"declare", "local", "typeset"}:
        for argument in expanded[cursor + 1 :]:
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            if argument == "--" or argument.startswith(("-", "+")):
                continue
            message = dynamic_env_assignment_message(argument, local_assignments, target_keys)
            if message is not None:
                messages.add(message)
            if shell_assignment_word(argument):
                name, value = argument.split("=", 1)
                local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
        return messages
    if command == "env":
        index = cursor + 1
        while index < len(expanded):
            argument = expanded[index]
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            redirection_index = shell_redirection_next_index(expanded, index)
            if redirection_index is not None:
                index = redirection_index
                continue
            if argument == "--":
                index += 1
                continue
            if argument in ENV_OPTIONS_WITHOUT_ARGUMENT or argument in ENV_SIGNAL_OPTIONS:
                index += 1
                continue
            if any(argument.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
                index += 1
                continue
            if argument in {"-S", "--split-string"} and index + 1 < len(expanded):
                split_inner = command_tokens(expanded[index + 1]) + expanded[index + 2 :]
                messages.update(
                    dynamic_env_tokens_messages(
                        expand_known_shell_variables(split_inner, local_assignments),
                        local_assignments,
                        target_keys,
                        depth=depth + 1,
                    )
                )
                return messages
            if argument.startswith("--split-string="):
                split_inner = command_tokens(argument.split("=", 1)[1]) + expanded[index + 1 :]
                messages.update(
                    dynamic_env_tokens_messages(
                        expand_known_shell_variables(split_inner, local_assignments),
                        local_assignments,
                        target_keys,
                        depth=depth + 1,
                    )
                )
                return messages
            if argument in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < len(expanded):
                index += 2
                continue
            if any(
                argument.startswith(f"{option}=")
                for option in ENV_OPTIONS_WITH_ARGUMENT
                if option.startswith("--")
            ):
                index += 1
                continue
            if argument.startswith("-") and not argument.startswith("--"):
                split_inner = env_short_split_tokens(expanded, index)
                if split_inner is not None:
                    messages.update(
                        dynamic_env_tokens_messages(
                            expand_known_shell_variables(split_inner, local_assignments),
                            local_assignments,
                            target_keys,
                            depth=depth + 1,
                        )
                    )
                    return messages
                parsed_index = env_short_cluster_next_index(expanded, index, argument[1:])
                if parsed_index is not None:
                    index = parsed_index
                    continue
            message = dynamic_env_assignment_message(argument, local_assignments, target_keys)
            if message is None and not env_assignment_argument(argument):
                break
            if message is not None:
                messages.add(message)
            if shell_assignment_word(argument):
                name, value = argument.split("=", 1)
                local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
            index += 1
        if index < len(expanded) and expanded[index] not in SHELL_COMMAND_BOUNDARIES:
            inner = expand_known_shell_variables(expanded[index:], local_assignments)
            messages.update(
                dynamic_env_tokens_messages(
                    inner,
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
        return messages
    if command == "eval":
        inner = expanded[cursor + 1 :]
        if inner and inner[0] == "--":
            inner = inner[1:]
        if inner:
            inner = expand_known_shell_variables(inner, local_assignments)
            messages.update(
                dynamic_env_tokens_messages(
                    command_tokens(" ".join(inner)),
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    if command in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(expanded[cursor:])
        if nested is not None:
            nested_tokens = expand_known_shell_variables(command_tokens(nested), local_assignments)
            messages.update(
                dynamic_env_tokens_messages(
                    nested_tokens,
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    if command in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(expanded[cursor:])
        if inner is not None:
            messages.update(
                dynamic_env_tokens_messages(
                    inner,
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    return messages


def dynamic_env_tokens_messages(
    tokens: list[str],
    assignments: dict[str, str],
    target_keys: dict[str, str],
    *,
    depth: int = 0,
) -> set[str]:
    messages: set[str] = set()
    for segment in shell_command_segments_from_tokens(tokens):
        messages.update(dynamic_env_segment_messages(segment, assignments, target_keys, depth=depth))
    return messages


def shell_command_segments_from_tokens(tokens: list[str]) -> list[list[str]]:
    segments: list[list[str]] = []
    segment: list[str] = []
    expanded = merge_split_shell_parameter_assignment_tokens(tokens)
    index = 0
    substitution_depth = 0
    while index < len(expanded):
        assignment = shell_assignment_from_tokens(expanded, index)
        if assignment is not None:
            _name, _value, next_index = assignment
            segment.extend(expanded[index:next_index])
            index = next_index
            continue
        token = expanded[index]
        if (token == "$" or token.endswith("$")) and index + 1 < len(expanded) and expanded[index + 1] == "(":
            segment.extend([token, expanded[index + 1]])
            substitution_depth += 1
            index += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            if segment:
                segments.append(segment)
            segment = []
            index += 1
            continue
        segment.append(token)
        index += 1
    if segment:
        segments.append(segment)
    return segments
def dynamic_env_target_override_messages(text: str) -> set[str]:
    messages: set[str] = set()
    target_keys = {
        "CARGO_TARGET_DIR": "CARGO_TARGET_DIR raw target override must be classified",
        "CARGO_BUILD_TARGET_DIR": "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        "CARGO_TARGET_TMPDIR": "CARGO_TARGET_TMPDIR raw target override must be classified",
        "CARGO_INCREMENTAL": "CARGO_INCREMENTAL raw cache override must be classified",
        "CARGO_BUILD_RUSTFLAGS": "CARGO_BUILD_RUSTFLAGS raw output override must be classified",
        "CARGO_ENCODED_RUSTFLAGS": "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        "CARGO_INSTALL_ROOT": "CARGO_INSTALL_ROOT install output override must be classified",
        "CARGO_HOME": "CARGO_HOME raw cache override must be classified",
        "RUSTUP_HOME": "RUSTUP_HOME raw toolchain override must be classified",
        "RUSTFLAGS": "RUSTFLAGS raw output override must be classified",
        "RUSTC_WRAPPER": "RUSTC_WRAPPER raw compiler wrapper must be classified",
        "RUSTC_WORKSPACE_WRAPPER": "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified",
        "BOLT_ALLOW_LOCAL_RUST": "BOLT_ALLOW_LOCAL_RUST local Rust break-glass must not be checked in",
        "BOLT_MANAGED_JUST": "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        "GITHUB_ACTIONS": "GITHUB_ACTIONS local CI spoof must not be checked in",
    }
    assignments: dict[str, str] = {}
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        for segment in shell_command_segments_from_tokens(command_tokens(stripped)):
            messages.update(dynamic_env_segment_messages(segment, assignments, target_keys))
            segment_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
            for name, value in segment_assignments.items():
                if is_persistent_assignment:
                    alias_value = shell_assignment_alias_value(value, target_keys)
                    if alias_value is not None:
                        assignments[name] = alias_value
                    else:
                        assignments[name] = shell_assignment_tracking_value(value, target_keys)
    return messages
def github_env_payload_assignments(payload: str, *, decode_newlines: bool = False) -> list[str]:
    if decode_newlines:
        payload = payload.replace("\\n", "\n")
    assignments: list[str] = []
    payload_lines = payload.splitlines() or [payload]
    index = 0
    while index < len(payload_lines):
        line = payload_lines[index]
        clean = line.strip()
        heredoc = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)<<(.+)", clean)
        if heredoc:
            name = heredoc.group(1)
            delimiter = storage_strip_quotes(heredoc.group(2).strip())
            body: list[str] = []
            index += 1
            while index < len(payload_lines):
                candidate = payload_lines[index]
                if candidate.strip() == delimiter:
                    break
                body.append(candidate.strip())
                index += 1
            assignments.append(f"{name}={shlex.quote(storage_strip_quotes(chr(10).join(body)))}")
            if index < len(payload_lines):
                index += 1
            continue
        if "=" not in clean:
            index += 1
            continue
        name, value = clean.split("=", 1)
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            assignments.append(f"{name}={shlex.quote(storage_strip_quotes(value))}")
        index += 1
    return assignments


def github_env_assignments_from_echo_tokens(tokens: list[str]) -> list[str]:
    if len(tokens) < 4 or pathlib.Path(tokens[0]).name != "echo":
        return []
    for redirect_index, token in enumerate(tokens):
        if token != ">>":
            continue
        target = storage_strip_quotes(tokens[redirect_index + 1]) if redirect_index + 1 < len(tokens) else ""
        if target not in {"$GITHUB_ENV", "${GITHUB_ENV}"}:
            continue
        payload_start = 1
        decode_newlines = False
        while payload_start < redirect_index and re.fullmatch(r"-[neE]+", tokens[payload_start]):
            for option in tokens[payload_start][1:]:
                if option == "e":
                    decode_newlines = True
                elif option == "E":
                    decode_newlines = False
            payload_start += 1
        payload = " ".join(tokens[payload_start:redirect_index])
        return github_env_payload_assignments(payload, decode_newlines=decode_newlines)
    return []


def github_env_assignment_from_echo_tokens(tokens: list[str]) -> str | None:
    assignments = github_env_assignments_from_echo_tokens(tokens)
    return assignments[0] if assignments else None


def printf_rendered_payload(format_payload: str, argument_tokens: list[str]) -> str | None:
    chunks: list[str] = []
    argument_index = 0
    while True:
        chunk: list[str] = []
        consumed_argument = False
        index = 0
        while index < len(format_payload):
            if format_payload[index] != "%":
                chunk.append(format_payload[index])
                index += 1
                continue
            if index + 1 >= len(format_payload):
                return None
            conversion = format_payload[index + 1]
            if conversion == "%":
                chunk.append("%")
                index += 2
                continue
            if conversion not in {"s", "b"}:
                return None
            value = argument_tokens[argument_index] if argument_index < len(argument_tokens) else ""
            if argument_index < len(argument_tokens):
                argument_index += 1
            chunk.append(value.replace("\\n", "\n") if conversion == "b" else value)
            consumed_argument = True
            index += 2
        chunks.append("".join(chunk))
        if argument_index >= len(argument_tokens) or not consumed_argument:
            break
    return "".join(chunks)


def github_env_assignments_from_printf_tokens(tokens: list[str]) -> list[str]:
    if len(tokens) < 4 or pathlib.Path(tokens[0]).name != "printf":
        return []
    for redirect_index, token in enumerate(tokens):
        if token != ">>":
            continue
        target = storage_strip_quotes(tokens[redirect_index + 1]) if redirect_index + 1 < len(tokens) else ""
        if target not in {"$GITHUB_ENV", "${GITHUB_ENV}"}:
            continue
        payload_tokens = tokens[1:redirect_index]
        if not payload_tokens:
            return []
        if payload_tokens[0] == "--":
            payload_tokens = payload_tokens[1:]
        if not payload_tokens:
            return []
        format_payload = storage_strip_quotes(payload_tokens[0]).replace("\\n", "\n")
        argument_tokens = [storage_strip_quotes(value) for value in payload_tokens[1:]]
        payload = printf_rendered_payload(format_payload, argument_tokens)
        if payload is None:
            return []
        return github_env_payload_assignments(payload)
    return []


def github_env_assignment_from_printf_tokens(tokens: list[str]) -> str | None:
    assignments = github_env_assignments_from_printf_tokens(tokens)
    return assignments[0] if assignments else None


def github_env_assignments_from_line(line: str) -> list[str]:
    clean = strip_comment(line).strip()
    tokens = command_tokens(clean)
    assignments: list[str] = []
    for segment in shell_command_segments_from_tokens(tokens):
        for extractor in (github_env_assignments_from_echo_tokens, github_env_assignments_from_printf_tokens):
            assignments.extend(extractor(segment))
    return assignments


def github_env_line_assignments_around_cat_heredoc(
    line: str,
) -> tuple[list[str], tuple[str, bool, bool] | None, list[str]]:
    clean = strip_comment(line).strip()
    before: list[str] = []
    after: list[str] = []
    heredoc_spec: tuple[str, bool, bool] | None = None
    for segment in shell_command_segments_from_tokens(command_tokens(clean)):
        spec = github_env_cat_heredoc_spec(segment, clean)
        if spec is not None and heredoc_spec is None:
            heredoc_spec = spec
            continue
        target = after if heredoc_spec is not None else before
        for extractor in (github_env_assignments_from_echo_tokens, github_env_assignments_from_printf_tokens):
            target.extend(extractor(segment))
    return before, heredoc_spec, after


def shell_heredoc_quoted_delimiters(line: str) -> dict[str, bool]:
    delimiters: dict[str, bool] = {}
    for match in re.finditer(r"<<(-?)\s*(['\"]?)([A-Za-z_][A-Za-z0-9_-]*)\2", line):
        delimiters[match.group(3)] = bool(match.group(2))
    return delimiters


def github_env_cat_heredoc_spec(tokens: list[str], line: str) -> tuple[str, bool, bool] | None:
    if len(tokens) < 5 or pathlib.Path(tokens[0]).name != "cat":
        return None
    writes_github_env = any(
        token == ">>"
        and index + 1 < len(tokens)
        and storage_strip_quotes(tokens[index + 1]) in {"$GITHUB_ENV", "${GITHUB_ENV}"}
        for index, token in enumerate(tokens)
    )
    if not writes_github_env:
        return None
    quoted_delimiters = shell_heredoc_quoted_delimiters(line)
    for index, token in enumerate(tokens):
        if token in {"<<", "<<-"} and index + 1 < len(tokens):
            delimiter = storage_strip_quotes(tokens[index + 1])
            return (delimiter, token == "<<-", quoted_delimiters.get(delimiter, False))
    return None


def github_env_assignments_from_cat_heredocs(text: str) -> list[str]:
    assignments: list[str] = []
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        clean = strip_comment(lines[index]).strip()
        heredoc_spec: tuple[str, bool, bool] | None = None
        for segment in shell_command_segments_from_tokens(command_tokens(clean)):
            heredoc_spec = github_env_cat_heredoc_spec(segment, clean)
            if heredoc_spec is not None:
                break
        if heredoc_spec is None:
            index += 1
            continue
        delimiter, strip_tabs, quoted_delimiter = heredoc_spec
        payload: list[str] = []
        index += 1
        while index < len(lines):
            candidate = lines[index]
            comparable = candidate.lstrip("\t") if strip_tabs else candidate
            if comparable == delimiter:
                break
            payload.append(candidate.lstrip("\t") if strip_tabs else candidate)
            index += 1
        payload_text = "\n".join(payload)
        if not quoted_delimiter:
            payload_text = payload_text.replace("\\\n", "")
        assignments.extend(github_env_payload_assignments(payload_text))
        if index < len(lines):
            index += 1
    return assignments


def github_env_assignment_line(line: str) -> str | None:
    assignments = github_env_assignments_from_line(line)
    return assignments[0] if assignments else None


def github_env_assignments_from_logical_text(text: str) -> list[str]:
    assignments: list[str] = []
    for line in shell_logical_lines(text):
        assignments.extend(github_env_assignments_from_line(line))
    return assignments


def github_env_assignment_lines(text: str) -> list[str]:
    assignments: list[str] = []
    pending = ""
    raw_lines = text.splitlines()
    index = 0
    while index < len(raw_lines):
        line = raw_lines[index]
        before, heredoc_spec, after = github_env_line_assignments_around_cat_heredoc(line)
        if heredoc_spec is None:
            pending = f"{pending}\n{line}" if pending else line
            balance_text = "\n".join(strip_comment(pending_line) for pending_line in pending.splitlines())
            if shell_quotes_are_balanced(balance_text) and not line.rstrip().endswith("\\"):
                assignments.extend(github_env_assignments_from_logical_text(pending))
                pending = ""
            index += 1
            continue

        if pending:
            assignments.extend(github_env_assignments_from_logical_text(pending))
            pending = ""
        assignments.extend(before)
        delimiter, strip_tabs, quoted_delimiter = heredoc_spec
        payload: list[str] = []
        index += 1
        while index < len(raw_lines):
            candidate = raw_lines[index]
            comparable = candidate.lstrip("\t") if strip_tabs else candidate
            if comparable == delimiter:
                break
            payload.append(candidate.lstrip("\t") if strip_tabs else candidate)
            index += 1
        payload_text = "\n".join(payload)
        if not quoted_delimiter:
            payload_text = payload_text.replace("\\\n", "")
        assignments.extend(github_env_payload_assignments(payload_text))
        assignments.extend(after)
        if index < len(raw_lines):
            index += 1
    if pending:
        assignments.extend(github_env_assignments_from_logical_text(pending))
    return assignments
