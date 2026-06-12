#!/usr/bin/env python3
"""Report GitHub Actions runner-minutes by workflow, job, and configured tier."""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import pathlib
import subprocess
import sys
import tomllib
import urllib.parse


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_RUNNER_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"


class MeterError(RuntimeError):
    """Raised when runner-minute input data is missing or malformed."""


@dataclasses.dataclass(frozen=True)
class RunnerConfig:
    label_to_tier: dict[str, str]
    workflow_keys: set[str]
    fingerprint_artifact_prefix: str
    fingerprint_workflow_key: str
    debug_workflow_key: str


def as_text(value: object) -> str:
    return "" if value is None else str(value)


def parse_time(value: object) -> dt.datetime | None:
    text = as_text(value)
    if not text:
        return None
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = dt.datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=dt.UTC)
    return parsed.astimezone(dt.UTC)


def isoformat_utc(value: dt.datetime) -> str:
    return value.astimezone(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def workflow_key_for_path(path: object) -> str:
    stem = pathlib.PurePosixPath(as_text(path)).stem
    return stem.replace("-", "_")


def load_runner_config(path: pathlib.Path = DEFAULT_RUNNER_CONFIG) -> RunnerConfig:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise MeterError(f"runner config could not be read: {path}: {exc}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise MeterError(f"runner config is invalid TOML: {path}: {exc}") from exc

    runners = data.get("runners")
    workflows = data.get("workflows")
    if not isinstance(runners, dict) or not isinstance(workflows, dict):
        raise MeterError("runner config must define [runners] and [workflows]")

    label_to_tier: dict[str, str] = {}
    for tier, runner in runners.items():
        if not isinstance(runner, dict):
            raise MeterError(f"runners.{tier} must be a table")
        label = runner.get("label")
        if not isinstance(label, str) or not label:
            raise MeterError(f"runners.{tier}.label must be a non-empty string")
        label_to_tier[label] = tier

    meter = data.get("meter")
    if not isinstance(meter, dict):
        raise MeterError("meter config must be a table")
    prefix = meter.get("fingerprint_artifact_prefix")
    if not isinstance(prefix, str) or not prefix:
        raise MeterError("meter.fingerprint_artifact_prefix must be a non-empty string")
    fingerprint_workflow_key = meter.get("fingerprint_workflow")
    if not isinstance(fingerprint_workflow_key, str) or not fingerprint_workflow_key:
        raise MeterError("meter.fingerprint_workflow must be a non-empty string")
    debug_workflow_key = meter.get("debug_workflow")
    if not isinstance(debug_workflow_key, str) or not debug_workflow_key:
        raise MeterError("meter.debug_workflow must be a non-empty string")
    configured_workflow_keys = {key for key, table in workflows.items() if isinstance(table, dict)}
    included = meter.get("included_workflows")
    if not isinstance(included, list) or not all(isinstance(key, str) and key for key in included):
        raise MeterError("meter.included_workflows must be a non-empty string list")
    workflow_keys = set(included)
    unknown_workflows = sorted(workflow_keys - configured_workflow_keys)
    if unknown_workflows:
        raise MeterError(f"meter.included_workflows references unknown workflows: {', '.join(unknown_workflows)}")
    for role_name, workflow_key in (
        ("fingerprint_workflow", fingerprint_workflow_key),
        ("debug_workflow", debug_workflow_key),
    ):
        if workflow_key not in configured_workflow_keys:
            raise MeterError(f"meter.{role_name} references unknown workflow: {workflow_key}")

    return RunnerConfig(
        label_to_tier=label_to_tier,
        workflow_keys=workflow_keys,
        fingerprint_artifact_prefix=prefix,
        fingerprint_workflow_key=fingerprint_workflow_key,
        debug_workflow_key=debug_workflow_key,
    )


def job_minutes(job: dict[str, object]) -> float:
    started = parse_time(job.get("started_at"))
    completed = parse_time(job.get("completed_at"))
    if started is None or completed is None or completed < started:
        return 0.0
    return round((completed - started).total_seconds() / 60.0, 3)


def job_runner_label(job: dict[str, object], config: RunnerConfig) -> str:
    labels = job.get("labels")
    if not isinstance(labels, list):
        return ""
    text_labels = [as_text(label) for label in labels if as_text(label)]
    for label in text_labels:
        if label in config.label_to_tier:
            return label
    return text_labels[0] if text_labels else ""


def extract_fingerprint(artifacts_payload: dict[str, object], prefix: str) -> str | None:
    artifacts = artifacts_payload.get("artifacts")
    if not isinstance(artifacts, list):
        return None
    matches = [
        as_text(artifact.get("name"))[len(prefix) :]
        for artifact in artifacts
        if isinstance(artifact, dict) and as_text(artifact.get("name")).startswith(prefix)
    ]
    valid_matches = sorted(match for match in matches if match)
    return valid_matches[0] if valid_matches else None


def run_sort_key(run: dict[str, object]) -> tuple[dt.datetime, int]:
    created = parse_time(run.get("created_at")) or dt.datetime.min.replace(tzinfo=dt.UTC)
    try:
        run_id = int(as_text(run.get("id")))
    except ValueError:
        run_id = 0
    return (created, run_id)


def lookup_by_run_id(mapping: dict[int | str, dict[str, object]], run_id: object) -> dict[str, object] | None:
    run_id_text = as_text(run_id)
    entry = mapping.get(run_id_text)
    if entry is not None:
        return entry
    if run_id_text.isdecimal():
        return mapping.get(int(run_id_text))
    return None


def run_is_newer_same_pr_workflow(
    candidate: dict[str, object],
    run: dict[str, object],
    candidate_pr_state: dict[str, object] | None,
    run_pr_state: dict[str, object] | None,
) -> bool:
    if as_text(run.get("event")) != "pull_request" or as_text(candidate.get("event")) != "pull_request":
        return False
    if workflow_key_for_path(candidate.get("path")) != workflow_key_for_path(run.get("path")):
        return False
    if not candidate_pr_state or not run_pr_state:
        return False
    if candidate_pr_state.get("number") != run_pr_state.get("number"):
        return False
    candidate_created = parse_time(candidate.get("created_at"))
    run_created = parse_time(run.get("created_at"))
    run_updated = parse_time(run.get("updated_at"))
    if candidate_created is None or run_created is None or run_updated is None:
        return False
    return run_created < candidate_created <= run_updated


def base_classifications(
    run: dict[str, object],
    all_runs: list[dict[str, object]],
    pr_state: dict[str, object] | None,
    pr_state_by_run_id: dict[int | str, dict[str, object]],
    fingerprint: str | None,
    prior_green_fingerprints: set[str],
    fingerprint_workflow_key: str,
) -> list[str]:
    conclusion = as_text(run.get("conclusion"))
    classifications: list[str] = []
    if conclusion == "success":
        classifications.append("completed-green")
    elif conclusion == "cancelled" and any(
        run_is_newer_same_pr_workflow(
            candidate,
            run,
            lookup_by_run_id(pr_state_by_run_id, candidate.get("id")),
            pr_state,
        )
        for candidate in all_runs
    ):
        classifications.append("cancelled-superseded")
    elif conclusion == "cancelled":
        classifications.append("cancelled")
    elif conclusion:
        classifications.append("failed")
    else:
        classifications.append("incomplete")

    if pr_state and pr_state.get("draft_at_run") is True:
        classifications.append("draft-stage")
    if fingerprint and fingerprint in prior_green_fingerprints:
        classifications.append("fingerprint-identical")
    if workflow_key_for_path(run.get("path")) == fingerprint_workflow_key and fingerprint is None:
        classifications.append("fingerprint-unknown")
    return classifications


def add_tier_total(target: dict[str, dict[str, float]], tier: str, minutes: float) -> None:
    entry = target.setdefault(tier, {"minutes": 0.0})
    entry["minutes"] = round(entry["minutes"] + minutes, 3)


def build_report(
    *,
    repo: str,
    runs_payload: dict[str, object],
    jobs_payload_by_run_id: dict[int | str, dict[str, object]],
    artifacts_payload_by_run_id: dict[int | str, dict[str, object]],
    pr_state_by_run_id: dict[int | str, dict[str, object]],
    runner_config: RunnerConfig,
    generated_at: str,
) -> dict[str, object]:
    raw_runs = runs_payload.get("workflow_runs")
    if not isinstance(raw_runs, list):
        raise MeterError("workflow runs payload is malformed")
    all_runs = [run for run in raw_runs if isinstance(run, dict)]
    sorted_runs = sorted(all_runs, key=run_sort_key)

    prior_green_fingerprints: set[str] = set()
    report_runs: list[dict[str, object]] = []
    totals_by_tier: dict[str, dict[str, float]] = {}
    debug_sessions: list[dict[str, object]] = []

    for run in sorted_runs:
        run_id = as_text(run.get("id"))
        workflow_key = workflow_key_for_path(run.get("path"))
        jobs_payload = lookup_by_run_id(jobs_payload_by_run_id, run_id)
        artifacts_payload = lookup_by_run_id(artifacts_payload_by_run_id, run_id)
        if jobs_payload is None:
            raise MeterError(f"run {run_id} jobs payload is missing")
        if artifacts_payload is None:
            raise MeterError(f"run {run_id} artifacts payload is missing")

        jobs = jobs_payload.get("jobs")
        if not isinstance(jobs, list):
            raise MeterError(f"run {run_id} jobs payload is malformed")
        fingerprint = extract_fingerprint(artifacts_payload, runner_config.fingerprint_artifact_prefix)
        pr_state = lookup_by_run_id(pr_state_by_run_id, run_id)
        classifications = base_classifications(
            run,
            all_runs,
            pr_state if isinstance(pr_state, dict) else None,
            pr_state_by_run_id,
            fingerprint,
            prior_green_fingerprints,
            runner_config.fingerprint_workflow_key,
        )

        run_totals: dict[str, dict[str, float]] = {}
        report_jobs: list[dict[str, object]] = []
        for job in jobs:
            if not isinstance(job, dict):
                continue
            label = job_runner_label(job, runner_config)
            tier = runner_config.label_to_tier.get(label, "unknown" if label else "unassigned")
            minutes = job_minutes(job)
            add_tier_total(run_totals, tier, minutes)
            add_tier_total(totals_by_tier, tier, minutes)
            report_jobs.append(
                {
                    "name": as_text(job.get("name")),
                    "tier": tier,
                    "runner_label": label,
                    "minutes": minutes,
                    "status": as_text(job.get("status")),
                    "conclusion": as_text(job.get("conclusion")),
                    "started_at": as_text(job.get("started_at")),
                    "completed_at": as_text(job.get("completed_at")),
                }
            )

        report_run = {
            "id": int(run_id) if run_id.isdecimal() else run_id,
            "workflow_name": as_text(run.get("name")),
            "workflow_key": workflow_key,
            "workflow_path": as_text(run.get("path")),
            "event": as_text(run.get("event")),
            "head_branch": as_text(run.get("head_branch")),
            "head_sha": as_text(run.get("head_sha")),
            "status": as_text(run.get("status")),
            "conclusion": as_text(run.get("conclusion")),
            "created_at": as_text(run.get("created_at")),
            "updated_at": as_text(run.get("updated_at")),
            "url": as_text(run.get("html_url") or run.get("url")),
            "classifications": classifications,
            "fingerprint": fingerprint,
            "pull_request": pr_state if isinstance(pr_state, dict) else None,
            "totals_by_tier": run_totals,
            "jobs": report_jobs,
        }
        report_runs.append(report_run)
        if workflow_key == runner_config.debug_workflow_key:
            debug_sessions.append(report_run)
        if fingerprint and as_text(run.get("conclusion")) == "success":
            prior_green_fingerprints.add(fingerprint)

    return {
        "repo": repo,
        "generated_at": generated_at,
        "workflow_keys": sorted(runner_config.workflow_keys),
        "totals_by_tier": totals_by_tier,
        "runs": report_runs,
        "debug_sessions": debug_sessions,
        "notes": [
            "Runner-minutes are wall-clock job durations from GitHub Actions job timestamps.",
            "Fingerprint-identical classification is only available for runs with a fingerprint artifact.",
            "Cancelled-superseded is inferred from newer same-branch same-workflow pull_request runs.",
        ],
    }


class GhClient:
    def __init__(self, repo: str) -> None:
        self.repo = repo

    def api(self, path: str, *, params: dict[str, str] | None = None, paginate: bool = False) -> object:
        cmd = ["gh", "api"]
        if paginate:
            cmd.extend(["--paginate", "--slurp"])
        cmd.extend(["--method", "GET", f"repos/{self.repo}/{path}"])
        for key, value in (params or {}).items():
            cmd.extend(["-f", f"{key}={value}"])
        result = subprocess.run(cmd, text=True, capture_output=True, check=False)
        if result.returncode != 0:
            raise MeterError(result.stderr.strip() or f"gh api failed for {path}")
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise MeterError(f"gh api returned invalid JSON for {path}: {exc}") from exc
        if paginate and isinstance(payload, list):
            merged: dict[str, object] = {}
            for page in payload:
                if not isinstance(page, dict):
                    continue
                for key, value in page.items():
                    if isinstance(value, list):
                        merged.setdefault(key, [])
                        assert isinstance(merged[key], list)
                        merged[key].extend(value)
                    else:
                        merged[key] = value
            return merged
        return payload

    def graphql(self, query: str, fields: dict[str, str | int]) -> dict[str, object]:
        cmd = ["gh", "api", "graphql", "-f", f"query={query}"]
        for key, value in fields.items():
            flag = "-F" if isinstance(value, int) else "-f"
            cmd.extend([flag, f"{key}={value}"])
        result = subprocess.run(cmd, text=True, capture_output=True, check=False)
        if result.returncode != 0:
            raise MeterError(result.stderr.strip() or "gh graphql request failed")
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise MeterError(f"gh graphql returned invalid JSON: {exc}") from exc
        if not isinstance(payload, dict):
            raise MeterError("gh graphql payload is not an object")
        return payload


def infer_repo() -> str:
    result = subprocess.run(
        ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise MeterError("could not infer repo; pass --repo OWNER/REPO")
    repo = result.stdout.strip()
    if "/" not in repo:
        raise MeterError("could not infer repo; pass --repo OWNER/REPO")
    return repo


def configured_workflow_paths(client: GhClient, config: RunnerConfig) -> set[str]:
    payload = client.api("actions/workflows", paginate=True)
    workflows = payload.get("workflows")
    if not isinstance(workflows, list):
        raise MeterError("workflow list payload is malformed")
    paths: set[str] = set()
    for workflow in workflows:
        if not isinstance(workflow, dict):
            continue
        path = as_text(workflow.get("path"))
        if workflow_key_for_path(path) in config.workflow_keys:
            paths.add(path)
    return paths


def fetch_runs(
    client: GhClient,
    config: RunnerConfig,
    run_ids: list[str],
    days: int | None,
    limit: int | None,
) -> dict[str, object]:
    if run_ids:
        return {"workflow_runs": [client.api(f"actions/runs/{run_id}") for run_id in run_ids]}
    if days is None:
        raise MeterError("pass at least one --run-id or a --days lookback")
    since = dt.datetime.now(dt.UTC) - dt.timedelta(days=days)
    payload = client.api(
        "actions/runs",
        params={"per_page": "100", "created": f">={isoformat_utc(since)}"},
        paginate=True,
    )
    workflow_paths = configured_workflow_paths(client, config)
    runs = payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise MeterError("workflow runs payload is malformed")
    filtered = [run for run in runs if isinstance(run, dict) and as_text(run.get("path")) in workflow_paths]
    if limit is not None:
        filtered = filtered[:limit]
    return {"workflow_runs": filtered}


def fetch_jobs_and_artifacts(client: GhClient, runs_payload: dict[str, object]) -> tuple[dict[str, dict[str, object]], dict[str, dict[str, object]]]:
    runs = runs_payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise MeterError("workflow runs payload is malformed")
    jobs: dict[str, dict[str, object]] = {}
    artifacts: dict[str, dict[str, object]] = {}
    for run in runs:
        if not isinstance(run, dict):
            continue
        run_id = as_text(run.get("id"))
        jobs[run_id] = client.api(f"actions/runs/{run_id}/jobs", params={"per_page": "100"}, paginate=True)
        artifacts[run_id] = client.api(f"actions/runs/{run_id}/artifacts", params={"per_page": "100"}, paginate=True)
    return jobs, artifacts


def pull_number_from_run(run: dict[str, object]) -> int | None:
    pull_refs = run.get("pull_requests")
    if not isinstance(pull_refs, list):
        return None
    for pull_ref in pull_refs:
        if not isinstance(pull_ref, dict):
            continue
        number = pull_ref.get("number")
        if isinstance(number, int):
            return number
    return None


def head_owner_for_run(run: dict[str, object], default_owner: str) -> str:
    head_repository = run.get("head_repository")
    if not isinstance(head_repository, dict):
        return default_owner
    owner = head_repository.get("owner")
    if isinstance(owner, dict):
        login = as_text(owner.get("login"))
        if login:
            return login
    full_name = as_text(head_repository.get("full_name"))
    if "/" in full_name:
        return full_name.split("/", 1)[0]
    return default_owner


def select_pull_request_for_run(run: dict[str, object], pulls_payload: list[dict[str, object]]) -> dict[str, object] | None:
    run_created = parse_time(run.get("created_at"))
    if run_created is None:
        return pulls_payload[0] if pulls_payload else None
    candidates: list[dict[str, object]] = []
    for pull in pulls_payload:
        if not isinstance(pull, dict):
            continue
        created = parse_time(pull.get("created_at"))
        closed = parse_time(pull.get("closed_at"))
        if created and created > run_created:
            continue
        if closed and run_created > closed:
            continue
        candidates.append(pull)
    return sorted(candidates or pulls_payload, key=lambda pull: as_text(pull.get("updated_at")), reverse=True)[0] if pulls_payload else None


def draft_state_at_run(run_time: dt.datetime, pull: dict[str, object], timeline_nodes: list[dict[str, object]]) -> bool | None:
    events = sorted(
        (
            (parse_time(node.get("createdAt")), as_text(node.get("__typename")))
            for node in timeline_nodes
            if isinstance(node, dict)
        ),
        key=lambda item: item[0] or dt.datetime.max.replace(tzinfo=dt.UTC),
    )
    if events and events[0][1] == "ReadyForReviewEvent":
        is_draft = True
    elif events:
        is_draft = False
    else:
        current = pull.get("draft")
        return bool(current) if isinstance(current, bool) else None

    for created, typename in events:
        if created is None or created > run_time:
            break
        if typename == "ConvertToDraftEvent":
            is_draft = True
        elif typename == "ReadyForReviewEvent":
            is_draft = False
    return is_draft


def resolve_pr_states(client: GhClient, repo: str, runs_payload: dict[str, object]) -> dict[str, dict[str, object]]:
    owner, repo_name = repo.split("/", 1)
    runs = runs_payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise MeterError("workflow runs payload is malformed")
    states: dict[str, dict[str, object]] = {}
    pr_cache: dict[int, dict[str, object] | None] = {}
    branch_pull_cache: dict[str, list[dict[str, object]]] = {}
    timeline_cache: dict[int, list[dict[str, object]]] = {}
    query = """
query($owner:String!,$repo:String!,$number:Int!){
  repository(owner:$owner,name:$repo){
    pullRequest(number:$number){
      timelineItems(first:100,itemTypes:[READY_FOR_REVIEW_EVENT,CONVERT_TO_DRAFT_EVENT]){
        nodes{
          __typename
          ... on ReadyForReviewEvent{createdAt}
          ... on ConvertToDraftEvent{createdAt}
        }
      }
    }
  }
}
"""
    for run in runs:
        if not isinstance(run, dict) or as_text(run.get("event")) != "pull_request":
            continue
        run_id = as_text(run.get("id"))
        number = pull_number_from_run(run)
        if number is not None:
            if number not in pr_cache:
                try:
                    pull_payload = client.api(f"pulls/{number}")
                except MeterError:
                    pr_cache[number] = None
                else:
                    pr_cache[number] = pull_payload if isinstance(pull_payload, dict) else None
            pull = pr_cache[number]
        else:
            branch = as_text(run.get("head_branch"))
            if not branch:
                continue
            head = f"{head_owner_for_run(run, owner)}:{branch}"
            if head not in branch_pull_cache:
                pulls = client.api(
                    "pulls",
                    params={"head": head, "state": "all", "per_page": "20"},
                )
                if isinstance(pulls, list):
                    pull_list = pulls
                else:
                    pull_list = pulls.get("pulls", []) if isinstance(pulls, dict) else []
                branch_pull_cache[head] = [pull for pull in pull_list if isinstance(pull, dict)]
            pull = select_pull_request_for_run(run, branch_pull_cache[head])
            if pull is None:
                continue
            number = pull.get("number")
            if not isinstance(number, int):
                continue
        if pull is None:
            continue
        if number not in timeline_cache:
            payload = client.graphql(query, {"owner": owner, "repo": repo_name, "number": number})
            nodes = (
                payload.get("data", {})
                .get("repository", {})
                .get("pullRequest", {})
                .get("timelineItems", {})
                .get("nodes", [])
            )
            timeline_cache[number] = nodes if isinstance(nodes, list) else []
        run_time = parse_time(run.get("created_at"))
        draft_at_run = draft_state_at_run(run_time, pull, timeline_cache[number]) if run_time else None
        ready_events = [
            as_text(node.get("createdAt"))
            for node in timeline_cache[number]
            if isinstance(node, dict) and as_text(node.get("__typename")) == "ReadyForReviewEvent"
        ]
        states[run_id] = {
            "number": number,
            "draft_at_run": draft_at_run,
            "ready_at": sorted(ready_events)[0] if ready_events else None,
            "state": as_text(pull.get("state")),
        }
    return states


def render_text(report: dict[str, object]) -> str:
    lines = [
        f"Runner-minute report for {report['repo']} generated {report['generated_at']}",
        "",
        "Run summary:",
        "run_id workflow event conclusion classes total_minutes",
    ]
    for run in report["runs"]:
        assert isinstance(run, dict)
        total = sum(entry["minutes"] for entry in run["totals_by_tier"].values())
        lines.append(
            f"{run['id']} {run['workflow_key']} {run['event']} {run['conclusion']} "
            f"{','.join(run['classifications'])} {total:.3f}"
        )

    lines.extend(["", "Job details:", "run_id job tier label minutes conclusion"])
    for run in report["runs"]:
        assert isinstance(run, dict)
        for job in run["jobs"]:
            assert isinstance(job, dict)
            lines.append(
                f"{run['id']} {job['name']} {job['tier']} {job['runner_label']} "
                f"{job['minutes']:.3f} {job['conclusion']}"
            )

    lines.extend(["", "Tier totals:"])
    for tier, entry in sorted(report["totals_by_tier"].items()):
        assert isinstance(entry, dict)
        lines.append(f"{tier}: {entry['minutes']:.3f} minutes")

    debug_sessions = report.get("debug_sessions", [])
    if debug_sessions:
        lines.extend(["", "Debug sessions:"])
        for run in debug_sessions:
            assert isinstance(run, dict)
            total = sum(entry["minutes"] for entry in run["totals_by_tier"].values())
            lines.append(f"{run['id']} {run['created_at']} {total:.3f} minutes")
    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", help="GitHub repository as OWNER/REPO. Defaults to gh repo view.")
    parser.add_argument("--config", type=pathlib.Path, default=DEFAULT_RUNNER_CONFIG)
    parser.add_argument("--run-id", action="append", default=[], help="Specific workflow run ID to include.")
    parser.add_argument("--days", type=int, help="Look back this many days across configured workflows.")
    parser.add_argument("--limit", type=int, help="Maximum lookback runs to fetch after workflow filtering.")
    parser.add_argument("--json", action="store_true", help="Print JSON only.")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not args.run_id and args.days is None:
        raise MeterError("pass at least one --run-id or --days")
    repo = args.repo or infer_repo()
    config = load_runner_config(args.config)
    client = GhClient(repo)
    runs_payload = fetch_runs(client, config, args.run_id, args.days, args.limit)
    jobs_payload, artifacts_payload = fetch_jobs_and_artifacts(client, runs_payload)
    pr_states = resolve_pr_states(client, repo, runs_payload)
    report = build_report(
        repo=repo,
        runs_payload=runs_payload,
        jobs_payload_by_run_id=jobs_payload,
        artifacts_payload_by_run_id=artifacts_payload,
        pr_state_by_run_id=pr_states,
        runner_config=config,
        generated_at=isoformat_utc(dt.datetime.now(dt.UTC)),
    )
    encoded = json.dumps(report, indent=2, sort_keys=True)
    if args.json:
        print(encoded)
    else:
        print(render_text(report))
        print()
        print("JSON:")
        print(encoded)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except MeterError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        sys.exit(1)
