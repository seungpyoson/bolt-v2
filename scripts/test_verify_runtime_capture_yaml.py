#!/usr/bin/env python3
"""Self-tests for the runtime-capture YAML verifier."""

from __future__ import annotations

import importlib.util
import contextlib
import hashlib
import io
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path
from typing import Any, Callable


SCRIPT_PATH = Path(__file__).with_name("verify_runtime_capture_yaml.py")
SPEC = importlib.util.spec_from_file_location("verify_runtime_capture_yaml", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)
ORIGINAL_FIND_PINNED_NT_API_PATH = VERIFIER.find_pinned_nt_api_path
ORIGINAL_READ_PINNED_NT_POLYMARKET_QUERY_BLOB = (
    VERIFIER.read_pinned_nt_polymarket_query_blob
)

TEST_POLYMARKET_SOURCE = "".join(
    f"upstream line {line}\n" for line in range(1, 549)
).encode("utf-8")


def polymarket_fixture(
    revision: str, source: bytes = TEST_POLYMARKET_SOURCE
) -> bytes:
    source_lines = source.splitlines(keepends=True)
    body = b"".join(
        b"".join(source_lines[start - 1 : end])
        for start, end in ((130, 137), (529, 547))
    )
    return (
        "Source: NautilusTrader\n"
        f"Revision: {revision}\n"
        "Path: crates/adapters/polymarket/src/http/query.rs\n"
        f"Full source SHA-256: {hashlib.sha256(source).hexdigest()}\n"
        "Extracted ranges from pinned checkout: lines 130-137 and 529-547\n\n"
    ).encode("utf-8") + body


class RuntimeCaptureYamlVerifierTests(unittest.TestCase):
    PINNED_REV = "a" * 40

    def setUp(self) -> None:
        self._patched_attrs: list[tuple[str, Any]] = []

    def tearDown(self) -> None:
        while self._patched_attrs:
            name, value = self._patched_attrs.pop()
            setattr(VERIFIER, name, value)

    def patch_verifier_attr(self, name: str, value: Any) -> None:
        self._patched_attrs.append((name, getattr(VERIFIER, name)))
        setattr(VERIFIER, name, value)

    def write_fixture(
        self, mutate: Callable[[dict[str, Any]], None] | None = None
    ) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        runtime_capture_dir = root / "docs" / "bolt-v3" / "research" / "runtime-capture"
        naming_dir = root / "docs" / "bolt-v3" / "research" / "naming"
        src_dir = root / "src"
        tests_dir = root / "tests"
        nt_dir = root / "nt"
        for directory in (runtime_capture_dir, naming_dir, src_dir, tests_dir, nt_dir):
            directory.mkdir(parents=True, exist_ok=True)

        fixture: dict[str, Any] = {
            "surfaces": {
                "surfaces": [
                    {
                        "nt_api": "subscribe_quotes",
                        "nt_path": "crates/common/src/msgbus/api.rs:310",
                        "message_type": "QuoteTick",
                        "topic_pattern": "data.quotes.*.*",
                        "api_kind": "passive_pubsub",
                        "bolt_status": "captured_now",
                        "source_subscribe_fn": "subscribe_quotes",
                        "bolt_pattern_helper": "quotes_pattern",
                        "capture_stream": "quotes",
                        "storage_format": "Feather",
                        "suggested_capture_storage": "feather",
                    },
                    {
                        "nt_api": "subscribe_any",
                        "nt_path": "crates/common/src/msgbus/api.rs:201",
                        "message_type": "TradingStateChanged",
                        "storage_message_type": "TradingStateChanged",
                        "topic_pattern": "events.risk",
                        "api_kind": "passive_pubsub",
                        "bolt_status": "captured_now",
                        "source_subscribe_fn": "subscribe_any",
                        "bolt_pattern_helper": "trading_state_changed_events_pattern",
                        "capture_stream": "risk_trading_state_changed",
                        "storage_format": "JSONL",
                        "suggested_capture_storage": "jsonl",
                    },
                    {
                        "nt_api": "subscribe_book_snapshots",
                        "nt_path": "crates/common/src/msgbus/api.rs:298",
                        "message_type": "OrderBook",
                        "api_kind": "passive_pubsub",
                        "bolt_status": "safe_missing_passive_stream",
                        "publisher_evidence": (
                            "crates/data/src/engine/book.rs:170 emitted book snapshots"
                        ),
                        "subscriber_evidence": (
                            "crates/common/src/msgbus/api.rs:298 -> subscribe_book_snapshots"
                        ),
                        "reason": "Passive stream documented.",
                        "suggested_capture_storage": "boundary_wrapper",
                    },
                ]
            },
            "feas": {
                "types": [
                    {
                        "message_type": "QuoteTick",
                        "nt_path": "crates/model/src/data/quote.rs:41-66",
                        "recommended_storage": "feather",
                    },
                    {
                        "message_type": "OrderBookDeltas",
                        "nt_path": "crates/model/src/data/deltas.rs:36-58",
                        "recommended_storage": "unwrap_to_orderbookdelta",
                    },
                    {
                        "message_type": "TradingStateChanged",
                        "nt_path": "crates/common/src/messages/system/trading.rs:27-45",
                        "recommended_storage": "jsonl",
                    },
                ]
            },
            "current_capture": {
                "captured_streams": [
                    {
                        "stream": "quotes",
                        "storage_format": "Feather",
                        "test_coverage": ["captures_quote_ticks"],
                    },
                    {
                        "stream": "risk_trading_state_changed",
                        "storage_format": "JSONL",
                        "test_coverage": ["captures_trading_state_changed"],
                    },
                ]
            },
            "naming_audit": {"nautilus_trader_revision": self.PINNED_REV},
            "runtime_contracts_text": f"current value: `{self.PINNED_REV}`\n",
            "cargo_text": (
                "[dependencies]\n"
                f'nautilus-common = {{ git = "https://github.com/nautechsystems/'
                f'nautilus_trader.git", rev = "{self.PINNED_REV}" }}\n'
            ),
            "src_text": """
                const RISK_DIR: &str = stringify!(risk);
                const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

                fn quotes_pattern() {}
                fn trading_state_changed_events_pattern() {}

                fn wire_nt_runtime_capture() {
                    subscribe_quotes(quotes_pattern(), handler, None);
                    subscribe_any(trading_state_changed_events_pattern(), any_handler, None);
                    let _path = spool_root_path
                        .join(RISK_DIR)
                        .join(TRADING_STATE_CHANGED_FILE);
                }
            """,
            "test_text": """
                fn captures_quote_ticks() {}
                fn captures_trading_state_changed() {}
            """,
            "nt_api_text": (
                "pub fn subscribe_quotes("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n"
                "pub fn subscribe_any("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n"
                "pub fn subscribe_book_snapshots("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n"
            ),
            "nt_polymarket_query_bytes": TEST_POLYMARKET_SOURCE,
            "polymarket_fixture_bytes": polymarket_fixture(self.PINNED_REV),
        }
        if mutate is not None:
            mutate(fixture)

        surfaces_path = runtime_capture_dir / "nt-msgbus-surfaces.yaml"
        feas_path = runtime_capture_dir / "storage-feasibility.yaml"
        current_capture_path = runtime_capture_dir / "bolt-current-capture.yaml"
        naming_audit_path = naming_dir / "nt-owned-name-audit.yaml"
        runtime_contracts_path = (
            root / "docs" / "bolt-v3" / "2026-04-25-bolt-v3-runtime-contracts.md"
        )
        src_path = src_dir / "nt_runtime_capture.rs"
        test_path = tests_dir / "nt_runtime_capture.rs"
        nt_api_path = nt_dir / "api.rs"
        nt_polymarket_query_path = nt_dir / "query.rs"
        polymarket_fixture_path = (
            tests_dir
            / "fixtures"
            / f"nt_polymarket_query_post_order_params_{self.PINNED_REV[:8]}.txt"
        )

        surfaces_path.write_text(VERIFIER.yaml.safe_dump(fixture["surfaces"]), encoding="utf-8")
        feas_path.write_text(VERIFIER.yaml.safe_dump(fixture["feas"]), encoding="utf-8")
        current_capture_path.write_text(
            VERIFIER.yaml.safe_dump(fixture["current_capture"]), encoding="utf-8"
        )
        naming_audit_path.write_text(
            VERIFIER.yaml.safe_dump(fixture["naming_audit"]), encoding="utf-8"
        )
        runtime_contracts_path.write_text(
            fixture["runtime_contracts_text"], encoding="utf-8"
        )
        (root / "Cargo.toml").write_text(fixture["cargo_text"], encoding="utf-8")
        src_path.write_text(fixture["src_text"], encoding="utf-8")
        test_path.write_text(fixture["test_text"], encoding="utf-8")
        nt_api_path.write_text(fixture["nt_api_text"], encoding="utf-8")
        nt_polymarket_query_path.write_bytes(fixture["nt_polymarket_query_bytes"])
        polymarket_fixture_path.parent.mkdir(parents=True, exist_ok=True)
        polymarket_fixture_path.write_bytes(fixture["polymarket_fixture_bytes"])

        self.patch_verifier_attr("REPO_ROOT", root)
        self.patch_verifier_attr("SURFACES_PATH", surfaces_path)
        self.patch_verifier_attr("FEAS_PATH", feas_path)
        self.patch_verifier_attr("CURRENT_CAPTURE_PATH", current_capture_path)
        self.patch_verifier_attr("NAMING_AUDIT_PATH", naming_audit_path)
        self.patch_verifier_attr("SRC_PATH", src_path)
        self.patch_verifier_attr("TEST_PATH", test_path)
        self.patch_verifier_attr("POLYMARKET_QUERY_FIXTURE_PATH", polymarket_fixture_path)
        self.patch_verifier_attr(
            "find_pinned_nt_api_path",
            lambda findings, nautilus_revision: nt_api_path,
        )
        self.patch_verifier_attr(
            "find_pinned_nt_polymarket_query_path",
            lambda findings, nautilus_revision: nt_polymarket_query_path,
        )

        self.patch_verifier_attr(
            "read_pinned_nt_polymarket_query_blob",
            lambda findings, nautilus_revision, upstream_path: upstream_path.read_bytes(),
        )

    def assert_collects(
        self, expected_check_id: str, mutate: Callable[[dict[str, Any]], None]
    ) -> None:
        self.write_fixture(mutate)
        failures = VERIFIER.collect_failures()
        self.assertIn(expected_check_id, [check_id for check_id, _ in failures], failures)

    def test_collect_failures_accepts_consistent_fixture(self) -> None:
        self.write_fixture()

        self.assertEqual([], VERIFIER.collect_failures())

    def test_polymarket_fixture_filename_must_match_pinned_revision(self) -> None:
        self.write_fixture()
        fixture_path = VERIFIER.POLYMARKET_QUERY_FIXTURE_PATH
        mislabeled_path = fixture_path.with_name(
            "nt_polymarket_query_post_order_params_00000000.txt"
        )
        fixture_path.rename(mislabeled_path)
        self.patch_verifier_attr("POLYMARKET_QUERY_FIXTURE_PATH", mislabeled_path)

        failures = VERIFIER.collect_failures()

        self.assertIn(
            "13.polymarket_fixture_provenance",
            [check_id for check_id, _ in failures],
            failures,
        )
        self.assertTrue(
            any("fixture filename must be" in message for _, message in failures),
            failures,
        )

    def test_polymarket_fixture_uses_committed_blob_when_checkout_is_dirty(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        checkout = Path(temp.name)
        query_path = checkout / VERIFIER.POLYMARKET_QUERY_RELATIVE_PATH
        query_path.parent.mkdir(parents=True)
        query_path.write_bytes(TEST_POLYMARKET_SOURCE)
        subprocess.run(["git", "init", "-q", str(checkout)], check=True)
        subprocess.run(
            ["git", "-C", str(checkout), "add", query_path.relative_to(checkout)],
            check=True,
        )
        subprocess.run(
            [
                "git",
                "-C",
                str(checkout),
                "-c",
                "user.name=runtime-capture-test",
                "-c",
                "user.email=runtime-capture-test@example.invalid",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
            check=True,
        )
        revision = subprocess.run(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

        dirty_source = TEST_POLYMARKET_SOURCE.replace(
            b"upstream line 130", b"dirty line 130"
        )
        query_path.write_bytes(dirty_source)
        fixture_path = checkout / (
            f"nt_polymarket_query_post_order_params_{revision[:8]}.txt"
        )
        fixture_path.write_bytes(polymarket_fixture(revision, dirty_source))
        self.patch_verifier_attr("POLYMARKET_QUERY_FIXTURE_PATH", fixture_path)
        self.patch_verifier_attr(
            "read_pinned_nt_polymarket_query_blob",
            ORIGINAL_READ_PINNED_NT_POLYMARKET_QUERY_BLOB,
        )

        findings: list[tuple[str, str]] = []
        VERIFIER.verify_polymarket_query_fixture(findings, revision, query_path)

        self.assertIn(
            "13.polymarket_fixture_provenance",
            [check_id for check_id, _ in findings],
            findings,
        )

    def test_main_returns_zero_for_consistent_fixture(self) -> None:
        self.write_fixture()

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, VERIFIER.main())

    def test_main_returns_one_when_fixture_has_violations(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += "\nnormalized_sink\n"

        self.write_fixture(mutate)

        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(1, VERIFIER.main())

    def test_collect_failures_rejects_stale_runtime_capture_references(self) -> None:
        self.assert_collects(
            "1.stale_ref",
            lambda fixture: fixture.__setitem__(
                "src_text", fixture["src_text"] + "\nnormalized_sink\n"
            ),
        )

    def test_collect_failures_rejects_storage_rows_without_nt_line_refs(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["feas"]["types"][0]["nt_path"] = "crates/model/src/data/quote.rs"

        self.assert_collects("2.nt_path_line_ref", mutate)

    def test_collect_failures_rejects_captured_rows_without_source_linkage(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            del fixture["surfaces"]["surfaces"][0]["source_subscribe_fn"]

        self.assert_collects("3.captured_now_missing_source_subscribe_fn", mutate)

    def test_collect_failures_rejects_captured_nt_api_source_mismatches(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["source_subscribe_fn"] = "subscribe_any"

        self.assert_collects("3.captured_now_nt_api_mismatch", mutate)

    def test_collect_failures_rejects_captured_rows_without_pattern_helper(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            del fixture["surfaces"]["surfaces"][0]["bolt_pattern_helper"]

        self.assert_collects("3.captured_now_missing_pattern_helper", mutate)

    def test_collect_failures_rejects_captured_rows_without_capture_stream(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            del fixture["surfaces"]["surfaces"][0]["capture_stream"]

        self.assert_collects("3.captured_now_missing_capture_stream", mutate)

    def test_collect_failures_rejects_captured_rows_without_storage_format(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            del fixture["surfaces"]["surfaces"][0]["storage_format"]

        self.assert_collects("3.captured_now_missing_storage_format", mutate)

    def test_collect_failures_rejects_missing_pattern_helper_definition(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["bolt_pattern_helper"] = "missing_pattern"

        self.assert_collects("3.captured_now_pattern_missing_in_src", mutate)

    def test_collect_failures_accepts_pattern_helper_with_spaced_definition(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] = fixture["src_text"].replace(
                "fn quotes_pattern() {}",
                "fn quotes_pattern () {}",
            )

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_rejects_commented_pattern_helper_definitions(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] = fixture["src_text"].replace(
                "fn quotes_pattern() {}",
                "// fn quotes_pattern() {}",
            )

        self.assert_collects("3.captured_now_pattern_missing_in_src", mutate)

    def test_collect_failures_rejects_string_embedded_pattern_helper_definitions(
        self,
    ) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] = fixture["src_text"].replace(
                "fn quotes_pattern() {}",
                'let _doc = "fn quotes_pattern() {}";',
            )

        self.assert_collects("3.captured_now_pattern_missing_in_src", mutate)

    def test_collect_failures_rejects_safe_missing_rows_without_evidence(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            row = fixture["surfaces"]["surfaces"][2]
            del row["publisher_evidence"]
            del row["subscriber_evidence"]
            row["nt_path"] = "crates/common/src/msgbus/api"

        self.write_fixture(mutate)
        failure_ids = [check_id for check_id, _ in VERIFIER.collect_failures()]

        self.assertIn("4.safe_missing_no_publisher_evidence", failure_ids)
        self.assertIn("4.safe_missing_no_subscriber_evidence", failure_ids)

    def test_collect_failures_rejects_missing_risk_jsonl_capture_path(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] = fixture["src_text"].replace(
                ".join(RISK_DIR)",
                '.join("other")',
            )

        self.assert_collects("5.risk_jsonl_path_missing_in_src", mutate)

    def test_collect_failures_rejects_missing_risk_surface_row(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"] = [
                row
                for row in fixture["surfaces"]["surfaces"]
                if row.get("topic_pattern") != "events.risk"
            ]

        self.assert_collects("5.risk_row_missing", mutate)

    def test_collect_failures_rejects_risk_surface_not_captured_now(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][1]["bolt_status"] = "deferred_capture"

        self.assert_collects("5.risk_not_captured_now", mutate)

    def test_collect_failures_rejects_order_book_deltas_feather_storage(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["feas"]["types"][1]["recommended_storage"] = "feather"

        self.assert_collects("6.deltas_storage_feather", mutate)

    def test_collect_failures_rejects_missing_order_book_deltas_row(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["feas"]["types"] = [
                row
                for row in fixture["feas"]["types"]
                if row.get("message_type") != "OrderBookDeltas"
            ]

        self.assert_collects("6.deltas_row_missing", mutate)

    def test_collect_failures_rejects_unbounded_storage_values(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["feas"]["types"][0]["recommended_storage"] = "parquet"

        self.assert_collects("7.storage_not_allowed", mutate)

    def test_collect_failures_rejects_missing_storage_values(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            del fixture["feas"]["types"][0]["recommended_storage"]

        self.assert_collects("7.storage_missing", mutate)

    def test_collect_failures_rejects_unbounded_surface_storage_values(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["suggested_capture_storage"] = "sqlite"

        self.assert_collects("7.storage_not_allowed", mutate)

    def test_collect_failures_rejects_unbounded_api_kind_values(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["api_kind"] = "active_pubsub"

        self.assert_collects("8.api_kind_not_allowed", mutate)

    def test_collect_failures_rejects_missing_pinned_subscribe_api_rows(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["nt_api_text"] += (
                "\npub fn subscribe_book_depth10(pattern: Pattern, handler: Handler, "
                "priority: Option<u8>) {}\n"
            )

        self.assert_collects("9.pinned_subscribe_api_missing", mutate)

    def test_collect_failures_rejects_missing_pinned_nt_checkout(self) -> None:
        self.write_fixture()
        empty_home = tempfile.TemporaryDirectory()
        self.addCleanup(empty_home.cleanup)
        self.patch_verifier_attr(
            "find_pinned_nt_api_path",
            ORIGINAL_FIND_PINNED_NT_API_PATH,
        )

        with mock.patch.object(VERIFIER.Path, "home", return_value=Path(empty_home.name)):
            failure_ids = [check_id for check_id, _ in VERIFIER.collect_failures()]

        self.assertIn("9.pinned_nt_api_missing", failure_ids)

    def test_collect_failures_rejects_extra_passive_api_missing_from_pinned_nt(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["nt_api_text"] = fixture["nt_api_text"].replace(
                "pub fn subscribe_any("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n",
                "",
            )

        self.assert_collects("9.extra_passive_api_not_in_pinned_nt", mutate)

    def test_collect_failures_rejects_storage_recommendation_mismatches(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["suggested_capture_storage"] = "jsonl"

        self.assert_collects("10.surface_storage_mismatch", mutate)

    def test_collect_failures_rejects_captured_rows_without_storage_recommendation(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            del fixture["surfaces"]["surfaces"][0]["suggested_capture_storage"]

        self.assert_collects("10.captured_now_storage_missing", mutate)

    def test_collect_failures_rejects_unwaived_portfolio_snapshot_gap(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"].append(
                {
                    "nt_api": "subscribe_portfolio_snapshot",
                    "nt_path": "crates/common/src/msgbus/api.rs:470",
                    "message_type": "PortfolioSnapshot",
                    "api_kind": "passive_pubsub",
                    "bolt_status": "safe_missing_passive_stream",
                    "publisher_evidence": "crates/portfolio/src/portfolio.rs:2597 -> publish_portfolio_snapshot",
                    "subscriber_evidence": "crates/common/src/msgbus/api.rs:470 -> subscribe_portfolio_snapshot",
                    "reason": "Documented but not captured.",
                    "suggested_capture_storage": "jsonl",
                }
            )
            fixture["feas"]["types"].append(
                {
                    "message_type": "PortfolioSnapshot",
                    "nt_path": "crates/model/src/events/portfolio/snapshot.rs:46-68",
                    "recommended_storage": "jsonl",
                }
            )
            fixture["nt_api_text"] += (
                "pub fn subscribe_portfolio_snapshot("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n"
            )

        self.assert_collects("15.portfolio_snapshot_capture_or_waiver_missing", mutate)

    def test_collect_failures_rejects_captured_portfolio_snapshot_without_jsonl_spool_path(
        self,
    ) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"].append(
                {
                    "nt_api": "subscribe_portfolio_snapshot",
                    "nt_path": "crates/common/src/msgbus/api.rs:470",
                    "message_type": "PortfolioSnapshot",
                    "api_kind": "passive_pubsub",
                    "bolt_status": "captured_now",
                    "source_subscribe_fn": "subscribe_portfolio_snapshot",
                    "bolt_pattern_helper": "portfolio_snapshots_pattern",
                    "capture_stream": "portfolio_snapshot",
                    "storage_format": "JSONL",
                    "suggested_capture_storage": "jsonl",
                }
            )
            fixture["feas"]["types"].append(
                {
                    "message_type": "PortfolioSnapshot",
                    "nt_path": "crates/model/src/events/portfolio/snapshot.rs:46-68",
                    "recommended_storage": "jsonl",
                }
            )
            fixture["current_capture"]["captured_streams"].append(
                {
                    "stream": "portfolio_snapshot",
                    "storage_format": "JSONL",
                    "test_coverage": [
                        "captures_broad_nt_runtime_jsonl_records_outside_hot_path"
                    ],
                }
            )
            fixture["src_text"] += """
                fn portfolio_snapshots_pattern() {}
                fn subscribe_portfolio() {
                    subscribe_portfolio_snapshot(
                        portfolio_snapshots_pattern(),
                        handler,
                        None,
                    );
                }
            """
            fixture["test_text"] += (
                "\nfn captures_broad_nt_runtime_jsonl_records_outside_hot_path() {}\n"
            )
            fixture["nt_api_text"] += (
                "pub fn subscribe_portfolio_snapshot("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n"
            )

        self.assert_collects("15.portfolio_snapshot_jsonl_path_missing_in_src", mutate)

    def test_collect_failures_rejects_captured_portfolio_snapshot_without_write_branch(
        self,
    ) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"].append(
                {
                    "nt_api": "subscribe_portfolio_snapshot",
                    "nt_path": "crates/common/src/msgbus/api.rs:470",
                    "message_type": "PortfolioSnapshot",
                    "api_kind": "passive_pubsub",
                    "bolt_status": "captured_now",
                    "source_subscribe_fn": "subscribe_portfolio_snapshot",
                    "bolt_pattern_helper": "portfolio_snapshots_pattern",
                    "capture_stream": "portfolio_snapshot",
                    "storage_format": "JSONL",
                    "suggested_capture_storage": "jsonl",
                }
            )
            fixture["feas"]["types"].append(
                {
                    "message_type": "PortfolioSnapshot",
                    "nt_path": "crates/model/src/events/portfolio/snapshot.rs:46-68",
                    "recommended_storage": "jsonl",
                }
            )
            fixture["current_capture"]["captured_streams"].append(
                {
                    "stream": "portfolio_snapshot",
                    "storage_format": "JSONL",
                    "test_coverage": [
                        "captures_broad_nt_runtime_jsonl_records_outside_hot_path"
                    ],
                }
            )
            fixture["src_text"] += """
                const PORTFOLIO_SNAPSHOT_DIR: &str = "portfolio_snapshot";
                const SNAPSHOTS_FILE: &str = "snapshots.jsonl";

                fn portfolio_snapshots_pattern() {}
                fn subscribe_portfolio() {
                    subscribe_portfolio_snapshot(
                        portfolio_snapshots_pattern(),
                        handler,
                        None,
                    );
                }

                fn build_jsonl_paths(spool_root_path: PathBuf) {
                    let _path = spool_root_path
                        .join(PORTFOLIO_SNAPSHOT_DIR)
                        .join(SNAPSHOTS_FILE);
                }
            """
            fixture["test_text"] += (
                "\nfn captures_broad_nt_runtime_jsonl_records_outside_hot_path() {}\n"
            )
            fixture["nt_api_text"] += (
                "pub fn subscribe_portfolio_snapshot("
                "pattern: Pattern, handler: Handler, priority: Option<u8>) {}\n"
            )

        self.assert_collects("15.portfolio_snapshot_write_branch_missing_in_src", mutate)

    def test_collect_failures_rejects_surface_storage_without_feasibility_row(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["message_type"] = "MissingType"

        self.assert_collects("10.surface_storage_missing_feasibility", mutate)

    def test_collect_failures_rejects_source_subscribe_rows_missing_from_yaml(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                fn trades_pattern() {}
                fn extra_subscription() {
                    subscribe_trades(trades_pattern(), handler, None);
                }
            """

        self.assert_collects("11.source_subscribe_not_captured_now", mutate)

    def test_collect_failures_rejects_source_subscribe_literal_patterns(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                fn literal_subscription() {
                    subscribe_trades(MStr::pattern("data.trades"), handler, None);
                }
            """

        self.assert_collects("11.source_subscribe_literal_pattern", mutate)

    def test_collect_failures_rejects_source_subscribe_without_pattern_helper(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                fn dynamic_subscription() {
                    subscribe_trades(build_topic(), handler, None);
                }
            """

        self.assert_collects("11.source_subscribe_no_pattern_helper", mutate)

    def test_collect_failures_rejects_unparseable_source_subscribe_calls(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                fn trades_pattern() {}
                fn broken_subscription() {
                    subscribe_trades(trades_pattern(), handler, None;
                }
            """

        self.assert_collects("11.source_subscribe_parse_failed", mutate)

    def test_collect_failures_rejects_captured_surface_missing_source_call(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            row = fixture["surfaces"]["surfaces"][0]
            row["source_subscribe_fn"] = "subscribe_trades"
            row["bolt_pattern_helper"] = "trades_pattern"
            fixture["src_text"] += "\nfn trades_pattern() {}\n"

        self.assert_collects("11.captured_now_not_in_source", mutate)

    def test_collect_failures_ignores_source_subscribe_calls_inside_raw_strings(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += '''
                fn raw_doc_fixture() {
                    let _doc = r#"prefix " subscribe_trades(trades_pattern(), handler, None)"#;
                }
            '''

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_ignores_source_subscribe_calls_inside_nested_comments(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                /*
                    outer comment start
                    /* nested comment */
                    subscribe_trades(trades_pattern(), handler, None);
                */
            """

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_ignores_lifetimes_when_scanning_source_calls(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                fn lifetime_doc<'a>(value: &'a str) -> &'a str {
                    value
                }
            """

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_detects_source_subscribe_calls_after_lifetimes(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                fn trades_pattern() {}
                fn lifetime_subscription<'a>(value: &'a str) -> &'a str {
                    subscribe_trades(trades_pattern(), handler, None);
                    value
                }
            """

        self.assert_collects("11.source_subscribe_not_captured_now", mutate)

    def test_collect_failures_ignores_subscribe_function_definitions(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                pub fn subscribe_trades(pattern: Pattern, handler: Handler) {
                    let _ = (pattern, handler);
                }
            """

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_ignores_wide_spaced_subscribe_function_definitions(
        self,
    ) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["src_text"] += """
                pub fn                         subscribe_trades(
                    pattern: Pattern,
                    handler: Handler,
                ) {
                    let _ = (pattern, handler);
                }
            """

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_rejects_current_capture_storage_mismatches(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["current_capture"]["captured_streams"][0]["storage_format"] = "JSONL"

        self.assert_collects("12.current_capture_storage_mismatch", mutate)

    def test_collect_failures_rejects_missing_current_capture_streams(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["capture_stream"] = "missing_quotes"

        self.assert_collects("12.current_capture_stream_missing", mutate)

    def test_collect_failures_rejects_suggested_storage_format_mismatches(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["suggested_capture_storage"] = "jsonl"

        self.assert_collects("12.suggested_storage_format_mismatch", mutate)

    def test_collect_failures_rejects_documented_pin_drift(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["naming_audit"]["nautilus_trader_revision"] = "0" * 40

        self.assert_collects("13.pin_revision_mismatch", mutate)

    def test_collect_failures_rejects_polymarket_fixture_digest_drift(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            digest = hashlib.sha256(TEST_POLYMARKET_SOURCE).hexdigest().encode("ascii")
            fixture["polymarket_fixture_bytes"] = fixture["polymarket_fixture_bytes"].replace(
                digest, b"0" * 64
            )

        self.assert_collects("13.polymarket_fixture_provenance", mutate)

    def test_collect_failures_rejects_polymarket_fixture_path_drift(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["polymarket_fixture_bytes"] = fixture["polymarket_fixture_bytes"].replace(
                b"/query.rs", b"/models.rs"
            )

        self.assert_collects("13.polymarket_fixture_provenance", mutate)

    def test_collect_failures_rejects_polymarket_fixture_range_drift(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["polymarket_fixture_bytes"] = fixture["polymarket_fixture_bytes"].replace(
                b"lines 130-137", b"lines 130-136"
            )

        self.assert_collects("13.polymarket_fixture_provenance", mutate)

    def test_collect_failures_rejects_polymarket_fixture_byte_drift(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            original = fixture["polymarket_fixture_bytes"]
            fixture["polymarket_fixture_bytes"] = original[:-1] + bytes(
                [original[-1] ^ 1]
            )

        self.assert_collects("13.polymarket_fixture_provenance", mutate)

    def test_collect_failures_rejects_multiple_cargo_pins(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["cargo_text"] = (
                "[dependencies]\n"
                f'nautilus-common = {{ git = "https://github.com/nautechsystems/'
                f'nautilus_trader.git", rev = "{self.PINNED_REV}" }}\n'
                'nautilus-model = { git = "https://github.com/nautechsystems/'
                f'nautilus_trader.git", rev = "{"b" * 40}" }}\n'
            )

        self.assert_collects("13.pin_revision_mismatch", mutate)

    def test_collect_failures_rejects_missing_cargo_pin(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["cargo_text"] = (
                "[dependencies]\n"
                'nautilus-common = { git = "https://github.com/nautechsystems/'
                'nautilus_trader.git" }\n'
            )

        self.assert_collects("13.pin_revision_missing", mutate)

    def test_collect_failures_rejects_personal_nautilus_source(self) -> None:
        personal_source = "https://github.com/" + "seungpyoson/" + "nautilus_trader.git"

        def mutate(fixture: dict[str, Any]) -> None:
            fixture["cargo_text"] = (
                "[dependencies]\n"
                f'nautilus-common = {{ git = "{personal_source}", rev = "{self.PINNED_REV}" }}\n'
            )

        self.assert_collects("13.pin_revision_mismatch", mutate)

    def test_collect_failures_accepts_quoted_table_cargo_pin(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["cargo_text"] = (
                '[dependencies."nautilus-common"]\n'
                'git = "https://github.com/nautechsystems/nautilus_trader.git"\n'
                f'rev = "{self.PINNED_REV}"\n'
            )

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_accepts_async_pinned_subscribe_apis(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["nt_api_text"] = fixture["nt_api_text"].replace(
                "pub fn subscribe_quotes(",
                "pub async fn subscribe_quotes(",
            )

        self.write_fixture(mutate)

        self.assertEqual([], VERIFIER.collect_failures())

    def test_collect_failures_rejects_surfaces_yaml_pin_literals(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["surfaces"]["surfaces"][0]["reason"] = self.PINNED_REV

        self.assert_collects("13.surfaces_yaml_pin_literal", mutate)

    def test_collect_failures_rejects_stale_current_capture_streams(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["current_capture"]["captured_streams"].append(
                {
                    "stream": "orphan_stream",
                    "storage_format": "JSONL",
                    "test_coverage": ["missing_orphan_stream_test"],
                }
            )

        self.assert_collects("14.current_capture_stale_stream", mutate)

    def test_collect_failures_rejects_current_capture_rows_with_missing_tests(self) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["current_capture"]["captured_streams"][0]["test_coverage"] = [
                "missing_quotes_test"
            ]

        self.assert_collects("14.current_capture_missing_test", mutate)

    def test_collect_failures_rejects_commented_current_capture_test_functions(
        self,
    ) -> None:
        def mutate(fixture: dict[str, Any]) -> None:
            fixture["test_text"] = fixture["test_text"].replace(
                "fn captures_quote_ticks() {}",
                "// fn captures_quote_ticks() {}",
            )

        self.assert_collects("14.current_capture_missing_test", mutate)

    def test_accepts_const_owned_risk_jsonl_path(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let path = spool_root_path
            .join(RISK_DIR)
            .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertTrue(VERIFIER.has_risk_jsonl_path(source))

    def test_accepts_literal_risk_jsonl_path(self) -> None:
        source = """
        const RISK_DIR: &str = "risk";
        let path = spool_root_path
            .join("risk")
            .join("trading_state_changed.jsonl");
        """

        self.assertTrue(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_filename_without_risk_path(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_risk_const_with_different_join_directory(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let path = spool_root_path
            .join("other")
            .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_join_chain_without_risk_const(self) -> None:
        source = """
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let path = spool_root_path
            .join("risk")
            .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_commented_risk_jsonl_path(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        // let path = spool_root_path
        //     .join(RISK_DIR)
        //     .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_string_embedded_risk_jsonl_path(self) -> None:
        source = r'''
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let doc = ".join(RISK_DIR).join(TRADING_STATE_CHANGED_FILE)";
        '''

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
