#!/usr/bin/env python3
"""Render the bolt-v2 systemd unit from the single-source install layout.

The committed ``deploy/systemd/bolt-v2.service`` is a GENERATED artifact. Its
install paths live in exactly one place — ``deploy/install-layout.env`` — and the
``deploy/systemd/bolt-v2.service.in`` template carries ``@MARKER@`` placeholders
for them. ``deploy/install.sh`` sources the same layout file, so the unit and the
installer can never drift apart. The systemd RUNTIME variable
``${BOLT_LIVE_PROFILE}`` is intentionally NOT a marker: it must reach systemd
verbatim, so the template uses ``${...}`` for runtime variables and ``@...@`` only
for generate-time install paths, keeping the two unambiguous.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
LAYOUT_PATH = REPO_ROOT / "deploy/install-layout.env"
TEMPLATE_PATH = REPO_ROOT / "deploy/systemd/bolt-v2.service.in"

REQUIRED_LAYOUT_KEYS = (
    "BOLT_HOME",
    "BOLT_INSTALL_ROOT",
    "LIVE_ENV_DIR",
    "BOLT_USER",
    "BOLT_GROUP",
)
_RESIDUAL_MARKER_RE = re.compile(r"@[A-Z_]+@")
# The skip-set is exactly bash's blank/comment no-op set: blank lines may
# contain only space or tab, and comment lines may have optional space/tab
# before ``#``. Every other raw line must pass the ``KEY=VALUE`` fullmatch
# grammar. For accepted lines, bash ``source`` assigns the identical value; all
# other forms fail closed on at least one consumer (parser ValueError or bash
# error), so the two consumers never silently assign different values. The value
# charset covers every current value (/srv/bolt-v2, /opt/bolt-v2, /etc/bolt-v2,
# bolt) while excluding whitespace and shell syntax.
_LAYOUT_LINE_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=([A-Za-z0-9_./:@%+,=-]+)")


def load_layout(path: Path = LAYOUT_PATH) -> dict[str, str]:
    """Parse the bash-sourceable KEY=value layout into a dict.

    Lines containing only bash blanks (space or tab) and ``#`` comments after
    optional bash blanks are skipped, matching bash's blank/comment no-op set.
    Every other raw line MUST match exactly ``KEY=VALUE`` with a shell-safe bare
    value. Lines accepted by this parser are assigned identically by
    ``deploy/install.sh`` (bash ``source``); every other form fails closed on at
    least one consumer with either parser ValueError or bash error, so the two
    consumers never silently assign different values.
    """
    layout: dict[str, str] = {}
    with path.open(encoding="utf-8", newline="") as handle:
        raw_text = handle.read()
    for raw_line in raw_text.split("\n"):
        stripped = raw_line.strip(" \t")
        if not stripped or stripped.startswith("#"):
            continue
        m = _LAYOUT_LINE_RE.fullmatch(raw_line)
        if not m:
            raise ValueError(
                f"{path}: malformed layout line (must be exactly KEY=VALUE with "
                f"a bare value; bash `source` and this parser would diverge): "
                f"{raw_line!r}"
            )
        layout[m.group(1)] = m.group(2)
    missing = [key for key in REQUIRED_LAYOUT_KEYS if key not in layout]
    if missing:
        raise ValueError(f"{path}: missing required layout keys: {', '.join(missing)}")
    return layout


def render(layout_path: Path = LAYOUT_PATH, template_path: Path = TEMPLATE_PATH) -> str:
    """Return the rendered unit text, byte-for-byte deterministic from inputs."""
    layout = load_layout(layout_path)
    bolt_home = layout["BOLT_HOME"]
    install_root = layout["BOLT_INSTALL_ROOT"]
    live_env_dir = layout["LIVE_ENV_DIR"]
    substitutions = {
        "@BOLT_BIN@": f"{install_root}/bolt-v2",
        "@BOLT_CONFIG_DIR@": f"{install_root}/config",
        "@LIVE_ENV_FILE@": f"{live_env_dir}/live.env",
        "@BOLT_HOME@": bolt_home,
        "@BOLT_USER@": layout["BOLT_USER"],
        "@BOLT_GROUP@": layout["BOLT_GROUP"],
    }
    text = template_path.read_text(encoding="utf-8")
    for marker, value in substitutions.items():
        text = text.replace(marker, value)
    residual = _RESIDUAL_MARKER_RE.search(text)
    if residual:
        raise ValueError(
            f"{template_path}: unresolved marker {residual.group(0)!r} after render "
            "(unknown or misspelled placeholder)"
        )
    return text


if __name__ == "__main__":
    sys.stdout.write(render())
