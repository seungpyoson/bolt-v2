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
        retained = retain_exact_names or ["1.95.0-aarch64-apple-darwin"]
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
        pinned = "1.95.0-aarch64-apple-darwin"
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
                    channel = "1.95.0"
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
                'retain_exact_names = ["1.95.0-aarch64-apple-darwin"]\n',
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
        pinned = "1.95.0-aarch64-apple-darwin"
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
                    channel = "1.95.0"
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
            retained = "1.95.0-aarch64-apple-darwin"
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
    unittest.main()
