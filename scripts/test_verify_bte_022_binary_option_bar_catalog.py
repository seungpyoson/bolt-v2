#!/usr/bin/env python3
"""Self-tests for the BTE-022 binary-option Bar catalog verifier."""

from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts/verify_bte_022_binary_option_bar_catalog.py"


def load_module():
    spec = importlib.util.spec_from_file_location("verify_bte_022_binary_option_bar_catalog", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def assert_finding(findings: list[str], needle: str) -> None:
    if not any(needle in finding for finding in findings):
        raise AssertionError(f"missing finding containing {needle!r}; got {findings!r}")


def test_status_must_stay_fail_closed(module) -> None:
    status = copy.deepcopy(module.load_json(module.STATUS))
    status["bte_022_can_close"] = True
    findings: list[str] = []
    module.verify_status(status, findings)
    assert_finding(findings, "bte_022_can_close")


def test_status_requires_bar_mapping(module) -> None:
    status = copy.deepcopy(module.load_json(module.STATUS))
    status["nt_data_class"] = "TradeTick"
    status["parquet_catalog_status"] = "trade_tick_only"
    findings: list[str] = []
    module.verify_status(status, findings)
    assert_finding(findings, "nt_data_class")
    assert_finding(findings, "parquet_catalog_status")


def test_source_text_must_contain_binary_option_bar_round_trip(module) -> None:
    catalog_projection = module.CATALOG_PROJECTION.read_text(encoding="utf-8").replace(
        "fn binary_option_bar_catalog_projection_round_trips_through_nt_catalog()",
        "fn removed()",
    )
    findings: list[str] = []
    module.verify_code(catalog_projection, module.RUN_MANIFEST.read_text(encoding="utf-8"), findings)
    assert_finding(findings, "binary_option_bar_catalog_projection_round_trips")


def test_comments_and_strings_only_do_not_satisfy_code_requirements(module) -> None:
    catalog_projection = "\n".join(
        f"// {snippet}\nconst STUFFED: &str = {json.dumps(snippet)};"
        for snippet in module.CATALOG_PROJECTION_REQUIRED_SOURCE_SNIPPETS
    )
    run_manifest = "\n".join(
        f"/* {snippet} */\nconst STUFFED: &str = {json.dumps(snippet)};"
        for snippet in module.RUN_MANIFEST_REQUIRED_SOURCE_SNIPPETS
    )

    findings: list[str] = []
    module.verify_code(catalog_projection, run_manifest, findings)

    assert_finding(findings, "binary_option_bar_catalog_projection_round_trips")
    assert_finding(findings, "trade_bar_replay_accepts_bar_data_config")


def test_mapping_evaluation_rejects_stale_bar_rejection(module) -> None:
    evaluation = copy.deepcopy(module.load_json(module.MAPPING_EVALUATION))
    exposure = evaluation["nt_surface_evidence"]["current_bte_manifest_exposure"]
    exposure["accepted_data_classes"] = [item for item in exposure["accepted_data_classes"] if item != "Bar"]
    exposure["rejected_for_now"].append("Bar")
    kalshi = module.mapping_for_binding(evaluation, "kalshi-official-historical-api")
    kalshi["decision"] = "Official Kalshi data is blocked because BTE currently admits only TradeTick."
    findings: list[str] = []
    module.verify_evaluation(evaluation, findings)
    assert_finding(findings, "accepted_data_classes")
    assert_finding(findings, "rejected_for_now")
    assert_finding(findings, "admits only TradeTick")


def test_justfile_requires_source_fence_wiring(module) -> None:
    justfile = module.JUSTFILE.read_text(encoding="utf-8").replace(
        "    python3 scripts/verify_bte_022_binary_option_bar_catalog.py\n",
        "",
    )
    findings: list[str] = []
    module.verify_justfile(justfile, findings)
    assert_finding(findings, "source-fence-static-inner")


def main() -> int:
    module = load_module()
    test_status_must_stay_fail_closed(module)
    test_status_requires_bar_mapping(module)
    test_source_text_must_contain_binary_option_bar_round_trip(module)
    test_comments_and_strings_only_do_not_satisfy_code_requirements(module)
    test_mapping_evaluation_rejects_stale_bar_rejection(module)
    test_justfile_requires_source_fence_wiring(module)
    print("OK: BTE-022 binary-option Bar catalog verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
