from __future__ import annotations

import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


MODULE_PATH = (
    Path(__file__).resolve().parents[1] / "scripts" / "ci_tracked_mtime_manifest.py"
)
MODULE_SPEC = importlib.util.spec_from_file_location(
    "ci_tracked_mtime_manifest", MODULE_PATH
)
assert MODULE_SPEC is not None
assert MODULE_SPEC.loader is not None
mtime_manifest = importlib.util.module_from_spec(MODULE_SPEC)
MODULE_SPEC.loader.exec_module(mtime_manifest)


class TrackedMtimeManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.repo = Path(self.tmpdir.name) / "repo"
        self.repo.mkdir()

        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        subprocess.run(
            ["git", "-C", str(self.repo), "config", "user.name", "test"],
            check=True,
        )
        subprocess.run(
            ["git", "-C", str(self.repo), "config", "user.email", "test@example.com"],
            check=True,
        )

        self.manifest = self.repo / "manifest.json"

    def write_tracked_file(self, rel_path: str, content: str) -> Path:
        full_path = self.repo / rel_path
        full_path.parent.mkdir(parents=True, exist_ok=True)
        full_path.write_text(content, encoding="utf-8")
        return full_path

    def commit_all(self, message: str = "init") -> None:
        subprocess.run(["git", "-C", str(self.repo), "add", "."], check=True)
        subprocess.run(
            ["git", "-C", str(self.repo), "commit", "-q", "-m", message],
            check=True,
        )

    def capture_manifest(self) -> None:
        self.assertEqual(mtime_manifest.capture(self.repo, self.manifest), 0)

    def test_restore_skips_malformed_manifest(self) -> None:
        self.write_tracked_file("tracked.txt", "alpha\n")
        self.commit_all()
        self.manifest.write_text("{ malformed json", encoding="utf-8")

        stderr = io.StringIO()
        with mock.patch("sys.stderr", stderr):
            rc = mtime_manifest.restore(self.repo, self.manifest)

        self.assertEqual(rc, 0)
        self.assertIn("could not read manifest", stderr.getvalue())

    def test_restore_skips_dirty_worktree_files(self) -> None:
        tracked = self.write_tracked_file("tracked.txt", "alpha\n")
        self.commit_all()
        self.capture_manifest()

        tracked.write_text("beta\n", encoding="utf-8")
        dirty_stat = tracked.stat()
        dirty_mtime_ns = dirty_stat.st_mtime_ns + 5_000_000_000
        os.utime(tracked, ns=(dirty_stat.st_atime_ns, dirty_mtime_ns))

        self.assertEqual(mtime_manifest.restore(self.repo, self.manifest), 0)
        self.assertEqual(tracked.read_text(encoding="utf-8"), "beta\n")
        self.assertEqual(tracked.stat().st_mtime_ns, dirty_mtime_ns)

    def test_restore_catches_utime_errors(self) -> None:
        tracked = self.write_tracked_file("tracked.txt", "alpha\n")
        self.commit_all()
        self.capture_manifest()

        bumped_stat = tracked.stat()
        os.utime(
            tracked,
            ns=(bumped_stat.st_atime_ns, bumped_stat.st_mtime_ns + 5_000_000_000),
        )
        subprocess.run(
            ["git", "-C", str(self.repo), "update-index", "--refresh"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        stderr = io.StringIO()
        with (
            mock.patch.object(mtime_manifest.os, "utime", side_effect=OSError("denied")),
            mock.patch("sys.stderr", stderr),
        ):
            rc = mtime_manifest.restore(self.repo, self.manifest)

        self.assertEqual(rc, 0)
        self.assertIn("could not restore mtime", stderr.getvalue())

    def test_restore_skips_utime_when_mtime_already_matches(self) -> None:
        self.write_tracked_file("tracked.txt", "alpha\n")
        self.commit_all()
        self.capture_manifest()

        with mock.patch.object(mtime_manifest.os, "utime") as mocked_utime:
            rc = mtime_manifest.restore(self.repo, self.manifest)

        self.assertEqual(rc, 0)
        mocked_utime.assert_not_called()


if __name__ == "__main__":
    unittest.main()
