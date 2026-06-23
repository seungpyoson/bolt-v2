#!/usr/bin/env python3
"""Verify AI review model pins against provider model sources."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
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


def run_self_test() -> None:
    kimi_doc = "model\ndefault:kimi-k2.7-code\nAvailable options: `kimi-k2.7-code-highspeed`"
    assert parse_kimi_chat_docs_latest(kimi_doc) == "kimi-k2.7-code"
    kimi_api = json.dumps({"data": [{"id": "kimi-k2.6"}, {"id": "kimi-k2.7-code-highspeed"}, {"id": "kimi-k2.7-code"}]})
    assert parse_kimi_models_api_latest(kimi_api) == "kimi-k2.7-code"
    glm_index = "[GLM-5.1](/guides/llm/glm-5.1.md) [GLM-5.2](/guides/llm/glm-5.2.md)"
    assert parse_glm_docs_latest(glm_index) == "glm-5.2"
    migration = "Migration Checklist\n* Update model identifier to `glm-5.2`"
    assert parse_glm_migration_model(migration) == "glm-5.2"
    pins = ModelPins(kimi="kimi-for-coding", glm="glm-5.1", glm_pr_agent="openai/glm-5.1")
    findings = check_pins_against_latest(pins, "kimi-k2.7-code", "glm-5.2")
    assert any("kimi.model is stale" in finding for finding in findings), findings
    assert any("glm.model is stale" in finding for finding in findings), findings
    alias_findings = model_alias_findings(ModelPins(kimi="kimi-latest", glm="glm-5.2", glm_pr_agent="openai/glm-5.2"))
    assert any("latest alias" in finding for finding in alias_findings), alias_findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config-file", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--live", action="store_true", help="Fetch provider sources and compare pinned models")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        run_self_test()
        print("AI review model freshness self-tests OK")
        return 0

    try:
        pins, sources = load_config(args.config_file)
        findings = model_alias_findings(pins)
        if args.live:
            kimi_latest, glm_latest, warnings = live_latest_models(sources)
            for warning in warnings:
                print(f"warning: {warning}", file=sys.stderr)
            findings = check_pins_against_latest(pins, kimi_latest, glm_latest)
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
