#!/usr/bin/env python3
"""Publish one bound Claude deliverable from the completed action output."""

from __future__ import annotations

import argparse
import pathlib
import sys
import tomllib

from direct_ai_review import (
    EvidenceError,
    config_int,
    config_table,
    config_text,
    publish_claude_execution,
    required_env,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execution-file", required=True, type=pathlib.Path)
    parser.add_argument("--config-file", required=True, type=pathlib.Path)
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    runtime = tomllib.loads(args.config_file.read_text(encoding="utf-8"))
    publish_claude_execution(
        args.execution_file,
        provider_config=config_table(runtime, "claude"),
        review_config=config_table(runtime, "review"),
        repo=required_env("GITHUB_REPOSITORY"),
        pr_number=required_env("PR_NUMBER"),
        head_sha=required_env("PR_HEAD_SHA"),
        token=required_env("GITHUB_TOKEN"),
        api_url=config_text(config_table(runtime, "github"), "api_url"),
        api_timeout_seconds=config_int(config_table(runtime, "github"), "comment_timeout_seconds"),
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except EvidenceError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(1) from None
