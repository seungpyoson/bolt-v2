#!/usr/bin/env python3
"""Self-tests for the generated-systemd-unit drift verifier."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
from pathlib import Path


def _load(name: str):
    path = Path(__file__).with_name(f"{name}.py")
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


RENDERER = _load("render_install_unit")
VERIFIER = _load("verify_install_unit_generated")


def test_render_round_trips_committed_unit() -> None:
    expected = RENDERER.render()
    committed = VERIFIER.UNIT_PATH.read_text(encoding="utf-8")
    if committed != expected:
        raise AssertionError(
            "committed deploy/systemd/bolt-v2.service does not match the render; "
            "run `just generate-unit`"
        )


def test_render_leaves_runtime_variable_untouched() -> None:
    rendered = RENDERER.render()
    if "${BOLT_LIVE_PROFILE}" not in rendered:
        raise AssertionError("render must pass ${BOLT_LIVE_PROFILE} through verbatim")
    if "@" in rendered:
        raise AssertionError(f"render left an unresolved marker: {rendered!r}")


def test_render_derives_paths_from_layout() -> None:
    layout = RENDERER.load_layout()
    install_root = layout["BOLT_INSTALL_ROOT"]
    rendered = RENDERER.render()
    for required in (
        f"WorkingDirectory={layout['BOLT_HOME']}",
        f"EnvironmentFile={layout['LIVE_ENV_DIR']}/live.env",
        f"ExecStart={install_root}/bolt-v2 ops launch",
        f"--config-root {install_root}/config",
    ):
        if required not in rendered:
            raise AssertionError(f"render missing derived path line: {required!r}")


def test_missing_required_key_errors() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        layout_path = Path(tmp) / "install-layout.env"
        layout_path.write_text(
            "BOLT_HOME=/srv/bolt-v2\nBOLT_INSTALL_ROOT=/opt/bolt-v2\n",
            encoding="utf-8",
        )
        try:
            RENDERER.load_layout(layout_path)
        except ValueError as exc:
            if "LIVE_ENV_DIR" not in str(exc):
                raise AssertionError(f"unexpected error message: {exc}")
        else:
            raise AssertionError("expected ValueError for missing LIVE_ENV_DIR")


def test_unknown_marker_in_template_errors() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        layout_path = Path(tmp) / "install-layout.env"
        layout_path.write_text(
            "BOLT_HOME=/srv/bolt-v2\nBOLT_INSTALL_ROOT=/opt/bolt-v2\nLIVE_ENV_DIR=/etc/bolt-v2\n",
            encoding="utf-8",
        )
        template_path = Path(tmp) / "unit.in"
        template_path.write_text("WorkingDirectory=@BOLT_HOME@\nExtra=@TYPO_MARKER@\n", encoding="utf-8")
        try:
            RENDERER.render(layout_path, template_path)
        except ValueError as exc:
            if "@TYPO_MARKER@" not in str(exc):
                raise AssertionError(f"unexpected error message: {exc}")
        else:
            raise AssertionError("expected ValueError for unresolved marker")


def test_verifier_passes_on_committed_tree() -> None:
    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        code = VERIFIER.main()
    if code != 0:
        raise AssertionError(f"verifier should pass on committed tree, got code={code}")
    if "OK:" not in stdout.getvalue():
        raise AssertionError(f"verifier should print OK on success: {stdout.getvalue()!r}")


def test_verifier_detects_tampered_unit() -> None:
    import tempfile

    original_unit_path = VERIFIER.UNIT_PATH
    tampered = VERIFIER.UNIT_PATH.read_text(encoding="utf-8").replace(
        "Restart=on-failure", "Restart=always"
    )
    if tampered == VERIFIER.UNIT_PATH.read_text(encoding="utf-8"):
        raise AssertionError("tamper fixture did not change the unit text")
    stderr = io.StringIO()
    with tempfile.TemporaryDirectory() as tmp:
        tampered_path = Path(tmp) / "bolt-v2.service"
        tampered_path.write_text(tampered, encoding="utf-8")
        try:
            VERIFIER.UNIT_PATH = tampered_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.UNIT_PATH = original_unit_path
    if code != 1:
        raise AssertionError(f"verifier should flag a tampered unit, got code={code}")
    if "stale" not in stderr.getvalue():
        raise AssertionError(f"verifier should report staleness: {stderr.getvalue()!r}")


def main() -> int:
    tests = [
        test_render_round_trips_committed_unit,
        test_render_leaves_runtime_variable_untouched,
        test_render_derives_paths_from_layout,
        test_missing_required_key_errors,
        test_unknown_marker_in_template_errors,
        test_verifier_passes_on_committed_tree,
        test_verifier_detects_tampered_unit,
    ]
    for test in tests:
        test()
    print("OK: generated-unit drift verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
