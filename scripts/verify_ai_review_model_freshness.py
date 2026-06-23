#!/usr/bin/env python3
"""Verify AI review model pins against provider model sources."""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import re
import sys
import tempfile
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "ai-review.toml"
KIMI_CODE_MODEL_RE = re.compile(r"\bkimi-k(?P<version>\d+(?:\.\d+)*)-code(?:-highspeed)?\b")
GLM_TEXT_MODEL_RE = re.compile(r"\bGLM-(?P<version>\d+(?:\.\d+)*)(?![-\w])", re.IGNORECASE)


@dataclass(frozen=True)
class ModelPins:
    kimi: str
    glm: str
    glm_pr_agent: str


@dataclass(frozen=True)
class FreshnessSources:
    kimi_chat_docs_url: str
    kimi_models_url: str
    glm_docs_index_url: str
    glm_migration_docs_url: str


def version_key(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


def load_config(path: Path) -> tuple[ModelPins, FreshnessSources]:
    parsed = tomllib.loads(path.read_text(encoding="utf-8"))
    try:
        glm = parsed["glm"]
        kimi = parsed["kimi"]
        freshness = parsed["model_freshness"]
    except KeyError as exc:
        raise ValueError(f"ci/ai-review.toml missing required table/key: {exc}") from exc

    pr_agent = glm.get("pr_agent")
    if not isinstance(pr_agent, dict):
        raise ValueError("ci/ai-review.toml missing [glm.pr_agent]")

    pins = ModelPins(
        kimi=string_value(kimi, "model", "kimi.model"),
        glm=string_value(glm, "model", "glm.model"),
        glm_pr_agent=string_value(pr_agent, "model", "glm.pr_agent.model"),
    )
    sources = FreshnessSources(
        kimi_chat_docs_url=string_value(freshness, "kimi_chat_docs_url", "model_freshness.kimi_chat_docs_url"),
        kimi_models_url=string_value(freshness, "kimi_models_url", "model_freshness.kimi_models_url"),
        glm_docs_index_url=string_value(freshness, "glm_docs_index_url", "model_freshness.glm_docs_index_url"),
        glm_migration_docs_url=string_value(
            freshness,
            "glm_migration_docs_url",
            "model_freshness.glm_migration_docs_url",
        ),
    )
    return pins, sources


def string_value(table: dict[str, object], key: str, label: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"ci/ai-review.toml {label} must be a non-empty string")
    return value


def model_alias_findings(pins: ModelPins) -> list[str]:
    findings: list[str] = []
    for label, value in (
        ("kimi.model", pins.kimi),
        ("glm.model", pins.glm),
        ("glm.pr_agent.model", pins.glm_pr_agent),
    ):
        if "latest" in value.lower():
            findings.append(f"{label} must be an exact model id, not a latest alias: {value!r}")
    if pins.glm_pr_agent != f"openai/{pins.glm}":
        findings.append(
            "glm.pr_agent.model must wrap the same exact GLM model as glm.model "
            f"({pins.glm_pr_agent!r} != 'openai/{pins.glm}')"
        )
    return findings


def pick_latest_kimi_code_model(model_ids: Iterable[str]) -> str | None:
    matches: list[tuple[tuple[int, ...], bool, str]] = []
    for model_id in model_ids:
        match = KIMI_CODE_MODEL_RE.search(model_id)
        if match:
            matches.append((version_key(match.group("version")), model_id.endswith("-highspeed"), model_id))
    if not matches:
        return None
    matches.sort(key=lambda item: (item[0], not item[1]), reverse=True)
    return matches[0][2]


def parse_kimi_chat_docs_latest(text: str) -> str | None:
    default_match = re.search(r"default:\s*(kimi-k\d+(?:\.\d+)*-code)\b", text)
    if default_match:
        return default_match.group(1)
    return pick_latest_kimi_code_model(match.group(0) for match in KIMI_CODE_MODEL_RE.finditer(text))


def parse_kimi_models_api_latest(text: str) -> str | None:
    payload = json.loads(text)
    data = payload.get("data")
    if not isinstance(data, list):
        raise ValueError("Kimi models API response missing data list")
    model_ids = [item.get("id") for item in data if isinstance(item, dict) and isinstance(item.get("id"), str)]
    return pick_latest_kimi_code_model(model_ids)


def parse_glm_docs_latest(text: str) -> str | None:
    candidates: list[tuple[tuple[int, ...], str]] = []
    for match in GLM_TEXT_MODEL_RE.finditer(text):
        model = f"glm-{match.group('version')}"
        candidates.append((version_key(match.group("version")), model))
    if not candidates:
        return None
    candidates.sort(reverse=True)
    return candidates[0][1]


def parse_glm_migration_model(text: str) -> str | None:
    match = re.search(r"Update\s+`?model`?\s+to\s+`(glm-\d+(?:\.\d+)*)`", text, re.IGNORECASE)
    return match.group(1) if match else parse_glm_docs_latest(text)


def fetch_text(url: str, token: str | None = None) -> str:
    headers = {"User-Agent": "bolt-v2-ai-review-model-freshness/1.0"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"GET {url} failed with HTTP {exc.code}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"GET {url} failed: {exc.reason}") from exc


def live_latest_models(sources: FreshnessSources) -> tuple[str | None, str | None, list[str]]:
    warnings: list[str] = []
    kimi_token = os.environ.get("MOONSHOT_API_KEY") or os.environ.get("KIMI_API_KEY")
    kimi_latest: str | None = None
    if kimi_token:
        try:
            kimi_latest = parse_kimi_models_api_latest(fetch_text(sources.kimi_models_url, token=kimi_token))
        except Exception as exc:  # noqa: BLE001 - keep fallback diagnostic actionable.
            warnings.append(f"Kimi models API unavailable; falling back to public docs: {exc}")
    if kimi_latest is None:
        kimi_latest = parse_kimi_chat_docs_latest(fetch_text(sources.kimi_chat_docs_url))

    glm_latest = parse_glm_docs_latest(fetch_text(sources.glm_docs_index_url))
    migration_model = parse_glm_migration_model(fetch_text(sources.glm_migration_docs_url))
    if glm_latest and migration_model and glm_latest != migration_model:
        warnings.append(
            "Z.AI docs disagree on latest GLM text model: "
            f"index={glm_latest!r}, migration={migration_model!r}"
        )
    return kimi_latest, glm_latest or migration_model, warnings


def check_pins_against_latest(pins: ModelPins, kimi_latest: str | None, glm_latest: str | None) -> list[str]:
    findings = model_alias_findings(pins)
    if kimi_latest is None:
        findings.append("Could not determine latest Kimi coding model from provider sources")
    elif pins.kimi != kimi_latest:
        findings.append(f"ci/ai-review.toml kimi.model is stale: {pins.kimi!r}; latest is {kimi_latest!r}")

    if glm_latest is None:
        findings.append("Could not determine latest GLM text coding model from provider sources")
    elif pins.glm != glm_latest:
        findings.append(f"ci/ai-review.toml glm.model is stale: {pins.glm!r}; latest is {glm_latest!r}")
    return findings


def model_update_warning(*, provider: str, config_key: str, current: str, latest: str) -> str:
    return (
        f"{provider} model update available: ci/ai-review.toml {config_key} uses `{current}`, "
        f"but official provider sources report `{latest}` as the latest coding model. "
        f"The review continues with the pinned model; update the model pin in a reviewed PR."
    )


def build_advisory_outputs(
    *,
    pins: ModelPins,
    kimi_latest: str | None,
    glm_latest: str | None,
    warnings: list[str],
) -> dict[str, str]:
    kimi_warning = ""
    glm_warning = ""
    if kimi_latest and pins.kimi != kimi_latest:
        kimi_warning = model_update_warning(
            provider="Kimi",
            config_key="kimi.model",
            current=pins.kimi,
            latest=kimi_latest,
        )
    if glm_latest and pins.glm != glm_latest:
        glm_warning = model_update_warning(
            provider="GLM",
            config_key="glm.model",
            current=pins.glm,
            latest=glm_latest,
        )
    stale = bool(kimi_warning or glm_warning)
    return {
        "stale": "true" if stale else "false",
        "kimi_warning": kimi_warning,
        "glm_warning": glm_warning,
        "source_warnings": "\n".join(warnings),
    }


def write_github_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT", "")
    if not output_path:
        return
    with Path(output_path).open("a", encoding="utf-8") as output:
        if "\n" in value:
            delimiter = f"EOF_{name.upper()}"
            output.write(f"{name}<<{delimiter}\n{value}\n{delimiter}\n")
        else:
            output.write(f"{name}={value}\n")


def write_github_outputs(values: dict[str, str]) -> None:
    for name, value in values.items():
        write_github_output(name, value)


def run_self_test() -> None:
    old_kimi_version = "9.1"
    current_kimi_version = "9.2"
    next_kimi_version = "9.3"
    old_kimi = f"kimi-k{old_kimi_version}-code"
    current_kimi = f"kimi-k{current_kimi_version}-code"
    next_kimi = f"kimi-k{next_kimi_version}-code"
    old_glm_version = "9.1"
    current_glm_version = "9.2"
    old_glm = f"glm-{old_glm_version}"
    current_glm = f"glm-{current_glm_version}"
    current_pr_agent_glm = f"openai/{current_glm}"
    kimi_doc = f"model\ndefault:{current_kimi}\nAvailable options: `{current_kimi}-highspeed`"
    assert parse_kimi_chat_docs_latest(kimi_doc) == current_kimi
    kimi_api = json.dumps({"data": [{"id": old_kimi}, {"id": f"{current_kimi}-highspeed"}, {"id": current_kimi}]})
    assert parse_kimi_models_api_latest(kimi_api) == current_kimi
    glm_index = f"[GLM-{old_glm_version}](/guides/llm/glm-old.md) [GLM-{current_glm_version}](/guides/llm/glm-current.md)"
    assert parse_glm_docs_latest(glm_index) == current_glm
    migration = f"Migration Checklist\n* Update model identifier to `{current_glm}`"
    assert parse_glm_migration_model(migration) == current_glm
    pins = ModelPins(kimi=old_kimi, glm=old_glm, glm_pr_agent=f"openai/{old_glm}")
    findings = check_pins_against_latest(pins, current_kimi, current_glm)
    assert any("kimi.model is stale" in finding for finding in findings), findings
    assert any("glm.model is stale" in finding for finding in findings), findings
    alias_findings = model_alias_findings(ModelPins(kimi="kimi-latest", glm=current_glm, glm_pr_agent=current_pr_agent_glm))
    assert any("latest alias" in finding for finding in alias_findings), alias_findings
    advisory = build_advisory_outputs(
        pins=ModelPins(kimi=current_kimi, glm=current_glm, glm_pr_agent=current_pr_agent_glm),
        kimi_latest=next_kimi,
        glm_latest=current_glm,
        warnings=[],
    )
    assert advisory["stale"] == "true", advisory
    assert advisory["kimi_warning"], advisory
    assert advisory["glm_warning"] == "", advisory
    assert current_kimi in advisory["kimi_warning"], advisory
    assert next_kimi in advisory["kimi_warning"], advisory
    config_text = f"""
[model_freshness]
kimi_chat_docs_url = "https://example.invalid/kimi-chat"
kimi_models_url = "https://example.invalid/kimi-models"
glm_docs_index_url = "https://example.invalid/glm-index"
glm_migration_docs_url = "https://example.invalid/glm-migration"

[glm]
model = "{current_glm}"

[glm.pr_agent]
model = "{current_pr_agent_glm}"

[kimi]
model = "{current_kimi}"
"""
    original_live_latest_models = live_latest_models
    try:
        globals()["live_latest_models"] = lambda sources: (next_kimi, current_glm, [])
        with tempfile.TemporaryDirectory() as tmpdir:
            config_path = Path(tmpdir) / "ai-review.toml"
            config_path.write_text(config_text, encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                assert main(["--config-file", str(config_path), "--live", "--advisory"]) == 0
    finally:
        globals()["live_latest_models"] = original_live_latest_models


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config-file", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--live", action="store_true", help="Fetch provider sources and compare pinned models")
    parser.add_argument("--advisory", action="store_true", help="Emit stale-model outputs but exit zero for drift")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        run_self_test()
        print("AI review model freshness self-tests OK")
        return 0

    try:
        pins, sources = load_config(args.config_file)
        findings = model_alias_findings(pins)
        kimi_latest: str | None = None
        glm_latest: str | None = None
        warnings: list[str] = []
        if args.live:
            try:
                kimi_latest, glm_latest, warnings = live_latest_models(sources)
            except Exception as exc:
                if not args.advisory:
                    raise
                warnings = [f"AI review model freshness source check unavailable: {exc}"]
            for warning in warnings:
                print(f"warning: {warning}", file=sys.stderr)
            findings = check_pins_against_latest(pins, kimi_latest, glm_latest)
        if args.advisory:
            advisory = build_advisory_outputs(
                pins=pins,
                kimi_latest=kimi_latest,
                glm_latest=glm_latest,
                warnings=warnings,
            )
            write_github_outputs(advisory)
            if advisory["stale"] == "true":
                for warning in (advisory["kimi_warning"], advisory["glm_warning"]):
                    if warning:
                        print(f"warning: {warning}", file=sys.stderr)
                return 0
            if findings:
                for finding in findings:
                    print(f"warning: {finding}", file=sys.stderr)
                return 0
        if findings:
            for finding in findings:
                print(f"ERROR: {finding}", file=sys.stderr)
            return 1
    except Exception as exc:  # noqa: BLE001 - script should report clean verifier errors.
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1

    if args.live:
        print(f"AI review model pins are fresh: Kimi={pins.kimi}, GLM={pins.glm}")
    else:
        print("AI review model pins use exact, internally consistent model ids")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
