#!/usr/bin/env python3
"""Self-tests for developer-tool storage hygiene policy."""

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "developer_tool_storage_hygiene.py"
POLICY = REPO_ROOT / "ci" / "developer-tool-storage-hygiene.toml"


class DeveloperToolStorageHygieneTests(unittest.TestCase):
    def load_tool_module(self) -> object:
        spec = importlib.util.spec_from_file_location("developer_tool_storage_hygiene_under_test", SCRIPT)
        if spec is None or spec.loader is None:
            raise AssertionError(f"unable to load {SCRIPT}")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module

    def run_tool(
        self,
        command: str,
        home_root: pathlib.Path,
        repo_root: pathlib.Path,
        policy: pathlib.Path = POLICY,
        extra_args: list[str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                command,
                "--policy",
                str(policy),
                "--home-root",
                str(home_root),
                "--repo-root",
                str(repo_root),
                "--json",
                *(extra_args or []),
            ],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def write_policy_fixture(
        self,
        policy: pathlib.Path,
        *,
        codex_log_max_bytes: int = 8,
        factory_log_max_bytes: int = 8,
        sessions_ttl_days: int = 14,
        retain_exact_names: list[str] | None = None,
        remove_exact_names: list[str] | None = None,
    ) -> None:
        retained = retain_exact_names or ["1.96.0-aarch64-apple-darwin"]
        removed = remove_exact_names or []
        policy.write_text(
            textwrap.dedent(
                f"""\
                schema_version = 1

                [codex.log]
                path_family = "~/.codex/log/codex-tui.log"
                category = "AI agent"
                growth_shape = "single_file"
                owner = "owned"
                native_policy = "partial"
                cleanup_mode = "rotate"
                max_bytes = {codex_log_max_bytes}
                retained_rotations = 2
                active_writer_processes = ["codex", "codex-tui"]

                [codex.sessions]
                path_family = "~/.codex/sessions/**/*.jsonl"
                category = "AI agent"
                growth_shape = "many_files"
                owner = "owned"
                native_policy = "none_found"
                cleanup_mode = "ttl_prune"
                ttl_days = {sessions_ttl_days}
                active_writer_processes = ["codex", "codex-tui"]

                [codex.sqlite]
                path_family = "~/.codex/logs_2.sqlite*"
                category = "AI agent"
                growth_shape = "sqlite_with_wal"
                owner = "report_only"
                native_policy = "none_found"
                cleanup_mode = "none"

                [codex.archived_sessions]
                path_family = "~/.codex/archived_sessions/**"
                category = "AI agent"
                growth_shape = "tree"
                owner = "report_only"
                native_policy = "none_found"
                cleanup_mode = "none"

                [native_guidance.codex_history]
                path_family = "~/.codex/history.jsonl"
                category = "AI agent"
                growth_shape = "single_file"
                owner = "report_only"
                native_policy = "yes"
                cleanup_mode = "none"
                max_bytes = 10
                persistence = "save-all"

                [factory.log]
                path_family = "~/.factory/logs/droid-log-single.log"
                category = "AI agent"
                growth_shape = "single_file"
                owner = "owned"
                native_policy = "none_found"
                cleanup_mode = "rotate"
                max_bytes = {factory_log_max_bytes}
                retained_rotations = 2
                active_writer_processes = ["factory", "droid"]

                [rustup.toolchains]
                path_family = "~/.rustup/toolchains/*"
                category = "version manager"
                growth_shape = "tree"
                owner = "owned"
                native_policy = "yes"
                cleanup_mode = "toolchain_retention"
                retain_exact_names = {json.dumps(retained)}
                remove_exact_names = {json.dumps(removed)}

                [preflight]
                free_disk_warning_bytes = 100
                free_disk_error_bytes = 50
                owned_storage_warning_bytes = 100
                owned_storage_error_bytes = 200

                [adjacent.browser_cache]
                id = "browser.cache"
                path_family = "~/Library/Caches"
                category = "browser tooling"
                growth_shape = "tree"
                owner = "out_of_scope"
                native_policy = "not_applicable"
                cleanup_mode = "none"

                [adjacent.codex_plugins]
                id = "codex.plugins"
                path_family = "~/.codex/plugins"
                category = "MCP/plugin"
                growth_shape = "tree"
                owner = "out_of_scope"
                native_policy = "not_applicable"
                cleanup_mode = "none"

                [adjacent.package_manager_cache]
                id = "package_manager.cache"
                path_family = "~/.npm"
                category = "package manager"
                growth_shape = "tree"
                owner = "out_of_scope"
                native_policy = "not_applicable"
                cleanup_mode = "none"
                """
            ),
            encoding="utf-8",
        )

    def test_status_reports_required_policy_surfaces(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            home_root.mkdir()
            repo_root.mkdir()

            result = self.run_tool("status", home_root, repo_root)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        surface_ids = {entry["id"] for entry in payload["surfaces"]}
        adjacent_ids = {entry["id"] for entry in payload["adjacent_surfaces"]}

        self.assertTrue(
            {
                "codex.log",
                "codex.sessions",
                "native_guidance.codex_history",
                "codex.sqlite",
                "codex.archived_sessions",
                "factory.log",
                "rustup.toolchains",
            }
            <= surface_ids
        )
        self.assertTrue({"browser.cache", "codex.plugins", "package_manager.cache"} <= adjacent_ids)

        mutable = {
            entry["id"]: entry["active_writer_processes"]
            for entry in payload["surfaces"]
            if entry["cleanup_mode"] in {"rotate", "ttl_prune"}
        }
        self.assertEqual(mutable["codex.log"], ["codex", "codex-tui"])
        self.assertEqual(mutable["codex.sessions"], ["codex", "codex-tui"])
        self.assertEqual(mutable["factory.log"], ["factory", "droid"])

    def test_status_loads_primary_surfaces_from_policy_tables(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            home_root.mkdir()
            repo_root.mkdir()
            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8")
                + textwrap.dedent(
                    """\

                    [codex.extra_report]
                    path_family = "~/.codex/extra-report.jsonl"
                    category = "AI agent"
                    growth_shape = "single_file"
                    owner = "report_only"
                    native_policy = "none_found"
                    cleanup_mode = "none"
                    """
                ),
                encoding="utf-8",
            )

            result = self.run_tool("status", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        surface_ids = {entry["id"] for entry in payload["surfaces"]}
        self.assertIn("codex.extra_report", surface_ids)

    def test_load_policy_rejects_unsupported_schema_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            self.write_policy_fixture(policy_path)
            policy_path.write_text(
                policy_path.read_text(encoding="utf-8").replace("schema_version = 1", "schema_version = 2", 1),
                encoding="utf-8",
            )

            tool = self.load_tool_module()

            with self.assertRaisesRegex(tool.PolicyError, "unsupported schema_version"):
                tool.load_policy(policy_path)

    def test_load_policy_rejects_boolean_schema_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            self.write_policy_fixture(policy_path)
            policy_path.write_text(
                policy_path.read_text(encoding="utf-8").replace("schema_version = 1", "schema_version = true", 1),
                encoding="utf-8",
            )

            tool = self.load_tool_module()

            with self.assertRaisesRegex(tool.PolicyError, "schema_version must be an integer"):
                tool.load_policy(policy_path)

    def test_load_policy_rejects_oversized_policy_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            tool = self.load_tool_module()
            policy_path.write_bytes(b"x" * (tool.MAX_POLICY_BYTES + 1))

            with self.assertRaisesRegex(tool.PolicyError, "policy file exceeds"):
                tool.load_policy(policy_path)

    def test_dry_run_reports_oversized_log_rotation_candidates_without_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            factory_log = home_root / ".factory" / "logs" / "droid-log-single.log"
            codex_log.parent.mkdir(parents=True)
            factory_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex-log-data")
            factory_log.write_bytes(b"factory-log-data")
            repo_root.mkdir()

            before = {codex_log: codex_log.read_bytes(), factory_log: factory_log.read_bytes()}
            result = self.run_tool("dry-run", home_root, repo_root, policy)
            after = {codex_log: codex_log.read_bytes(), factory_log: factory_log.read_bytes()}

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        candidates = {(entry["surface_id"], pathlib.Path(entry["path"]).name): entry for entry in payload["candidates"]}

        self.assertEqual(candidates[("codex.log", "codex-tui.log")]["action"], "rotate")
        self.assertEqual(candidates[("codex.log", "codex-tui.log")]["reason"], "size_exceeds_max_bytes")
        self.assertEqual(candidates[("codex.log", "codex-tui.log")]["estimated_reclaim_bytes"], 0)
        self.assertEqual(candidates[("factory.log", "droid-log-single.log")]["action"], "rotate")
        self.assertEqual(candidates[("factory.log", "droid-log-single.log")]["reason"], "size_exceeds_max_bytes")
        self.assertEqual(candidates[("factory.log", "droid-log-single.log")]["estimated_reclaim_bytes"], 0)

    def test_dry_run_refuses_log_rotation_when_retained_sidecar_is_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            sidecar_target = tmp_path / "sidecar-target.log"
            sidecar_target.write_bytes(b"outside")
            codex_log.with_name("codex-tui.log.1").symlink_to(sidecar_target)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        codex_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"
        ]
        self.assertEqual(len(codex_candidates), 1)
        self.assertEqual(codex_candidates[0]["action"], "refuse")
        self.assertEqual(codex_candidates[0]["reason"], "symlink_not_followed")
        self.assertEqual(pathlib.Path(codex_candidates[0]["path"]).name, "codex-tui.log.1")

    def test_dry_run_refuses_log_rotation_when_extra_sidecar_is_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            sidecar_target = tmp_path / "sidecar-target.log"
            sidecar_target.write_bytes(b"outside")
            codex_log.with_name("codex-tui.log.3").symlink_to(sidecar_target)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        codex_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"
        ]
        self.assertEqual(len(codex_candidates), 1)
        self.assertEqual(codex_candidates[0]["action"], "refuse")
        self.assertEqual(codex_candidates[0]["reason"], "symlink_not_followed")
        self.assertEqual(pathlib.Path(codex_candidates[0]["path"]).name, "codex-tui.log.3")

    def test_dry_run_refuses_log_rotation_when_log_directory_is_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            target_log_root = home_root / "relocated" / "codex-log"
            target_log_root.mkdir(parents=True)
            codex_log = target_log_root / "codex-tui.log"
            codex_log.write_bytes(b"codex log requiring rotation")
            log_root = home_root / ".codex" / "log"
            log_root.parent.mkdir(parents=True)
            log_root.symlink_to(target_log_root, target_is_directory=True)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        codex_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"
        ]
        self.assertEqual(len(codex_candidates), 1)
        self.assertEqual(codex_candidates[0]["action"], "refuse")
        self.assertEqual(codex_candidates[0]["reason"], "symlink_not_followed")
        self.assertEqual(pathlib.Path(codex_candidates[0]["path"]).name, "log")

    def test_dry_run_counts_extra_rotation_sidecars_as_reclaimable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            codex_log.with_name("codex-tui.log.1").write_bytes(b"sidecar-one")
            retained_oldest = b"sidecar-two"
            extra_sidecar = b"sidecar-three"
            codex_log.with_name("codex-tui.log.2").write_bytes(retained_oldest)
            codex_log.with_name("codex-tui.log.3").write_bytes(extra_sidecar)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        codex_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"
        ]
        self.assertEqual(len(codex_candidates), 1)
        self.assertEqual(
            codex_candidates[0]["estimated_reclaim_bytes"],
            len(retained_oldest) + len(extra_sidecar),
        )
        measurements = {
            entry["surface_id"]: entry for entry in payload["surface_measurements"]
        }
        self.assertEqual(measurements["codex.log"]["path_count"], 4)
        self.assertEqual(
            measurements["codex.log"]["bytes"],
            len(b"codex log requiring rotation")
            + len(b"sidecar-one")
            + len(retained_oldest)
            + len(extra_sidecar),
        )

    def test_dry_run_reports_extra_rotation_sidecar_when_active_log_is_small(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"ok")
            extra_sidecar = b"sidecar-three"
            codex_log.with_name("codex-tui.log.3").write_bytes(extra_sidecar)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        codex_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"
        ]
        self.assertEqual(len(codex_candidates), 1)
        self.assertEqual(codex_candidates[0]["action"], "delete")
        self.assertEqual(codex_candidates[0]["reason"], "rotation_retention_exceeded")
        self.assertEqual(pathlib.Path(codex_candidates[0]["path"]).name, "codex-tui.log.3")
        self.assertEqual(codex_candidates[0]["estimated_reclaim_bytes"], len(extra_sidecar))

    def test_dry_run_reports_extra_rotation_sidecar_when_active_log_is_missing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            extra_sidecar = b"sidecar-three"
            codex_log.with_name("codex-tui.log.3").write_bytes(extra_sidecar)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        codex_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"
        ]
        self.assertEqual(len(codex_candidates), 1)
        self.assertEqual(codex_candidates[0]["action"], "delete")
        self.assertEqual(codex_candidates[0]["reason"], "rotation_retention_exceeded")
        self.assertEqual(pathlib.Path(codex_candidates[0]["path"]).name, "codex-tui.log.3")
        self.assertEqual(codex_candidates[0]["estimated_reclaim_bytes"], len(extra_sidecar))

    def test_dry_run_honors_cleanup_mode_none_for_configured_surface(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    'cleanup_mode = "rotate"\nmax_bytes = 8',
                    'cleanup_mode = "none"\nmax_bytes = 8',
                    1,
                ),
                encoding="utf-8",
            )

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        self.assertEqual(
            [entry for entry in payload["candidates"] if entry["surface_id"] == "codex.log"],
            [],
        )

    def test_dry_run_reports_only_stale_codex_session_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, sessions_ttl_days=1)

            sessions = home_root / ".codex" / "sessions"
            old_session = sessions / "2026" / "05" / "old.jsonl"
            new_session = sessions / "2026" / "05" / "new.jsonl"
            old_session.parent.mkdir(parents=True)
            old_session.write_bytes(b"old session")
            new_session.write_bytes(b"new session")
            repo_root.mkdir()

            old_mtime = time.time() - (2 * 24 * 60 * 60)
            os.utime(old_session, (old_mtime, old_mtime))
            before = {old_session: old_session.read_bytes(), new_session: new_session.read_bytes()}
            result = self.run_tool("dry-run", home_root, repo_root, policy)
            after = {old_session: old_session.read_bytes(), new_session: new_session.read_bytes()}

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        sessions_by_name = {
            pathlib.Path(entry["path"]).name: entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "codex.sessions"
        }

        self.assertEqual(set(sessions_by_name), {"old.jsonl"})
        self.assertEqual(sessions_by_name["old.jsonl"]["action"], "delete")
        self.assertEqual(sessions_by_name["old.jsonl"]["reason"], "older_than_ttl_days")
        self.assertEqual(sessions_by_name["old.jsonl"]["estimated_reclaim_bytes"], len(b"old session"))

    def test_dry_run_reports_refusals_for_glob_roots_that_cannot_be_scanned(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            outside_sessions = tmp_path / "outside-sessions"
            outside_sessions.mkdir()
            sessions_root = home_root / ".codex" / "sessions"
            sessions_root.parent.mkdir(parents=True)
            sessions_root.symlink_to(outside_sessions, target_is_directory=True)

            outside_archived = tmp_path / "outside-archived"
            outside_archived.mkdir()
            archived_root = home_root / ".codex" / "archived_sessions"
            archived_root.symlink_to(outside_archived, target_is_directory=True)

            outside_rustup = tmp_path / "outside-rustup-toolchains"
            outside_rustup.mkdir()
            rustup_root = home_root / ".rustup" / "toolchains"
            rustup_root.parent.mkdir(parents=True)
            rustup_root.symlink_to(outside_rustup, target_is_directory=True)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        candidates = {
            entry["surface_id"]: entry
            for entry in payload["candidates"]
            if entry.get("reason") == "symlink_not_followed"
        }
        report_only = {
            entry["surface_id"]: entry
            for entry in payload["report_only"]
            if entry.get("reason") == "symlink_not_followed"
        }
        self.assertEqual(pathlib.Path(candidates["codex.sessions"]["path"]).name, "sessions")
        self.assertEqual(candidates["codex.sessions"]["action"], "refuse")
        self.assertEqual(pathlib.Path(candidates["rustup.toolchains"]["path"]).name, "toolchains")
        self.assertEqual(candidates["rustup.toolchains"]["action"], "refuse")
        self.assertEqual(pathlib.Path(report_only["codex.archived_sessions"]["path"]).name, "archived_sessions")
        self.assertEqual(report_only["codex.archived_sessions"]["estimated_reclaim_bytes"], 0)

    def test_dry_run_reports_symlink_refusal_for_relocated_glob_parent(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            outside_codex = tmp_path / "outside-codex"
            (outside_codex / "sessions").mkdir(parents=True)
            codex_root = home_root / ".codex"
            codex_root.parent.mkdir(parents=True, exist_ok=True)
            codex_root.symlink_to(outside_codex, target_is_directory=True)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        candidates = {
            entry["surface_id"]: entry
            for entry in payload["candidates"]
            if entry.get("reason") == "symlink_not_followed"
        }
        self.assertEqual(pathlib.Path(candidates["codex.sessions"]["path"]).name, ".codex")
        self.assertEqual(candidates["codex.sessions"]["action"], "refuse")

    def test_dry_run_refuses_glob_ancestor_symlinks_within_home_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, remove_exact_names=["old-toolchain"])

            relocated_codex = home_root / "relocated-codex"
            session = relocated_codex / "sessions" / "old.jsonl"
            session.parent.mkdir(parents=True)
            session.write_bytes(b"old session")
            old_mtime = time.time() - (20 * 24 * 60 * 60)
            os.utime(session, (old_mtime, old_mtime))
            codex_root = home_root / ".codex"
            codex_root.parent.mkdir(parents=True, exist_ok=True)
            codex_root.symlink_to(relocated_codex, target_is_directory=True)

            relocated_rustup = home_root / "relocated-rustup"
            toolchain = relocated_rustup / "toolchains" / "old-toolchain"
            toolchain.mkdir(parents=True)
            (toolchain / "bin").mkdir()
            rustup_root = home_root / ".rustup"
            rustup_root.symlink_to(relocated_rustup, target_is_directory=True)
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                "[toolchain]\nchannel = \"stable\"\n",
                encoding="utf-8",
            )

            result = self.run_tool(
                "dry-run",
                home_root,
                repo_root,
                policy,
                [
                    "--active-rustup-toolchain",
                    "stable-aarch64-apple-darwin",
                    "--default-rustup-toolchain",
                    "stable-aarch64-apple-darwin",
                ],
            )

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        candidates = {
            entry["surface_id"]: entry
            for entry in payload["candidates"]
            if entry.get("reason") == "symlink_not_followed"
        }
        self.assertEqual(pathlib.Path(candidates["codex.sessions"]["path"]).name, ".codex")
        self.assertEqual(candidates["codex.sessions"]["action"], "refuse")
        self.assertEqual(pathlib.Path(candidates["rustup.toolchains"]["path"]).name, ".rustup")
        self.assertEqual(candidates["rustup.toolchains"]["action"], "refuse")

    def test_dry_run_does_not_traverse_directory_symlink_inside_glob_surface(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            sessions_root = home_root / ".codex" / "sessions"
            sessions_root.mkdir(parents=True)
            outside_sessions = tmp_path / "outside-sessions"
            outside_sessions.mkdir()
            outside_file = outside_sessions / "old.jsonl"
            outside_file.write_bytes(b"outside")
            stale_time = time.time() - (31 * 24 * 60 * 60)
            os.utime(outside_file, (stale_time, stale_time))
            (sessions_root / "linked").symlink_to(outside_sessions, target_is_directory=True)
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            payload = tool.build_dry_run(policy, home_root, repo_root)

        session_entries = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.sessions"
        ]
        self.assertEqual(session_entries, [])

    def test_dry_run_reports_session_that_disappears_during_scan_as_refusal(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, sessions_ttl_days=1)

            session = home_root / ".codex" / "sessions" / "old.jsonl"
            session.parent.mkdir(parents=True)
            session.write_bytes(b"old session")
            old_mtime = time.time() - (2 * 24 * 60 * 60)
            os.utime(session, (old_mtime, old_mtime))
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_lstat = pathlib.Path.lstat

            def disappearing_lstat(path: pathlib.Path) -> os.stat_result:
                if path == session:
                    session.unlink(missing_ok=True)
                    raise FileNotFoundError(str(path))
                return original_lstat(path)

            pathlib.Path.lstat = disappearing_lstat
            try:
                payload = tool.build_dry_run(policy, home_root, repo_root)
            finally:
                pathlib.Path.lstat = original_lstat

        refusals = [
            entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "codex.sessions" and entry["action"] == "refuse"
        ]
        self.assertEqual(len(refusals), 1)
        self.assertEqual(refusals[0]["reason"], "path_disappeared_during_scan")
        self.assertEqual(refusals[0]["estimated_reclaim_bytes"], 0)

    def test_dry_run_reports_session_that_disappears_during_state_tokening_as_refusal(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, sessions_ttl_days=1)

            session = home_root / ".codex" / "sessions" / "old.jsonl"
            session.parent.mkdir(parents=True)
            session.write_bytes(b"old session")
            old_mtime = time.time() - (2 * 24 * 60 * 60)
            os.utime(session, (old_mtime, old_mtime))
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_lstat = pathlib.Path.lstat
            calls = {"session": 0}

            def disappearing_lstat(path: pathlib.Path) -> os.stat_result:
                if path == session:
                    calls["session"] += 1
                    if calls["session"] >= 2:
                        session.unlink(missing_ok=True)
                        raise FileNotFoundError(str(path))
                return original_lstat(path)

            pathlib.Path.lstat = disappearing_lstat
            try:
                payload = tool.build_dry_run(policy, home_root, repo_root)
            finally:
                pathlib.Path.lstat = original_lstat

        refusals = [
            entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "codex.sessions" and entry["action"] == "refuse"
        ]
        self.assertEqual(len(refusals), 1)
        self.assertEqual(refusals[0]["reason"], "path_disappeared_during_scan")
        self.assertEqual(refusals[0]["estimated_reclaim_bytes"], 0)

    def test_dry_run_reports_codex_sqlite_files_as_report_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            sqlite = home_root / ".codex" / "logs_2.sqlite"
            sqlite_wal = home_root / ".codex" / "logs_2.sqlite-wal"
            sqlite.parent.mkdir(parents=True)
            sqlite.write_bytes(b"sqlite")
            sqlite_wal.write_bytes(b"sqlite-wal")
            repo_root.mkdir()

            before = {sqlite: sqlite.read_bytes(), sqlite_wal: sqlite_wal.read_bytes()}
            result = self.run_tool("dry-run", home_root, repo_root, policy)
            after = {sqlite: sqlite.read_bytes(), sqlite_wal: sqlite_wal.read_bytes()}

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        sqlite_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.sqlite"
        ]
        report_only = {
            pathlib.Path(entry["path"]).name: entry
            for entry in payload["report_only"]
            if entry["surface_id"] == "codex.sqlite"
        }

        self.assertEqual(sqlite_candidates, [])
        self.assertEqual(set(report_only), {"logs_2.sqlite", "logs_2.sqlite-wal"})
        self.assertEqual(report_only["logs_2.sqlite"]["reason"], "report_only_policy")
        self.assertEqual(report_only["logs_2.sqlite-wal"]["reason"], "report_only_policy")

    def test_dry_run_reports_report_only_file_that_disappears_during_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            sqlite = home_root / ".codex" / "logs_2.sqlite"
            sqlite.parent.mkdir(parents=True)
            sqlite.write_bytes(b"sqlite")
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_lstat = pathlib.Path.lstat

            def disappearing_lstat(path: pathlib.Path) -> os.stat_result:
                if path == sqlite:
                    sqlite.unlink(missing_ok=True)
                    raise FileNotFoundError(str(path))
                return original_lstat(path)

            pathlib.Path.lstat = disappearing_lstat
            try:
                payload = tool.build_dry_run(policy, home_root, repo_root)
            finally:
                pathlib.Path.lstat = original_lstat

        reports = [
            entry
            for entry in payload["report_only"]
            if entry["surface_id"] == "codex.sqlite"
        ]
        self.assertEqual(len(reports), 1)
        self.assertEqual(reports[0]["reason"], "path_disappeared_during_scan")
        self.assertEqual(reports[0]["estimated_reclaim_bytes"], 0)

    def test_dry_run_reports_codex_history_native_guidance_as_report_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            history = home_root / ".codex" / "history.jsonl"
            history.parent.mkdir(parents=True)
            history.write_bytes(b"history rows that exceed the native guidance cap")
            repo_root.mkdir()

            before = history.read_bytes()
            result = self.run_tool("dry-run", home_root, repo_root, policy)
            after = history.read_bytes()

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        history_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "native_guidance.codex_history"
        ]
        history_reports = [
            entry for entry in payload["report_only"] if entry["surface_id"] == "native_guidance.codex_history"
        ]

        self.assertEqual(history_candidates, [])
        self.assertEqual(len(history_reports), 1)
        self.assertEqual(history_reports[0]["reason"], "native_guidance_report_only")
        self.assertEqual(
            history_reports[0]["native_config"],
            {"max_bytes": 10, "persistence": "save-all"},
        )

    def test_dry_run_reports_fixed_report_only_symlink_as_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            outside_history = tmp_path / "history.jsonl"
            outside_history.write_bytes(b"outside history")
            history = home_root / ".codex" / "history.jsonl"
            history.parent.mkdir(parents=True)
            history.symlink_to(outside_history)
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        history_reports = [
            entry for entry in payload["report_only"] if entry["surface_id"] == "native_guidance.codex_history"
        ]
        self.assertEqual(len(history_reports), 1)
        self.assertEqual(history_reports[0]["reason"], "symlink_not_followed")
        self.assertEqual(pathlib.Path(history_reports[0]["path"]).name, "history.jsonl")

    def test_dry_run_reports_archived_sessions_tree_as_report_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            archive_root = home_root / ".codex" / "archived_sessions"
            transcript = archive_root / "2026" / "05" / "transcript.jsonl"
            transcript.parent.mkdir(parents=True)
            transcript.write_bytes(b"archived transcript")
            repo_root.mkdir()

            before = transcript.read_bytes()
            result = self.run_tool("dry-run", home_root, repo_root, policy)
            after = transcript.read_bytes()

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        archive_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "codex.archived_sessions"
        ]
        archive_reports = [
            entry for entry in payload["report_only"] if entry["surface_id"] == "codex.archived_sessions"
        ]

        self.assertEqual(archive_candidates, [])
        self.assertEqual(len(archive_reports), 1)
        self.assertEqual(pathlib.Path(archive_reports[0]["path"]).name, "archived_sessions")
        self.assertEqual(archive_reports[0]["reason"], "report_only_policy")
        self.assertGreaterEqual(archive_reports[0]["bytes"], len(b"archived transcript"))

    def test_dry_run_rustup_candidates_are_exact_name_removals_with_protections(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        pinned = "1.96.0-aarch64-apple-darwin"
        retained = "retained-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"
        unlisted = "unlisted-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(
                policy,
                retain_exact_names=[pinned, retained],
                remove_exact_names=[active, default, pinned, retained, removable],
            )

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, pinned, retained, removable, unlisted):
                toolchain_dir = toolchains / name
                toolchain_dir.mkdir(parents=True)
                (toolchain_dir / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                textwrap.dedent(
                    """\
                    [toolchain]
                    channel = "1.96.0"
                    """
                ),
                encoding="utf-8",
            )

            result = self.run_tool(
                "dry-run",
                home_root,
                repo_root,
                policy,
                [
                    "--active-rustup-toolchain",
                    active,
                    "--default-rustup-toolchain",
                    default,
                ],
            )
            remaining_after_dry_run = sorted(path.name for path in toolchains.iterdir())

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(remaining_after_dry_run, sorted([active, default, pinned, retained, removable, unlisted]))
        payload = json.loads(result.stdout)
        rustup_candidates = [
            entry for entry in payload["candidates"] if entry["surface_id"] == "rustup.toolchains"
        ]
        protected = {
            pathlib.Path(entry["path"]).name: entry["reason"]
            for entry in payload["protected"]
            if entry["surface_id"] == "rustup.toolchains"
        }

        self.assertEqual([pathlib.Path(entry["path"]).name for entry in rustup_candidates], [removable])
        self.assertEqual(rustup_candidates[0]["action"], "remove_tree")
        self.assertEqual(rustup_candidates[0]["reason"], "exact_name_remove_policy")
        self.assertGreater(rustup_candidates[0]["estimated_reclaim_bytes"], 0)
        self.assertEqual(protected[active], "active_toolchain")
        self.assertEqual(protected[default], "default_toolchain")
        self.assertEqual(protected[pinned], "project_pinned_toolchain")
        self.assertEqual(protected[retained], "exact_name_retain_policy")
        self.assertEqual(protected[unlisted], "not_in_remove_exact_names")

    def test_dry_run_reports_rustup_toolchain_lstat_disappearance_as_refusal(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, remove_exact_names=[removable])

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, removable):
                toolchain = toolchains / name
                toolchain.mkdir(parents=True)
                (toolchain / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                textwrap.dedent(
                    """\
                    [toolchain]
                    channel = "1.96.0"
                    """
                ),
                encoding="utf-8",
            )

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_entry_lstat = tool._entry_lstat
            missing_marker = toolchains / removable / "marker"

            def disappearing_entry_lstat(entry: os.DirEntry[str]) -> os.stat_result:
                if pathlib.Path(entry.path) == missing_marker:
                    missing_marker.unlink(missing_ok=True)
                    raise FileNotFoundError(entry.path)
                return original_entry_lstat(entry)

            tool._entry_lstat = disappearing_entry_lstat
            try:
                payload = tool.build_dry_run(
                    policy,
                    home_root,
                    repo_root,
                    active_rustup_toolchains=(active,),
                    default_rustup_toolchains=(default,),
                )
            finally:
                tool._entry_lstat = original_entry_lstat

        refusals = [
            entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "rustup.toolchains" and entry["action"] == "refuse"
        ]
        self.assertEqual(len(refusals), 1)
        self.assertEqual(refusals[0]["reason"], "path_disappeared_during_scan")
        self.assertEqual(refusals[0]["estimated_reclaim_bytes"], 0)

    def test_dry_run_reports_rustup_toolchains_root_iterdir_failure_as_refusal(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            toolchains = home_root / ".rustup" / "toolchains"
            toolchains.mkdir(parents=True)
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_iterdir = pathlib.Path.iterdir

            def failing_iterdir(path: pathlib.Path):
                if path == toolchains:
                    raise PermissionError(str(path))
                return original_iterdir(path)

            pathlib.Path.iterdir = failing_iterdir
            try:
                payload = tool.build_dry_run(policy, home_root, repo_root)
            finally:
                pathlib.Path.iterdir = original_iterdir

        refusals = [
            entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "rustup.toolchains" and entry["action"] == "refuse"
        ]
        self.assertEqual(len(refusals), 1)
        self.assertEqual(pathlib.Path(refusals[0]["path"]).name, "toolchains")
        self.assertEqual(refusals[0]["reason"], "path_disappeared_during_scan")
        self.assertEqual(refusals[0]["estimated_reclaim_bytes"], 0)

    def test_dry_run_reports_rustup_toolchain_that_disappears_during_measurement_as_refusal(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, remove_exact_names=[removable])

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, removable):
                toolchain = toolchains / name
                toolchain.mkdir(parents=True)
                (toolchain / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                textwrap.dedent(
                    """\
                    [toolchain]
                    channel = "1.96.0"
                    """
                ),
                encoding="utf-8",
            )

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_measurement = tool._measurement_and_state_token

            def disappearing_measurement(path: pathlib.Path) -> tuple[int, str]:
                if path == toolchains / removable:
                    raise FileNotFoundError(str(path))
                return original_measurement(path)

            tool._measurement_and_state_token = disappearing_measurement
            try:
                payload = tool.build_dry_run(
                    policy,
                    home_root,
                    repo_root,
                    active_rustup_toolchains=(active,),
                    default_rustup_toolchains=(default,),
                )
            finally:
                tool._measurement_and_state_token = original_measurement

        refusals = [
            entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "rustup.toolchains" and entry["action"] == "refuse"
        ]
        self.assertEqual(len(refusals), 1)
        self.assertEqual(refusals[0]["reason"], "path_disappeared_during_scan")
        self.assertEqual(refusals[0]["estimated_reclaim_bytes"], 0)

    def test_dry_run_measures_rustup_toolchain_without_path_rglob(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, remove_exact_names=[removable])

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, removable):
                toolchain = toolchains / name
                (toolchain / "nested").mkdir(parents=True)
                (toolchain / "nested" / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                textwrap.dedent(
                    """\
                    [toolchain]
                    channel = "1.96.0"
                    """
                ),
                encoding="utf-8",
            )

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_rglob = pathlib.Path.rglob

            def refusing_rglob(path: pathlib.Path, pattern: str) -> object:
                if path == toolchains / removable:
                    raise AssertionError("rustup removal measurement used Path.rglob")
                return original_rglob(path, pattern)

            with mock.patch.object(pathlib.Path, "rglob", refusing_rglob):
                payload = tool.build_dry_run(
                    policy,
                    home_root,
                    repo_root,
                    active_rustup_toolchains=(active,),
                    default_rustup_toolchains=(default,),
                )

        removals = [
            entry
            for entry in payload["candidates"]
            if entry["surface_id"] == "rustup.toolchains" and entry["action"] == "remove_tree"
        ]
        self.assertEqual(len(removals), 1)
        self.assertGreater(removals[0]["bytes"], 0)
        self.assertIn("state_token", removals[0])

    def test_dry_run_does_not_measure_internal_directory_symlink_targets(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"
        outside_payload = b"outside-target" * 1024

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, remove_exact_names=[removable])

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, removable):
                toolchain = toolchains / name
                toolchain.mkdir(parents=True)
                (toolchain / "marker").write_bytes(name.encode("utf-8"))
            outside_tree = tmp_path / "outside-tree"
            outside_tree.mkdir()
            outside_file = outside_tree / "large.bin"
            outside_file.write_bytes(outside_payload)
            (toolchains / removable / "linked-tree").symlink_to(
                outside_tree,
                target_is_directory=True,
            )
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                textwrap.dedent(
                    """\
                    [toolchain]
                    channel = "1.96.0"
                    """
                ),
                encoding="utf-8",
            )

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            first = tool.build_dry_run(
                policy,
                home_root,
                repo_root,
                active_rustup_toolchains=(active,),
                default_rustup_toolchains=(default,),
            )
            outside_file.write_bytes(outside_payload + b"changed")
            second = tool.build_dry_run(
                policy,
                home_root,
                repo_root,
                active_rustup_toolchains=(active,),
                default_rustup_toolchains=(default,),
            )

        first_removal = next(
            entry
            for entry in first["candidates"]
            if entry["surface_id"] == "rustup.toolchains" and entry["action"] == "remove_tree"
        )
        second_removal = next(
            entry
            for entry in second["candidates"]
            if entry["surface_id"] == "rustup.toolchains" and entry["action"] == "remove_tree"
        )
        self.assertLess(first_removal["bytes"], len(outside_payload))
        self.assertEqual(first_removal["bytes"], second_removal["bytes"])
        self.assertEqual(first_removal["state_token"], second_removal["state_token"])

    def test_dry_run_fails_closed_for_rustup_removals_without_active_default_snapshots(self) -> None:
        active = "active-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, remove_exact_names=[active])

            toolchain = home_root / ".rustup" / "toolchains" / active
            toolchain.mkdir(parents=True)
            (toolchain / "marker").write_bytes(b"active")
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("active/default rustup snapshots are required", result.stderr)

    def test_dry_run_fails_closed_for_rustup_removals_without_repo_toolchain_pin(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, remove_exact_names=[removable])

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, removable):
                toolchain = toolchains / name
                toolchain.mkdir(parents=True)
                (toolchain / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()

            result = self.run_tool(
                "dry-run",
                home_root,
                repo_root,
                policy,
                [
                    "--active-rustup-toolchain",
                    active,
                    "--default-rustup-toolchain",
                    default,
                ],
            )

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("rust-toolchain.toml", result.stderr)

    def test_dry_run_rejects_oversized_repo_toolchain_pin(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, remove_exact_names=[removable])

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, removable):
                toolchain = toolchains / name
                toolchain.mkdir(parents=True)
                (toolchain / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()
            tool = self.load_tool_module()
            (repo_root / "rust-toolchain.toml").write_bytes(
                b"x" * (tool.MAX_RUST_TOOLCHAIN_BYTES + 1)
            )

            result = self.run_tool(
                "dry-run",
                home_root,
                repo_root,
                policy,
                [
                    "--active-rustup-toolchain",
                    active,
                    "--default-rustup-toolchain",
                    default,
                ],
            )

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("rust-toolchain.toml exceeds", result.stderr)

    def test_policy_validation_fails_closed_when_mutable_active_writers_missing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            home_root.mkdir()
            repo_root.mkdir()
            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    'active_writer_processes = ["codex", "codex-tui"]\n\n[codex.sessions]',
                    "\n[codex.sessions]",
                    1,
                ),
                encoding="utf-8",
            )

            result = self.run_tool("status", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("codex.log.active_writer_processes", result.stderr)

    def test_policy_validation_fails_closed_when_mode_fields_are_missing(self) -> None:
        cases = (
            ("max_bytes = 8\n", "codex.log.max_bytes"),
            ("retained_rotations = 2\n", "codex.log.retained_rotations"),
            ("ttl_days = 14\n", "codex.sessions.ttl_days"),
            (
                'retain_exact_names = ["1.96.0-aarch64-apple-darwin"]\n',
                "rustup.toolchains.retain_exact_names",
            ),
            ("remove_exact_names = []\n", "rustup.toolchains.remove_exact_names"),
        )
        for removed_line, expected_error in cases:
            with self.subTest(expected_error=expected_error):
                with tempfile.TemporaryDirectory() as tmp:
                    tmp_path = pathlib.Path(tmp)
                    policy = tmp_path / "policy.toml"
                    home_root = tmp_path / "home"
                    repo_root = tmp_path / "repo"
                    home_root.mkdir()
                    repo_root.mkdir()
                    self.write_policy_fixture(policy)
                    policy.write_text(
                        policy.read_text(encoding="utf-8").replace(removed_line, "", 1),
                        encoding="utf-8",
                    )

                    result = self.run_tool("status", home_root, repo_root, policy)

                self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
                self.assertIn(expected_error, result.stderr)

    def test_policy_validation_fails_closed_for_unknown_owner_or_cleanup_mode(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            home_root.mkdir()
            repo_root.mkdir()

            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace('owner = "owned"', 'owner = "ownd"', 1),
                encoding="utf-8",
            )
            owner_result = self.run_tool("status", home_root, repo_root, policy)

            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    'cleanup_mode = "rotate"',
                    'cleanup_mode = "rotte"',
                    1,
                ),
                encoding="utf-8",
            )
            mode_result = self.run_tool("status", home_root, repo_root, policy)

            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    'owner = "owned"',
                    'owner = "report_only"',
                    1,
                ),
                encoding="utf-8",
            )
            combo_result = self.run_tool("status", home_root, repo_root, policy)

            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    'owner = "out_of_scope"',
                    'owner = "owned"',
                    1,
                ),
                encoding="utf-8",
            )
            adjacent_owner_result = self.run_tool("status", home_root, repo_root, policy)

        self.assertEqual(owner_result.returncode, 2, (owner_result.stdout, owner_result.stderr))
        self.assertIn("codex.log.owner", owner_result.stderr)
        self.assertEqual(mode_result.returncode, 2, (mode_result.stdout, mode_result.stderr))
        self.assertIn("codex.log.cleanup_mode", mode_result.stderr)
        self.assertEqual(combo_result.returncode, 2, (combo_result.stdout, combo_result.stderr))
        self.assertIn("codex.log.owner/cleanup_mode", combo_result.stderr)
        self.assertEqual(
            adjacent_owner_result.returncode,
            2,
            (adjacent_owner_result.stdout, adjacent_owner_result.stderr),
        )
        self.assertIn("browser.cache.owner/cleanup_mode", adjacent_owner_result.stderr)

    def test_policy_validation_fails_closed_when_threshold_ordering_is_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            home_root.mkdir()
            repo_root.mkdir()
            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    "free_disk_warning_bytes = 100\nfree_disk_error_bytes = 50",
                    "free_disk_warning_bytes = 50\nfree_disk_error_bytes = 100",
                    1,
                ),
                encoding="utf-8",
            )

            result = self.run_tool("status", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("free_disk_error_bytes", result.stderr)

    def test_policy_validation_fails_closed_when_threshold_values_are_negative_or_bool(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            home_root.mkdir()
            repo_root.mkdir()
            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    "owned_storage_warning_bytes = 100",
                    "owned_storage_warning_bytes = true",
                    1,
                ),
                encoding="utf-8",
            )

            bool_result = self.run_tool("status", home_root, repo_root, policy)

            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    "owned_storage_warning_bytes = 100",
                    "owned_storage_warning_bytes = -1",
                    1,
                ),
                encoding="utf-8",
            )

            negative_result = self.run_tool("status", home_root, repo_root, policy)

        self.assertEqual(bool_result.returncode, 2, (bool_result.stdout, bool_result.stderr))
        self.assertIn("owned_storage_warning_bytes", bool_result.stderr)
        self.assertEqual(negative_result.returncode, 2, (negative_result.stdout, negative_result.stderr))
        self.assertIn("owned_storage_warning_bytes", negative_result.stderr)

    def test_policy_validation_fails_closed_when_cleanup_integer_is_bool(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)
            policy.write_text(
                policy.read_text(encoding="utf-8").replace("max_bytes = 8", "max_bytes = true", 1),
                encoding="utf-8",
            )

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("codex.log.max_bytes", result.stderr)

    def test_dry_run_output_includes_measurements_refusals_and_adjacent_context(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, sessions_ttl_days=1)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"large codex log")

            outside_target = tmp_path / "outside-session.jsonl"
            outside_target.write_bytes(b"outside")
            session_link = home_root / ".codex" / "sessions" / "link.jsonl"
            session_link.parent.mkdir(parents=True)
            session_link.symlink_to(outside_target)

            browser_cache = home_root / "Library" / "Caches" / "browser-cache"
            codex_plugin = home_root / ".codex" / "plugins" / "plugin-cache"
            npm_cache = home_root / ".npm" / "cache-entry"
            for path in (browser_cache, codex_plugin, npm_cache):
                path.parent.mkdir(parents=True)
                path.write_bytes(path.name.encode("utf-8"))
            repo_root.mkdir()

            result = self.run_tool("dry-run", home_root, repo_root, policy)

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        self.assertEqual(payload["policy_path"], str(policy))
        self.assertEqual(payload["evaluated_root"], str(home_root))

        measurements = {entry["surface_id"]: entry for entry in payload["surface_measurements"]}
        self.assertGreaterEqual(measurements["codex.log"]["bytes"], len(b"large codex log"))
        self.assertTrue(measurements["codex.log"]["cleanup_eligible"])
        self.assertFalse(measurements["codex.sqlite"]["cleanup_eligible"])

        refusals = {
            pathlib.Path(entry["path"]).name: entry
            for entry in payload["candidates"]
            if entry["action"] == "refuse"
        }
        self.assertEqual(refusals["link.jsonl"]["reason"], "symlink_not_followed")
        self.assertEqual(refusals["link.jsonl"]["estimated_reclaim_bytes"], 0)

        adjacent = {entry["surface_id"]: entry for entry in payload["adjacent_context"]}
        self.assertEqual(set(adjacent), {"browser.cache", "codex.plugins", "package_manager.cache"})
        self.assertEqual(adjacent["browser.cache"]["owner"], "out_of_scope")
        self.assertGreater(adjacent["codex.plugins"]["bytes"], 0)

    def test_preflight_reports_threshold_errors_and_warnings_without_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"x" * 120)
            repo_root.mkdir()

            before = codex_log.read_bytes()
            result = self.run_tool(
                "preflight",
                home_root,
                repo_root,
                policy,
                ["--available-disk-bytes", "40"],
            )
            after = codex_log.read_bytes()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        self.assertTrue(payload["read_only"])
        self.assertEqual(payload["mode"], "preflight")
        self.assertEqual(payload["status"], "error")
        self.assertIn("free_disk_below_error", payload["errors"])
        self.assertIn("owned_storage_above_warning", payload["warnings"])
        self.assertEqual(payload["available_disk_bytes"], 40)
        self.assertGreaterEqual(payload["owned_storage_bytes"], 120)

    def test_preflight_fails_closed_when_owned_surface_measurement_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)
            policy_path.write_text(
                policy_path.read_text(encoding="utf-8").replace(
                    'cleanup_mode = "rotate"\nmax_bytes = 8',
                    'cleanup_mode = "none"\nmax_bytes = 8',
                    1,
                ),
                encoding="utf-8",
            )

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"unreadable during measurement")
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_lstat = pathlib.Path.lstat

            def failing_lstat(path: pathlib.Path) -> os.stat_result:
                if path == codex_log:
                    raise PermissionError(str(path))
                return original_lstat(path)

            pathlib.Path.lstat = failing_lstat
            try:
                payload = tool.build_preflight(
                    policy,
                    home_root,
                    repo_root,
                    available_disk_bytes=1000,
                )
            finally:
                pathlib.Path.lstat = original_lstat

        self.assertEqual(payload["status"], "error")
        self.assertIn("owned_storage_measurement_failed", payload["errors"])
        measurements = {entry["surface_id"]: entry for entry in payload["surface_measurements"]}
        self.assertEqual(measurements["codex.log"]["measurement_errors"][0]["reason"], "measurement_failed")

    def test_preflight_fails_closed_when_owned_glob_root_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            outside_sessions = tmp_path / "outside-sessions"
            outside_sessions.mkdir()
            sessions_root = home_root / ".codex" / "sessions"
            sessions_root.parent.mkdir(parents=True)
            sessions_root.symlink_to(outside_sessions, target_is_directory=True)
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            payload = tool.build_preflight(
                policy,
                home_root,
                repo_root,
                available_disk_bytes=1000,
            )

        self.assertEqual(payload["status"], "error")
        self.assertIn("owned_storage_measurement_failed", payload["errors"])
        root_errors = [
            entry
            for entry in payload["owned_storage_measurement_errors"]
            if entry.get("surface_id") == "codex.sessions"
        ]
        self.assertEqual(len(root_errors), 1)
        self.assertEqual(pathlib.Path(root_errors[0]["path"]).name, "sessions")
        self.assertEqual(root_errors[0]["reason"], "symlink_not_followed")

    def test_preflight_fails_closed_when_owned_glob_ancestor_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            relocated_codex = home_root / "relocated-codex"
            (relocated_codex / "sessions").mkdir(parents=True)
            codex_root = home_root / ".codex"
            codex_root.parent.mkdir(parents=True, exist_ok=True)
            codex_root.symlink_to(relocated_codex, target_is_directory=True)
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            payload = tool.build_preflight(
                policy,
                home_root,
                repo_root,
                available_disk_bytes=1000,
            )

        self.assertEqual(payload["status"], "error")
        self.assertIn("owned_storage_measurement_failed", payload["errors"])
        root_errors = [
            entry
            for entry in payload["owned_storage_measurement_errors"]
            if entry.get("surface_id") == "codex.sessions"
        ]
        self.assertEqual(len(root_errors), 1)
        self.assertEqual(pathlib.Path(root_errors[0]["path"]).name, ".codex")
        self.assertEqual(root_errors[0]["reason"], "symlink_not_followed")

    def test_preflight_fails_closed_when_owned_fixed_path_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            outside_log_root = tmp_path / "outside-log-root"
            outside_log_root.mkdir()
            log_root = home_root / ".codex" / "log"
            log_root.parent.mkdir(parents=True)
            log_root.symlink_to(outside_log_root, target_is_directory=True)
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            payload = tool.build_preflight(
                policy,
                home_root,
                repo_root,
                available_disk_bytes=1000,
            )

        self.assertEqual(payload["status"], "error")
        self.assertIn("owned_storage_measurement_failed", payload["errors"])
        root_errors = [
            entry
            for entry in payload["owned_storage_measurement_errors"]
            if entry.get("surface_id") == "codex.log"
        ]
        self.assertEqual(len(root_errors), 1)
        self.assertEqual(pathlib.Path(root_errors[0]["path"]).name, "log")
        self.assertEqual(root_errors[0]["reason"], "symlink_not_followed")

    def test_preflight_fails_closed_when_owned_cleanup_none_path_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)
            policy_path.write_text(
                policy_path.read_text(encoding="utf-8").replace(
                    'cleanup_mode = "rotate"\nmax_bytes = 8',
                    'cleanup_mode = "none"\nmax_bytes = 8',
                    1,
                ),
                encoding="utf-8",
            )

            outside_log_root = tmp_path / "outside-log-root"
            outside_log_root.mkdir()
            log_root = home_root / ".codex" / "log"
            log_root.parent.mkdir(parents=True)
            log_root.symlink_to(outside_log_root, target_is_directory=True)
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            payload = tool.build_preflight(
                policy,
                home_root,
                repo_root,
                available_disk_bytes=1000,
            )

        self.assertEqual(payload["status"], "error")
        self.assertIn("owned_storage_measurement_failed", payload["errors"])
        root_errors = [
            entry
            for entry in payload["owned_storage_measurement_errors"]
            if entry.get("surface_id") == "codex.log"
        ]
        self.assertEqual(len(root_errors), 1)
        self.assertEqual(pathlib.Path(root_errors[0]["path"]).name, "log")
        self.assertEqual(root_errors[0]["reason"], "symlink_not_followed")

    def test_preflight_fails_closed_when_log_sidecar_scan_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"ok")
            repo_root.mkdir()

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_iterdir = pathlib.Path.iterdir

            def failing_iterdir(path: pathlib.Path):
                if path == codex_log.parent:
                    raise PermissionError(str(path))
                return original_iterdir(path)

            pathlib.Path.iterdir = failing_iterdir
            try:
                payload = tool.build_preflight(
                    policy,
                    home_root,
                    repo_root,
                    available_disk_bytes=1000,
                )
            finally:
                pathlib.Path.iterdir = original_iterdir

        self.assertEqual(payload["status"], "error")
        self.assertIn("owned_storage_measurement_failed", payload["errors"])
        root_errors = [
            entry
            for entry in payload["owned_storage_measurement_errors"]
            if entry.get("surface_id") == "codex.log"
        ]
        self.assertEqual(len(root_errors), 1)
        self.assertEqual(pathlib.Path(root_errors[0]["path"]).name, "log")
        self.assertEqual(root_errors[0]["reason"], "path_disappeared_during_scan")

    def test_preflight_reports_adjacent_caches_without_counting_them_as_owned(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            browser_cache = home_root / "Library" / "Caches" / "browser-cache"
            package_cache = home_root / ".npm" / "cache-entry"
            for path in (browser_cache, package_cache):
                path.parent.mkdir(parents=True)
                path.write_bytes(b"x" * 256)
            repo_root.mkdir()

            result = self.run_tool(
                "preflight",
                home_root,
                repo_root,
                policy,
                ["--available-disk-bytes", "1000"],
            )

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "ok")
        self.assertEqual(payload["owned_storage_bytes"], 0)
        adjacent = {entry["surface_id"]: entry for entry in payload["adjacent_context"]}
        self.assertGreater(adjacent["browser.cache"]["bytes"], 0)
        self.assertGreater(adjacent["package_manager.cache"]["bytes"], 0)
        self.assertEqual(set(payload["follow_up_classes"]), {"browser.cache", "package_manager.cache"})

    def test_apply_rotates_log_candidate_from_matching_dry_run_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            current_after = codex_log.read_bytes()
            rotated_after = codex_log.with_name("codex-tui.log.1").read_bytes()

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(current_after, b"")
        self.assertEqual(rotated_after, original)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "applied")
        self.assertEqual(payload["actions_taken"][0]["action"], "rotate")
        self.assertEqual(payload["actions_taken"][0]["surface_id"], "codex.log")

    def test_apply_rejects_oversized_dry_run_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)
            home_root.mkdir()
            repo_root.mkdir()
            tool = self.load_tool_module()
            report.write_bytes(b"x" * (tool.MAX_DRY_RUN_REPORT_BYTES + 1))

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertIn("dry-run report exceeds", result.stderr)

    def test_apply_rotates_log_and_prunes_extra_rotation_sidecars(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            sidecar_one = b"sidecar-one"
            sidecar_two = b"sidecar-two"
            codex_log.write_bytes(original)
            codex_log.with_name("codex-tui.log.1").write_bytes(sidecar_one)
            codex_log.with_name("codex-tui.log.2").write_bytes(sidecar_two)
            extra_sidecar = codex_log.with_name("codex-tui.log.3")
            extra_sidecar.write_bytes(b"sidecar-three")
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            current_after = codex_log.read_bytes()
            rotated_one_after = codex_log.with_name("codex-tui.log.1").read_bytes()
            rotated_two_after = codex_log.with_name("codex-tui.log.2").read_bytes()
            extra_exists_after = extra_sidecar.exists()

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(current_after, b"")
        self.assertEqual(rotated_one_after, original)
        self.assertEqual(rotated_two_after, sidecar_one)
        self.assertFalse(extra_exists_after)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["bytes_reclaimed"], len(sidecar_two) + len(b"sidecar-three"))

    def test_apply_prunes_extra_rotation_sidecar_without_rotating_small_active_log(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"ok"
            extra_sidecar = b"sidecar-three"
            codex_log.write_bytes(original)
            extra_path = codex_log.with_name("codex-tui.log.3")
            extra_path.write_bytes(extra_sidecar)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            current_after = codex_log.read_bytes()
            rotated_exists = codex_log.with_name("codex-tui.log.1").exists()
            extra_exists_after = extra_path.exists()

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(current_after, original)
        self.assertFalse(rotated_exists)
        self.assertFalse(extra_exists_after)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "applied")
        self.assertEqual(payload["actions_taken"][0]["action"], "delete")
        self.assertEqual(payload["bytes_reclaimed"], len(extra_sidecar))

    def test_apply_does_not_rotate_when_retained_sidecar_is_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            sidecar_target = tmp_path / "sidecar-target.log"
            sidecar_target.write_bytes(b"outside")
            sidecar = codex_log.with_name("codex-tui.log.1")
            sidecar.symlink_to(sidecar_target)
            repo_root.mkdir()

            stale_report = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(stale_report.returncode, 0, (stale_report.stdout, stale_report.stderr))
            payload = json.loads(stale_report.stdout)
            payload["candidates"] = [
                {
                    "surface_id": "codex.log",
                    "path": str(codex_log),
                    "action": "rotate",
                    "reason": "size_exceeds_max_bytes",
                    "bytes": len(original),
                    "estimated_reclaim_bytes": 0,
                    "state_token": "stale",
                }
            ]
            report.write_text(json.dumps(payload), encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            current_after = codex_log.read_bytes()
            sidecar_is_symlink = sidecar.is_symlink()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(current_after, original)
        self.assertTrue(sidecar_is_symlink)
        apply_payload = json.loads(result.stdout)
        self.assertEqual(apply_payload["status"], "aborted")
        self.assertEqual(apply_payload["reason"], "candidate_state_changed")
        self.assertEqual(apply_payload["refusal_reasons"][0]["reason"], "symlink_not_followed")

    def test_apply_requires_process_snapshot_for_mutable_writer_surfaces(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report)],
            )
            after = codex_log.read_bytes()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(after, original)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "refused")
        self.assertEqual(payload["reason"], "process_snapshot_required")
        self.assertEqual(payload["actions_taken"], [])

    def test_apply_deletes_stale_session_candidate_from_matching_dry_run_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy, sessions_ttl_days=1)

            session = home_root / ".codex" / "sessions" / "old.jsonl"
            session.parent.mkdir(parents=True)
            session.write_bytes(b"old session")
            old_mtime = time.time() - (2 * 24 * 60 * 60)
            os.utime(session, (old_mtime, old_mtime))
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            exists_after = session.exists()

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertFalse(exists_after)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "applied")
        self.assertEqual(payload["actions_taken"][0]["action"], "delete")
        self.assertEqual(payload["actions_taken"][0]["surface_id"], "codex.sessions")

    def test_apply_rechecks_candidate_state_immediately_before_delete(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, sessions_ttl_days=1)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            session = home_root / ".codex" / "sessions" / "old.jsonl"
            codex_log.parent.mkdir(parents=True)
            session.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            session.write_bytes(b"old session")
            old_mtime = time.time() - (2 * 24 * 60 * 60)
            os.utime(session, (old_mtime, old_mtime))
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy_path)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_rotate = tool._rotate_log
            fresh_session = b"fresh session data"

            def rotate_then_rewrite_session(path: pathlib.Path, retained_rotations: int) -> None:
                original_rotate(path, retained_rotations)
                session.write_bytes(fresh_session)

            tool._rotate_log = rotate_then_rewrite_session
            try:
                payload = tool.build_apply(
                    policy,
                    home_root,
                    repo_root,
                    dry_run_report=report,
                    process_snapshot_supplied=True,
                )
            finally:
                tool._rotate_log = original_rotate
            session_exists_after = session.exists()
            session_after = session.read_bytes() if session_exists_after else None

        self.assertEqual(payload["status"], "aborted")
        self.assertEqual(payload["reason"], "candidate_state_changed")
        self.assertEqual(payload["actions_taken"][0]["action"], "rotate")
        self.assertTrue(session_exists_after)
        self.assertEqual(session_after, fresh_session)

    def test_apply_rechecks_candidate_without_rebuilding_full_dry_run(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path, sessions_ttl_days=1)

            sessions_root = home_root / ".codex" / "sessions"
            sessions_root.mkdir(parents=True)
            for name in ("old-a.jsonl", "old-b.jsonl"):
                session = sessions_root / name
                session.write_bytes(b"old session")
                old_mtime = time.time() - (2 * 24 * 60 * 60)
                os.utime(session, (old_mtime, old_mtime))
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy_path)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_build_dry_run = tool.build_dry_run
            build_dry_run_calls = 0

            def counting_build_dry_run(*args, **kwargs):
                nonlocal build_dry_run_calls
                build_dry_run_calls += 1
                return original_build_dry_run(*args, **kwargs)

            tool.build_dry_run = counting_build_dry_run
            try:
                payload = tool.build_apply(
                    policy,
                    home_root,
                    repo_root,
                    dry_run_report=report,
                    process_snapshot_supplied=True,
                )
            finally:
                tool.build_dry_run = original_build_dry_run

        self.assertEqual(payload["status"], "applied")
        self.assertEqual(len(payload["actions_taken"]), 2)
        self.assertEqual(build_dry_run_calls, 1)

    def test_apply_reports_partial_summary_when_later_mutation_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            factory_log = home_root / ".factory" / "logs" / "droid-log-single.log"
            codex_log.parent.mkdir(parents=True)
            factory_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            factory_log.write_bytes(b"factory log requiring rotation")
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy_path)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_rotate = tool._rotate_log

            def flaky_rotate(path: pathlib.Path, retained_rotations: int) -> None:
                if path.name == "droid-log-single.log":
                    raise OSError("synthetic rotate failure")
                original_rotate(path, retained_rotations)

            tool._rotate_log = flaky_rotate
            try:
                payload = tool.build_apply(
                    policy,
                    home_root,
                    repo_root,
                    dry_run_report=report,
                    process_snapshot_supplied=True,
                )
            finally:
                tool._rotate_log = original_rotate

        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["reason"], "mutation_failed")
        self.assertEqual(payload["actions_taken"][0]["surface_id"], "codex.log")
        self.assertEqual(payload["failed_action"]["surface_id"], "factory.log")

    def test_apply_removes_only_unprotected_exact_name_rustup_candidate(self) -> None:
        active = "active-aarch64-apple-darwin"
        default = "default-aarch64-apple-darwin"
        pinned = "1.96.0-aarch64-apple-darwin"
        retained = "retained-aarch64-apple-darwin"
        removable = "old-aarch64-apple-darwin"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(
                policy,
                retain_exact_names=[pinned, retained],
                remove_exact_names=[active, default, pinned, retained, removable],
            )

            toolchains = home_root / ".rustup" / "toolchains"
            for name in (active, default, pinned, retained, removable):
                toolchain_dir = toolchains / name
                toolchain_dir.mkdir(parents=True)
                (toolchain_dir / "marker").write_bytes(name.encode("utf-8"))
            repo_root.mkdir()
            (repo_root / "rust-toolchain.toml").write_text(
                textwrap.dedent(
                    """\
                    [toolchain]
                    channel = "1.96.0"
                    """
                ),
                encoding="utf-8",
            )
            rustup_args = [
                "--active-rustup-toolchain",
                active,
                "--default-rustup-toolchain",
                default,
            ]

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy, rustup_args)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), *rustup_args],
            )
            remaining_after_apply = sorted(path.name for path in toolchains.iterdir())

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(remaining_after_apply, sorted([active, default, pinned, retained]))
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "applied")
        self.assertEqual(payload["actions_taken"][0]["action"], "remove_tree")
        self.assertEqual(payload["actions_taken"][0]["surface_id"], "rustup.toolchains")

    def test_apply_preserves_and_reports_report_only_codex_surfaces(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            sqlite = home_root / ".codex" / "logs_2.sqlite"
            history = home_root / ".codex" / "history.jsonl"
            archived = home_root / ".codex" / "archived_sessions" / "session.jsonl"
            codex_log.parent.mkdir(parents=True)
            sqlite.parent.mkdir(parents=True, exist_ok=True)
            archived.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            sqlite.write_bytes(b"sqlite")
            history.write_bytes(b"history")
            archived.write_bytes(b"archived")
            repo_root.mkdir()

            before = {sqlite: sqlite.read_bytes(), history: history.read_bytes(), archived: archived.read_bytes()}
            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            after = {sqlite: sqlite.read_bytes(), history: history.read_bytes(), archived: archived.read_bytes()}

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(after, before)
        payload = json.loads(result.stdout)
        skipped = {entry["surface_id"] for entry in payload["skipped_report_only"]}
        self.assertTrue(
            {
                "codex.sqlite",
                "native_guidance.codex_history",
                "codex.archived_sessions",
            }
            <= skipped
        )

    def test_apply_revalidates_policy_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")
            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    'active_writer_processes = ["codex", "codex-tui"]\n\n[codex.sessions]',
                    "\n[codex.sessions]",
                    1,
                ),
                encoding="utf-8",
            )

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            after = codex_log.read_bytes()

        self.assertEqual(result.returncode, 2, (result.stdout, result.stderr))
        self.assertEqual(after, original)
        self.assertIn("codex.log.active_writer_processes", result.stderr)

    def test_apply_rescans_and_aborts_when_candidate_state_changed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            changed = b"changed codex log requiring rotation"
            codex_log.write_bytes(changed)
            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            after = codex_log.read_bytes()
            rotated_exists = codex_log.with_name("codex-tui.log.1").exists()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(after, changed)
        self.assertFalse(rotated_exists)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "aborted")
        self.assertEqual(payload["reason"], "candidate_state_changed")
        self.assertEqual(payload["actions_taken"], [])

    def test_apply_rescans_and_aborts_when_same_size_candidate_state_changed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"same-size-log-a"
            changed = b"same-size-log-b"
            self.assertEqual(len(original), len(changed))
            codex_log.write_bytes(original)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            time.sleep(0.01)
            codex_log.write_bytes(changed)
            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            after = codex_log.read_bytes()
            rotated_exists = codex_log.with_name("codex-tui.log.1").exists()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(after, changed)
        self.assertFalse(rotated_exists)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "aborted")
        self.assertEqual(payload["reason"], "candidate_state_changed")
        self.assertEqual(payload["actions_taken"], [])

    def test_apply_rescans_and_aborts_when_rotation_sidecar_state_changed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            sidecar = codex_log.with_name("codex-tui.log.1")
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            sidecar.write_bytes(b"sidecar-a")
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            time.sleep(0.01)
            sidecar.write_bytes(b"sidecar-b")
            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            log_after = codex_log.read_bytes()
            sidecar_after = sidecar.read_bytes()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(log_after, original)
        self.assertEqual(sidecar_after, b"sidecar-b")
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "aborted")
        self.assertEqual(payload["reason"], "candidate_state_changed")

    def test_apply_rotation_failure_preserves_oldest_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy_path = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy_path)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            sidecar_one = codex_log.with_name("codex-tui.log.1")
            sidecar_two = codex_log.with_name("codex-tui.log.2")
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            sidecar_one.write_bytes(b"sidecar-one")
            sidecar_two.write_bytes(b"sidecar-two")
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy_path)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            tool = self.load_tool_module()
            policy = tool.load_policy(policy_path)
            original_rename = pathlib.Path.rename

            def failing_current_log_rename(path: pathlib.Path, target: pathlib.Path) -> pathlib.Path:
                if path == codex_log:
                    raise OSError("synthetic current log rename failure")
                return original_rename(path, target)

            pathlib.Path.rename = failing_current_log_rename
            try:
                payload = tool.build_apply(
                    policy,
                    home_root,
                    repo_root,
                    dry_run_report=report,
                    process_snapshot_supplied=True,
                )
            finally:
                pathlib.Path.rename = original_rename

            current_after = codex_log.read_bytes() if codex_log.exists() else None
            sidecar_one_after = sidecar_one.read_bytes() if sidecar_one.exists() else None
            sidecar_two_after = sidecar_two.read_bytes() if sidecar_two.exists() else None

        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["reason"], "mutation_failed")
        self.assertEqual(payload["actions_taken"], [])
        self.assertEqual(current_after, b"codex log requiring rotation")
        self.assertEqual(sidecar_one_after, b"sidecar-one")
        self.assertEqual(sidecar_two_after, b"sidecar-two")

    def test_rotate_log_refuses_recreated_rotation_destination(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            codex_log = tmp_path / "home" / ".codex" / "log" / "codex-tui.log"
            sidecar_one = codex_log.with_name("codex-tui.log.1")
            live_sidecar = b"live sidecar recreated during rotation"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")

            tool = self.load_tool_module()
            original_rename = pathlib.Path.rename
            original_link = os.link

            def recreate_destination_before_rename(path: pathlib.Path, target: pathlib.Path) -> pathlib.Path:
                if target == sidecar_one and path.name.startswith(".codex-tui.log.rotate-"):
                    sidecar_one.write_bytes(live_sidecar)
                return original_rename(path, target)

            def recreate_destination_before_link(src: pathlib.Path, dst: pathlib.Path, *args, **kwargs) -> None:
                if pathlib.Path(dst) == sidecar_one and pathlib.Path(src).name.startswith(".codex-tui.log.rotate-"):
                    sidecar_one.write_bytes(live_sidecar)
                return original_link(src, dst, *args, **kwargs)

            pathlib.Path.rename = recreate_destination_before_rename
            os.link = recreate_destination_before_link
            try:
                with self.assertRaises((OSError, tool.PolicyError)):
                    tool._rotate_log(codex_log, 2)
            finally:
                pathlib.Path.rename = original_rename
                os.link = original_link

            sidecar_after = sidecar_one.read_bytes()

        self.assertEqual(sidecar_after, live_sidecar)

    def test_rotate_log_rollback_preserves_recreated_current_log(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            codex_log = tmp_path / "home" / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")

            tool = self.load_tool_module()
            original_create = tool._create_empty_file_no_follow

            def recreate_current_then_fail(path: pathlib.Path, mode: int) -> None:
                if path == codex_log:
                    path.write_bytes(b"live log recreated during rollback")
                    raise OSError("synthetic current log recreation")
                original_create(path, mode)

            tool._create_empty_file_no_follow = recreate_current_then_fail
            try:
                with self.assertRaises(OSError):
                    tool._rotate_log(codex_log, 2)
            finally:
                tool._create_empty_file_no_follow = original_create

            current_after = codex_log.read_bytes()

        self.assertEqual(current_after, b"live log recreated during rollback")

    def test_rotate_log_refuses_current_log_reappearing_as_broken_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            codex_log = tmp_path / "home" / ".codex" / "log" / "codex-tui.log"
            outside_target = tmp_path / "outside-target.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")

            tool = self.load_tool_module()
            original_validate = tool._validate_rotation_paths

            def replace_current_after_validation(path: pathlib.Path, retained_rotations: int) -> None:
                original_validate(path, retained_rotations)
                if path == codex_log:
                    path.unlink()
                    path.symlink_to(outside_target)

            tool._validate_rotation_paths = replace_current_after_validation
            try:
                with self.assertRaises((OSError, tool.PolicyError)):
                    tool._rotate_log(codex_log, 2)
            finally:
                tool._validate_rotation_paths = original_validate

            outside_exists = outside_target.exists()
            current_is_symlink = codex_log.is_symlink()

        self.assertFalse(outside_exists)
        self.assertTrue(current_is_symlink)

    def test_apply_rotation_preserves_original_log_mode(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")
            codex_log.chmod(0o600)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )
            mode_after = codex_log.stat().st_mode & 0o777

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        self.assertEqual(mode_after, 0o600)

    def test_apply_aborts_when_policy_changes_after_dry_run(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            policy.write_text(
                policy.read_text(encoding="utf-8").replace(
                    "retained_rotations = 2",
                    "retained_rotations = 1",
                    1,
                ),
                encoding="utf-8",
            )
            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report)],
            )
            after = codex_log.read_bytes()
            rotated_exists = codex_log.with_name("codex-tui.log.1").exists()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(after, original)
        self.assertFalse(rotated_exists)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "aborted")
        self.assertEqual(payload["reason"], "policy_changed_after_dry_run")
        self.assertEqual(payload["actions_taken"], [])

    def test_apply_refuses_mutable_actions_when_configured_active_writer_detected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            self.write_policy_fixture(policy)

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            original = b"codex log requiring rotation"
            codex_log.write_bytes(original)
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-name", "codex"],
            )
            after = codex_log.read_bytes()
            rotated_exists = codex_log.with_name("codex-tui.log.1").exists()

        self.assertEqual(result.returncode, 1, (result.stdout, result.stderr))
        self.assertEqual(after, original)
        self.assertFalse(rotated_exists)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "refused")
        self.assertEqual(payload["reason"], "active_writer_detected")
        self.assertEqual(payload["actions_taken"], [])
        self.assertEqual(payload["active_writer_refusals"][0]["process_names"], ["codex"])

    def test_successful_apply_summary_includes_actions_skips_and_refusal_reasons(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            policy = tmp_path / "policy.toml"
            report = tmp_path / "dry-run.json"
            home_root = tmp_path / "home"
            repo_root = tmp_path / "repo"
            retained = "1.96.0-aarch64-apple-darwin"
            self.write_policy_fixture(policy, retain_exact_names=[retained])

            codex_log = home_root / ".codex" / "log" / "codex-tui.log"
            codex_log.parent.mkdir(parents=True)
            codex_log.write_bytes(b"codex log requiring rotation")

            outside_target = tmp_path / "outside-session.jsonl"
            outside_target.write_bytes(b"outside")
            session_link = home_root / ".codex" / "sessions" / "link.jsonl"
            session_link.parent.mkdir(parents=True)
            session_link.symlink_to(outside_target)

            retained_toolchain = home_root / ".rustup" / "toolchains" / retained
            retained_toolchain.mkdir(parents=True)
            (retained_toolchain / "marker").write_bytes(b"retained")
            repo_root.mkdir()

            dry_run = self.run_tool("dry-run", home_root, repo_root, policy)
            self.assertEqual(dry_run.returncode, 0, (dry_run.stdout, dry_run.stderr))
            report.write_text(dry_run.stdout, encoding="utf-8")

            result = self.run_tool(
                "apply",
                home_root,
                repo_root,
                policy,
                ["--dry-run-report", str(report), "--process-snapshot-empty"],
            )

        self.assertEqual(result.returncode, 0, (result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "applied")
        self.assertEqual(payload["bytes_reclaimed"], 0)
        self.assertEqual(payload["actions_taken"][0]["action"], "rotate")
        self.assertEqual(payload["refusal_reasons"][0]["reason"], "symlink_not_followed")
        protected = {entry["reason"] for entry in payload["skipped_protected"]}
        self.assertIn("exact_name_retain_policy", protected)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
