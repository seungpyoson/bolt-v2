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
# A layout value is only safe if ``bash source`` and Python ``value.strip()``
# interpret it identically. This charset covers every current value
# (/srv/bolt-v2, /opt/bolt-v2, /etc/bolt-v2, bolt) and EXCLUDES every character
# bash word-splitting/quoting/comment-handling treats specially: whitespace,
# ``#``, quotes, ``$``, backtick, backslash, ``;``, ``&``, ``|``, ``<``, ``>``,
# parens, braces, ``*``, ``?``, ``[``, ``]``, ``~``, ``!``. A value matching this
# regex is the same string under both consumers; anything else fails closed.
_BARE_VALUE_RE = re.compile(r"^[A-Za-z0-9_./:@%+,=-]+$")


def load_layout(path: Path = LAYOUT_PATH) -> dict[str, str]:
    """Parse the bash-sourceable KEY=value layout into a dict.

    Blank lines and ``#`` comments are skipped. Values MUST be bare tokens (no
    whitespace, quotes, ``#``, or shell metacharacters) so the two consumers of
    this single source — ``deploy/install.sh`` (bash ``source``) and this parser
    — cannot interpret a value differently. The value is matched RAW (the
    partition value is NOT stripped before the fence): any whitespace anywhere
    in it — leading, trailing, or internal — is rejected, because a space after
    ``=`` (``BOLT_USER= bolt``) is a real bash-vs-Python divergence. A non-bare
    value (e.g. one carrying an inline ``# comment``, quotes, or spaces) fails
    closed with a ValueError naming the offending key.
    """
    layout: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        key, sep, value = line.partition("=")
        if not sep:
            raise ValueError(f"{path}: malformed layout line (no '='): {raw_line!r}")
        # Match the RAW value WITHOUT stripping: a space after `=` (e.g.
        # `BOLT_USER= bolt`) is a real bash-vs-Python divergence — bash `source`
        # sets an empty var and runs `bolt` as a command. `_BARE_VALUE_RE`
        # excludes whitespace, so any leading/trailing/internal space is
        # rejected; `line.strip()` above already handled line-level/`\r` space.
        if not _BARE_VALUE_RE.match(value):
            raise ValueError(
                f"{path}: value for {key.strip()!r} is not a bare token (bash "
                f"`source` and this parser would diverge): {raw_line!r}"
            )
        layout[key.strip()] = value
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
