#!/usr/bin/env python3
"""Self-tests for the governed sccache eligibility resolver."""

from __future__ import annotations

from sccache_eligibility import resolve_sccache_eligibility


LOCATION = {
    "bucket": "bolt-v2-ci-cache-675819144420-us-east-2",
    "region": "us-east-2",
    "key_prefix": "sccache/bolt-v2/arm64/root-nextest/",
}


def assert_case(
    label: str,
    *,
    active: bool = True,
    event_name: str,
    github_ref: str,
    read_role_arn: str = "arn:aws:iam::123456789012:role/read",
    write_role_arn: str = "arn:aws:iam::123456789012:role/write",
    location: dict[str, object] | None = None,
    expected_eligible: bool,
    expected_role: str,
    expected_mode: str,
) -> None:
    result = resolve_sccache_eligibility(
        active=active,
        event_name=event_name,
        github_ref=github_ref,
        read_role_arn=read_role_arn,
        write_role_arn=write_role_arn,
        location=LOCATION if location is None else location,
    )
    actual = (result.eligible, result.role_arn, result.cache_mode)
    expected = (expected_eligible, expected_role, expected_mode)
    if actual != expected:
        raise AssertionError(f"{label}: expected {expected!r}, got {actual!r}")


def main() -> int:
    read_role = "arn:aws:iam::123456789012:role/read"
    write_role = "arn:aws:iam::123456789012:role/write"
    assert_case(
        "main push may write",
        event_name="push",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=write_role,
        expected_mode="read_write",
    )
    assert_case(
        "main dispatch may write",
        event_name="workflow_dispatch",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=write_role,
        expected_mode="read_write",
    )
    assert_case(
        "pull request reads only",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "merge group reads only",
        event_name="merge_group",
        github_ref="refs/heads/gh-readonly-queue/main/pr-1302",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "schedule reads only",
        event_name="schedule",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "tag push gets no role",
        event_name="push",
        github_ref="refs/tags/v1.2.3",
        expected_eligible=False,
        expected_role="",
        expected_mode="none",
    )
    assert_case(
        "inactive compiles without cache",
        active=False,
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing read role fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        read_role_arn="",
        expected_eligible=False,
        expected_role="",
        expected_mode="read_only",
    )
    assert_case(
        "invalid prefix fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "key_prefix": "sccache/bolt-v2"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "location newlines are rejected",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "bucket": "bucket\nSCCACHE_REGION=evil"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    print("OK: sccache eligibility self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
