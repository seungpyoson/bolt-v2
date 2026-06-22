#!/usr/bin/env python3
"""Source-fence: the committed systemd unit matches its generated render.

``deploy/systemd/bolt-v2.service`` is a GENERATED artifact (see
``scripts/render_install_unit.py``). This drift guard fails CI if the committed
unit diverges from a fresh render of ``deploy/install-layout.env`` +
``deploy/systemd/bolt-v2.service.in``, catching a hand-edited unit or a stale
checkout that forgot to run ``just generate-unit``.
"""

from __future__ import annotations

import difflib
import sys
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).resolve().parent
if str(_SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS_DIR))

import render_install_unit

REPO_ROOT = _SCRIPTS_DIR.parent
UNIT_PATH = REPO_ROOT / "deploy/systemd/bolt-v2.service"
UNIT_REL = "deploy/systemd/bolt-v2.service"


def main() -> int:
    expected = render_install_unit.render()
    actual = UNIT_PATH.read_text(encoding="utf-8")
    if actual == expected:
        print(f"OK: {UNIT_REL} matches the generated render.")
        return 0

    diff = difflib.unified_diff(
        expected.splitlines(keepends=True),
        actual.splitlines(keepends=True),
        fromfile=f"{UNIT_REL} (generated)",
        tofile=f"{UNIT_REL} (committed)",
    )
    sys.stderr.writelines(diff)
    print(
        f"FAIL: {UNIT_REL} is stale; run `just generate-unit`",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
