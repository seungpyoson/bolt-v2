# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "duckdb==1.4.1",
#     "polars==1.34.0",
#     "pyarrow==22.0.0",
#     "numpy==2.3.4",
#     "requests==2.32.5",
#     "lz4==4.4.4",
# ]
# ///
"""Lead-lag follow-up (issue #633 item 2): one-command re-measurement of the study stack.

The published verdicts (docs/research/leadlag-taker-edge-2026-06-10.md and successors)
come from one April 2026 window; regimes change. This harness re-runs the whole pipeline
(scripts/leadlag_session4.py, leadlag_trades_leader.py, leadlag_subsecond.py) on a fresh
window with one command, so a re-measurement is a re-run, never a re-derivation. Per the
operator directive on #633, the harness is asset-agnostic: per-asset verdicts are OUTPUTS
of the rerun's tables, never inputs to it.

Re-measurement cadence (the policy home; #633 item 2):
  1. once, immediately after the #630 supervised pilot closes;
  2. monthly thereafter while the strategy family trades;
  3. additionally after structural market changes (Polymarket fee or tick-size change,
     leader-venue migration/outage, observed maker-regime shift).
Each run uses a fresh window of at least 4 consecutive days (the #631 minimum) over the
full configured asset set, with --report-dir docs/research/leadlag-remeasure-<window>/.

The lake must be backfilled for the window first; `preflight` reports per-source coverage.
The backfill scripts (backfill_archive_objects_to_s3.py for pmxt via its source-proof,
backfill_hyperliquid_core_to_s3.py, backfill_bybit_to_s3.py) live on branch
feat/023-venue-data-backfill, not on main.

Stage caches: the underlying scripts cache extracts under --workdir keyed by date; a
re-run of an already-extracted window skips downloads. Cache-generation compatibility
(e.g. extracts predating PR #640's dual-clock columns) is owned and enforced by the
analysis scripts themselves — they fail loud on mixed or unusable caches; the operator
remedy is to delete that window's cache dirs and re-extract.

Reproduction:
  uv run scripts/leadlag_remeasure.py preflight --dates <start>:<end>
  uv run scripts/leadlag_remeasure.py run --dates <start>:<end> --report-dir <dir>
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from datetime import date as _date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import leadlag_session4 as s4  # noqa: E402
import leadlag_trades_leader as tl  # noqa: E402

SCRIPTS_DIR = Path(__file__).resolve().parent
BACKFILL_BRANCH = "feat/023-venue-data-backfill"
MIN_WINDOW_DAYS = 4  # cadence policy (module docstring): the #631 minimum


def window_policy_violation(dates: list[str]) -> str | None:
    """Cadence policy check: a verdict-bearing window is >= MIN_WINDOW_DAYS
    CONSECUTIVE days. Returns a human-readable violation, or None if compliant.
    The policy lives in the module docstring; this is its enforcement."""
    if len(dates) < MIN_WINDOW_DAYS:
        return f"{len(dates)} day(s) < {MIN_WINDOW_DAYS}"
    ordered = sorted(_date.fromisoformat(d) for d in dates)
    for prev, cur in zip(ordered, ordered[1:]):
        if (cur - prev).days != 1:
            return f"non-consecutive: gap between {prev} and {cur}"
    return None

# (stage name, script, subcommand, extra argv, needs_trades_window, report filename)
STAGES = (
    ("resolve", "leadlag_session4.py", "resolve", (), False, None),
    ("extract-pm", "leadlag_session4.py", "extract-pm", (), False, None),
    ("extract-leader-hl", "leadlag_session4.py", "extract-leader", (), False, None),
    ("analyze-session4", "leadlag_session4.py", "analyze", (), False, "taker-edge.md"),
    ("extract-leader-trades", "leadlag_trades_leader.py", "extract-leader", (), True, None),
    ("subsecond", "leadlag_subsecond.py", "subsecond", (), False, "subsecond.md"),
    ("extract-sizes", "leadlag_subsecond.py", "extract-sizes", (), False, None),
    ("fillability-hl", "leadlag_subsecond.py", "fillability", ("--leader", "hl"), False, "fillability_hl.md"),
    ("fillability-trades", "leadlag_subsecond.py", "fillability", ("--leader", "trades"), True, "fillability_trades.md"),
    ("analyze-trades-mid", "leadlag_trades_leader.py", "analyze", ("--mark", "mid"), True, "trades_leader_mid.md"),
    ("analyze-trades-settlement", "leadlag_trades_leader.py", "analyze", ("--mark", "settlement"), True, "trades_leader_settlement.md"),
)
STAGE_NAMES = tuple(s[0] for s in STAGES)


def s3_keys(prefix: str) -> list[str]:
    proc = subprocess.run(
        ["aws", "s3", "ls", "--recursive", prefix], check=False, capture_output=True, text=True
    )
    if proc.returncode not in (0, 1):  # 1 = empty prefix; anything else is a real failure
        raise SystemExit(f"aws s3 ls failed for {prefix}: {proc.stderr.strip()}")
    return [parts[3] for line in proc.stdout.splitlines() if len(parts := line.split()) >= 4]


def parse_assets(raw: str) -> list[str]:
    """Validate asset tokens at the harness entry: a malformed token (space, typo,
    empty) must fail loud HERE, not waste a resolve stage or surface as a bare
    KeyError mid-pipeline. No silent normalization — fix the invocation instead."""
    assets = raw.split(",")
    unknown = [a for a in assets if a not in s4.LEADER_COIN_BY_ASSET]
    if unknown:
        raise SystemExit(
            f"unknown assets: {unknown}; valid: {','.join(sorted(s4.LEADER_COIN_BY_ASSET))} "
            "(comma-separated, no spaces)"
        )
    return assets


def coverage(dates: list[str], assets: list[str]) -> tuple[list[str], list[str], list[str]]:
    """Per-date lake coverage. Returns (report_lines, hl_dates, trades_dates)."""
    lines = ["| date | pmxt objects | hl coins covered | bybit symbols covered |", "|---|---|---|---|"]
    hl_dates: list[str] = []
    trades_dates: list[str] = []
    coins = [s4.LEADER_COIN_BY_ASSET[a] for a in assets]
    # One `aws s3 ls` per (source, date) — 3*len(dates) independent network
    # round-trips. Issue them concurrently so a 7-day window costs one batch
    # (~1 round-trip) instead of ~21 serial calls (~10-20s) before the operator
    # sees coverage. executor.map preserves input order, so the per-source
    # slices realign with `dates` by index.
    prefixes = (
        [f"{s4.PMXT_S3_PREFIX}/dt={date}/" for date in dates]
        + [f"{s4.HL_L2BOOK_PREFIX}/date={date.replace('-', '')}/" for date in dates]
        + [f"{tl.BYBIT_TRADES_PREFIX}/dt={date}/" for date in dates]
    )
    with ThreadPoolExecutor(max_workers=min(32, len(prefixes) or 1)) as pool:
        results = list(pool.map(s3_keys, prefixes))
    n = len(dates)
    pm_results, hl_results, by_results = results[:n], results[n : 2 * n], results[2 * n :]
    for i, date in enumerate(dates):
        n_pm = len(pm_results[i])
        n_hl = sum(1 for c in coins if any(f"/coin={c}/" in k for k in hl_results[i]))
        n_by = sum(
            1
            for c in coins
            if any(f"/symbol={tl.BYBIT_SYMBOL_BY_COIN[c]}/" in k for k in by_results[i])
        )
        lines.append(f"| {date} | {n_pm} | {n_hl}/{len(coins)} | {n_by}/{len(coins)} |")
        if n_pm and n_hl == len(coins):
            hl_dates.append(date)
            # trades stages consume PM extracts produced over the hl window, so a
            # trades-eligible date must ALSO be hl-eligible — bybit coverage alone
            # would admit dates whose PM extracts never get created
            if n_by == len(coins):
                trades_dates.append(date)
    return lines, hl_dates, trades_dates


def cmd_preflight(args: argparse.Namespace) -> None:
    dates = s4.parse_dates(args.dates)
    assets = parse_assets(args.assets)
    lines, hl_dates, trades_dates = coverage(dates, assets)
    print("\n".join(lines), flush=True)
    print(f"hl-clock window covered: {len(hl_dates)}/{len(dates)} dates", flush=True)
    print(f"trades-clock window covered: {len(trades_dates)}/{len(dates)} dates", flush=True)
    if len(hl_dates) < len(dates) or len(trades_dates) < len(dates):
        print(
            f"backfill the missing dates first: backfill scripts live on branch {BACKFILL_BRANCH} "
            "(backfill_archive_objects_to_s3.py for pmxt, backfill_hyperliquid_core_to_s3.py, "
            "backfill_bybit_to_s3.py)",
            flush=True,
        )


def run_stage(name: str, script: str, argv: list[str]) -> None:
    cmd = ["uv", "run", str(SCRIPTS_DIR / script), *argv]
    print(f"\n=== stage {name}: {' '.join(cmd)}", flush=True)
    subprocess.run(cmd, check=True)


def cmd_run(args: argparse.Namespace) -> None:
    dates = s4.parse_dates(args.dates)
    viol = window_policy_violation(dates)
    if viol:
        raise SystemExit(
            f"--dates violates the re-measurement cadence policy "
            f">= {MIN_WINDOW_DAYS} consecutive days ({viol}); see module docstring"
        )
    assets = parse_assets(args.assets)
    selected = args.stages.split(",") if args.stages else list(STAGE_NAMES)
    unknown = [s for s in selected if s not in STAGE_NAMES]
    if unknown:
        raise SystemExit(f"unknown stages: {unknown}; valid: {','.join(STAGE_NAMES)}")
    report_dir = Path(args.report_dir)
    report_dir.mkdir(parents=True, exist_ok=True)

    if args.skip_preflight:
        hl_dates, trades_dates = dates, dates
    else:
        lines, hl_dates, trades_dates = coverage(dates, assets)
        print("\n".join(lines), flush=True)
        if not hl_dates:
            raise SystemExit(
                f"no requested date is covered for pmxt+hl in the lake; backfill first "
                f"(scripts on branch {BACKFILL_BRANCH})"
            )
        if len(hl_dates) < len(dates):
            missing = sorted(set(dates) - set(hl_dates))
            print(f"RESTRICTING hl-clock stages to {len(hl_dates)} covered dates; missing: {missing}", flush=True)
        if not trades_dates:
            print("NO trades-clock coverage in window; trades stages will be SKIPPED", flush=True)
        elif len(trades_dates) < len(dates):
            missing = sorted(set(dates) - set(trades_dates))
            print(f"RESTRICTING trades-clock stages to {len(trades_dates)} covered dates; missing: {missing}", flush=True)
        # coverage restriction must not quietly shrink a window below the cadence
        # policy the requested --dates already passed
        viol = window_policy_violation(hl_dates)
        if viol:
            raise SystemExit(
                f"effective hl window violates the cadence policy ({viol}); "
                f"backfill the missing dates first (branch {BACKFILL_BRANCH})"
            )
        if trades_dates:
            viol = window_policy_violation(trades_dates)
            if viol:
                print(
                    f"effective trades window violates the cadence policy ({viol}); "
                    "trades stages will be SKIPPED",
                    flush=True,
                )
                trades_dates = []

    skipped: list[str] = []
    for name, script, subcommand, extra, needs_trades, report_name in STAGES:
        if name not in selected:
            continue
        window = trades_dates if needs_trades else hl_dates
        if not window:
            skipped.append(name)
            continue
        argv = [subcommand, "--dates", ",".join(window), "--assets", args.assets, *extra]
        if args.workdir:
            argv += ["--workdir", args.workdir]
        if report_name:
            argv += ["--report", str(report_dir / report_name)]
        run_stage(name, script, argv)
    if skipped:
        print(f"\nSKIPPED stages (no covered dates): {','.join(skipped)}", flush=True)
    print(f"\nre-measurement complete; reports under {report_dir}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_pre = sub.add_parser("preflight", help="lake coverage check for a window")
    p_pre.add_argument("--dates", required=True)
    p_pre.add_argument("--assets", default=s4.DEFAULT_ASSETS)
    p_pre.set_defaults(func=cmd_preflight)

    p_run = sub.add_parser("run", help="run the full study pipeline on a window")
    p_run.add_argument("--dates", required=True)
    p_run.add_argument("--assets", default=s4.DEFAULT_ASSETS)
    p_run.add_argument("--workdir")
    p_run.add_argument("--report-dir", required=True)
    p_run.add_argument("--stages", help=f"comma subset of: {','.join(STAGE_NAMES)}")
    p_run.add_argument("--skip-preflight", action="store_true")
    p_run.set_defaults(func=cmd_run)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
