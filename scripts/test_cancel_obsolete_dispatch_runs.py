#!/usr/bin/env python3
"""Self-tests for cancel_obsolete_dispatch_runs.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "cancel_obsolete_dispatch_runs.py"


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("cancel_obsolete_dispatch_runs", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load cancel_obsolete_dispatch_runs.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class FakeClient:
    def __init__(self, pages, *, runs_by_id=None, conflict_ids=None):
        self.pages = list(pages)
        self.runs_by_id = dict(runs_by_id or {})
        self.conflict_ids = set(conflict_ids or [])
        self.calls = []
        self.cancelled = []

    def get_json(self, path, params):
        self.calls.append((path, dict(params)))
        if path.startswith("actions/runs/"):
            run_id = int(path.rsplit("/", 1)[-1])
            return self.runs_by_id.get(run_id, run_payload(run_id))
        page = int(params["page"])
        return {"workflow_runs": self.pages[page - 1] if page <= len(self.pages) else []}

    def cancel_run(self, run_id):
        if run_id in self.conflict_ids:
            return "conflict"
        self.cancelled.append(run_id)
        return "cancelled"


def config(module, **overrides):
    values = {
        "workflow_name": "CI",
        "workflow_path": ".github/workflows/ci.yml",
        "workflow_event": "workflow_dispatch",
        "run_name_iteration": "CI [dispatch:iteration]",
        "active_statuses": frozenset({"queued", "requested", "waiting", "pending", "in_progress"}),
        "workflow_runs_per_page": 100,
        "max_pages": 2,
    }
    values.update(overrides)
    return module.DispatchCancelConfig(**values)


def run_payload(run_id, **overrides):
    payload = {
        "id": run_id,
        "name": "CI [dispatch:iteration]",
        "display_title": "CI [dispatch:iteration]",
        "path": ".github/workflows/ci.yml",
        "event": "workflow_dispatch",
        "head_branch": "feature/cost",
        "status": "in_progress",
        "conclusion": None,
        "created_at": "2026-06-15T08:00:00Z",
    }
    payload.update(overrides)
    return payload


def event_payload(**overrides):
    run = run_payload(200, created_at="2026-06-15T08:20:42Z", status="requested")
    run.update(overrides)
    return {"workflow_run": run}


def assert_cancels_only_older_active_same_branch_dispatch_runs() -> None:
    module = load_script()
    current = run_payload(200, created_at="2026-06-15T08:20:42Z", status="requested")
    fake = FakeClient(
        [
            [
                run_payload(100, created_at="2026-06-15T07:59:00Z"),
                run_payload(
                    107,
                    name="CI [dispatch:iteration]",
                    display_title="CI [dispatch:iteration]",
                    created_at="2026-06-15T07:59:00Z",
                ),
                run_payload(101, head_branch="other", created_at="2026-06-15T07:59:00Z"),
                run_payload(102, event="pull_request", created_at="2026-06-15T07:59:00Z"),
                run_payload(
                    103,
                    status="completed",
                    conclusion="success",
                    created_at="2026-06-15T07:59:00Z",
                ),
                run_payload(
                    104,
                    name="Backtester CI",
                    display_title="Backtester CI",
                    created_at="2026-06-15T07:59:00Z",
                ),
                run_payload(
                    105,
                    path=".github/workflows/backtester-ci.yml",
                    created_at="2026-06-15T07:59:00Z",
                ),
                run_payload(106, path="", created_at="2026-06-15T07:59:00Z"),
                run_payload(201, created_at="2026-06-15T08:21:00Z"),
                run_payload(200, created_at="2026-06-15T08:20:42Z"),
            ]
        ],
        runs_by_id={200: current},
    )
    summary = module.handle_payload(event_payload(), config=config(module), client=fake, dry_run=False)
    assert summary["obsolete_run_ids"] == [100, 107], summary
    assert summary["cancelled_run_ids"] == [100, 107], summary
    assert fake.cancelled == [100, 107], fake.cancelled
    assert fake.calls[0] == ("actions/runs/200", {})
    assert fake.calls[1] == (
        "actions/runs",
        {
            "branch": "feature/cost",
            "event": "workflow_dispatch",
            "per_page": "100",
            "page": "1",
        },
    )


def assert_cancels_same_second_lower_id_dispatch_runs() -> None:
    module = load_script()
    current = run_payload(200, created_at="2026-06-15T08:20:42Z", status="requested")
    fake = FakeClient(
        [
            [
                run_payload(199, created_at="2026-06-15T08:20:42Z"),
                run_payload(200, created_at="2026-06-15T08:20:42Z"),
                run_payload(201, created_at="2026-06-15T08:20:42Z"),
            ]
        ],
        runs_by_id={200: current},
    )
    summary = module.handle_payload(event_payload(), config=config(module), client=fake, dry_run=False)
    assert summary["obsolete_run_ids"] == [199], summary
    assert summary["cancelled_run_ids"] == [199], summary
    assert fake.cancelled == [199], fake.cancelled


def assert_ignores_non_dispatch_and_branchless_runs() -> None:
    module = load_script()
    fake = FakeClient([[run_payload(100)]])
    ignored = module.handle_payload(
        event_payload(event="pull_request"), config=config(module), client=fake, dry_run=False
    )
    assert ignored == {"ignored": True, "reason": "not configured workflow event"}, ignored
    assert fake.calls == [], fake.calls

    branchless = module.handle_payload(
        event_payload(head_branch=""), config=config(module), client=fake, dry_run=False
    )
    assert branchless == {"ignored": True, "reason": "workflow run has no branch"}, branchless
    assert fake.calls == [], fake.calls

    missing_path = module.handle_payload(
        event_payload(path=""), config=config(module), client=fake, dry_run=False
    )
    assert missing_path == {"ignored": True, "reason": "not configured workflow path"}, missing_path
    assert fake.calls == [], fake.calls


def assert_missing_or_unknown_current_marker_skips_cancellation() -> None:
    module = load_script()
    fake = FakeClient(
        [[run_payload(100, created_at="2026-06-15T07:59:00Z")]],
        runs_by_id={200: run_payload(200, display_title="CI", created_at="2026-06-15T08:20:42Z")},
    )
    summary = module.handle_payload(event_payload(), config=config(module), client=fake, dry_run=False)
    assert summary == {"ignored": True, "reason": "current dispatch run has no configured class marker"}, summary
    assert fake.cancelled == [], fake.cancelled


def assert_run_display_title_prefers_camel_case() -> None:
    module = load_script()
    title = module.run_display_title(
        run_payload(
            200,
            display_title="CI [dispatch:iteration]",
            displayTitle="CI [dispatch:iteration]",
        )
    )
    assert title == "CI [dispatch:iteration]", title


def assert_current_rehydrate_failure_skips_cancellation() -> None:
    module = load_script()

    class FailingRehydrateClient(FakeClient):
        def get_json(self, path, params):
            if path.startswith("actions/runs/"):
                raise module.DispatchCancelError("GitHub API GET actions/runs/200 network error: dns failed")
            return super().get_json(path, params)

    fake = FailingRehydrateClient([[run_payload(100, created_at="2026-06-15T07:59:00Z")]])
    summary = module.handle_payload(event_payload(), config=config(module), client=fake, dry_run=False)
    assert summary == {"ignored": True, "reason": "could not rehydrate current workflow run"}, summary
    assert fake.cancelled == [], fake.cancelled


def assert_dry_run_reports_without_cancelling() -> None:
    module = load_script()
    fake = FakeClient([[run_payload(100)]])
    summary = module.handle_payload(event_payload(), config=config(module), client=fake, dry_run=True)
    assert summary["obsolete_run_ids"] == [100], summary
    assert summary["cancelled_run_ids"] == [], summary
    assert fake.cancelled == [], fake.cancelled


def assert_cancel_conflict_is_recorded_not_failed() -> None:
    module = load_script()
    fake = FakeClient([[run_payload(100)]], conflict_ids={100})
    summary = module.handle_payload(event_payload(), config=config(module), client=fake, dry_run=False)
    assert summary["cancelled_run_ids"] == [], summary
    assert summary["conflict_run_ids"] == [100], summary


def assert_terminal_cancel_http_errors_are_conflicts() -> None:
    module = load_script()
    for code in (409, 404, 422):
        client = module.GitHubClient(repo="example/repo", token="token")

        def raise_terminal(_method, _path, *, params):
            raise module.GitHubApiError(method="POST", path="actions/runs/100/cancel", code=code, body="")

        client._request_json = raise_terminal
        assert client.cancel_run(100) == "conflict"


def assert_cancel_uses_force_cancel_endpoint() -> None:
    module = load_script()
    calls = []
    client = module.GitHubClient(repo="example/repo", token="token")

    def record_request(method, path, *, params):
        calls.append((method, path, params))
        return {}

    client._request_json = record_request
    assert client.cancel_run(100) == "cancelled"
    assert calls == [("POST", "actions/runs/100/force-cancel", {})], calls


def assert_paginates_until_partial_page() -> None:
    module = load_script()
    fake = FakeClient(
        [
            [run_payload(100), run_payload(101)],
            [run_payload(102)],
            [run_payload(103)],
        ]
    )
    summary = module.handle_payload(
        event_payload(),
        config=config(module, workflow_runs_per_page=2, max_pages=3),
        client=fake,
        dry_run=False,
    )
    assert summary["obsolete_run_ids"] == [100, 101, 102], summary
    assert [call[1]["page"] for call in fake.calls if call[0] == "actions/runs"] == ["1", "2"], fake.calls


def assert_warns_when_pagination_cap_is_full() -> None:
    module = load_script()
    fake = FakeClient(
        [
            [run_payload(100), run_payload(101)],
            [run_payload(102), run_payload(103)],
        ]
    )
    stderr = io.StringIO()
    with contextlib.redirect_stderr(stderr):
        summary = module.handle_payload(
            event_payload(),
            config=config(module, workflow_runs_per_page=2, max_pages=2),
            client=fake,
            dry_run=False,
        )
    assert summary["obsolete_run_ids"] == [100, 101, 102, 103], summary
    assert "max_pages=2" in stderr.getvalue(), stderr.getvalue()
    assert [call[1]["page"] for call in fake.calls if call[0] == "actions/runs"] == ["1", "2"], fake.calls


def assert_config_comes_from_toml() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        path.write_text(
            """
[ci_provenance]
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"

[ci_provenance.dispatch]
run_name_iteration = "CI [dispatch:iteration]"

[dispatch_cancel]
workflow_event = "workflow_dispatch"
active_statuses = ["queued", "in_progress"]
workflow_runs_per_page = 37
max_pages = 4
""".strip(),
            encoding="utf-8",
        )
        loaded = module.load_config(path)
    assert loaded.workflow_name == "CI", loaded
    assert loaded.workflow_path == ".github/workflows/ci.yml", loaded
    assert loaded.workflow_event == "workflow_dispatch", loaded
    assert loaded.run_name_iteration == "CI [dispatch:iteration]", loaded
    assert loaded.active_statuses == frozenset({"queued", "in_progress"}), loaded
    assert loaded.workflow_runs_per_page == 37, loaded
    assert loaded.max_pages == 4, loaded


def assert_github_client_uses_safe_redirect_opener() -> None:
    module = load_script()
    import ci_provenance

    captured_handlers = []
    original_build_opener = module.urllib.request.build_opener
    original_urlopen = module.urllib.request.urlopen

    class Response:
        def __enter__(self):
            return self

        def __exit__(self, _exc_type, _exc, _traceback):
            return False

        def read(self):
            return b"{}"

    class FakeOpener:
        def open(self, _request, timeout):
            if timeout != 30:
                raise AssertionError(f"unexpected timeout: {timeout!r}")
            return Response()

    def build_opener(*handlers):
        captured_handlers.extend(handlers)
        return FakeOpener()

    def direct_urlopen(_request, timeout):
        if timeout != 30:
            raise AssertionError(f"unexpected timeout: {timeout!r}")
        return Response()

    module.urllib.request.build_opener = build_opener
    module.urllib.request.urlopen = direct_urlopen
    try:
        client = module.GitHubClient(repo="example/repo", token="token")
        client.get_json("actions/runs", {})
    finally:
        module.urllib.request.build_opener = original_build_opener
        module.urllib.request.urlopen = original_urlopen

    if not any(isinstance(handler, ci_provenance.SafeGitHubRedirectHandler) for handler in captured_handlers):
        raise AssertionError("GitHub client must use SafeGitHubRedirectHandler")


def assert_api_transport_errors_are_domain_errors() -> None:
    module = load_script()
    original_open_github_api_request = module._ci_provenance.open_github_api_request

    def raise_url_error(_request, timeout):
        raise module.urllib.error.URLError("dns failed")

    module._ci_provenance.open_github_api_request = raise_url_error
    try:
        client = module.GitHubClient(repo="example/repo", token="token")
        try:
            client.get_json("actions/runs", {})
        except module.DispatchCancelError as exc:
            assert "network error" in str(exc), exc
            assert "token" not in str(exc), exc
        else:
            raise AssertionError("expected DispatchCancelError")
    finally:
        module._ci_provenance.open_github_api_request = original_open_github_api_request


def assert_invalid_json_is_domain_error() -> None:
    module = load_script()
    original_open_github_api_request = module._ci_provenance.open_github_api_request

    class Response:
        def __enter__(self):
            return self

        def __exit__(self, _exc_type, _exc, _traceback):
            return False

        def read(self):
            return b"{not-json"

    def invalid_json_response(_request, timeout):
        return Response()

    module._ci_provenance.open_github_api_request = invalid_json_response
    try:
        client = module.GitHubClient(repo="example/repo", token="token")
        try:
            client.get_json("actions/runs", {})
        except module.DispatchCancelError as exc:
            assert "invalid JSON" in str(exc), exc
            assert "token" not in str(exc), exc
        else:
            raise AssertionError("expected DispatchCancelError")
    finally:
        module._ci_provenance.open_github_api_request = original_open_github_api_request


def assert_invalid_event_file_json_is_domain_error() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        event_path = pathlib.Path(tmpdir) / "event.json"
        event_path.write_text("{not-json", encoding="utf-8")
        try:
            module.main(["--event-path", str(event_path), "--repo", "example/repo", "--dry-run"])
        except module.DispatchCancelError as exc:
            assert "event payload is invalid JSON" in str(exc), exc
        else:
            raise AssertionError("expected DispatchCancelError")


def main() -> int:
    assert_cancels_only_older_active_same_branch_dispatch_runs()
    assert_cancels_same_second_lower_id_dispatch_runs()
    assert_ignores_non_dispatch_and_branchless_runs()
    assert_missing_or_unknown_current_marker_skips_cancellation()
    assert_run_display_title_prefers_camel_case()
    assert_current_rehydrate_failure_skips_cancellation()
    assert_dry_run_reports_without_cancelling()
    assert_cancel_conflict_is_recorded_not_failed()
    assert_terminal_cancel_http_errors_are_conflicts()
    assert_cancel_uses_force_cancel_endpoint()
    assert_paginates_until_partial_page()
    assert_warns_when_pagination_cap_is_full()
    assert_config_comes_from_toml()
    assert_github_client_uses_safe_redirect_opener()
    assert_api_transport_errors_are_domain_errors()
    assert_invalid_json_is_domain_error()
    assert_invalid_event_file_json_is_domain_error()
    print("ok")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
