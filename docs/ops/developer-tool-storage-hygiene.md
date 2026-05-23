# Developer-Tool Storage Hygiene

Issue: #375

This policy covers developer-tool storage outside the Bolt runtime. It does not change verifier/parser architecture, managed Rust verification cache policy, cargo registry/git cache policy, browser profile cleanup, package-manager cache cleanup, or NautilusTrader runtime behavior.

## Ownership Map

| Surface | Policy owner | Apply behavior |
|---|---|---|
| `~/.codex/log/codex-tui.log` | #375 | Rotate when larger than configured `max_bytes`. |
| `~/.codex/sessions/**/*.jsonl` | #375 | Delete only files older than configured `ttl_days`. |
| `~/.factory/logs/droid-log-single.log` | #375 | Rotate when larger than configured `max_bytes`. |
| `~/.rustup/toolchains/*` | #375 | Remove only exact installed names in `remove_exact_names`, after active/default/project-pinned/retained protections. |
| `~/.codex/logs_2.sqlite*` | #375 report-only | Measure and report only. |
| `~/.codex/history.jsonl` | #375 native guidance | Measure and report native `history.max_bytes` and `history.persistence`; do not delete. |
| `~/.codex/archived_sessions/**` | #375 report-only | Measure and report only. |
| Browser, package-manager, and Codex plugin caches | Adjacent context | Report as out of scope; do not count as #375-owned bytes. |

The authoritative policy is `ci/developer-tool-storage-hygiene.toml`. Runtime path families, size caps, TTLs, retained rotations, exact rustup names, preflight thresholds, and active-writer process names all come from that TOML file.

## Commands

Status is read-only and reports the configured surface inventory:

```sh
python3 scripts/developer_tool_storage_hygiene.py status \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --json
```

Dry-run is read-only and emits cleanup candidates, protected entries, report-only entries, per-surface measurements, adjacent context, and refusal reasons:

```sh
python3 scripts/developer_tool_storage_hygiene.py dry-run \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --json
```

Log rotation bounds the active writer file and preserves the rotated file as history. Current-log rotation therefore reports zero estimated reclaimed bytes unless a retained old rotation is explicitly removed by a future policy.

Preflight is read-only and compares measured #375-owned storage plus supplied or observed free disk against configured thresholds:

```sh
python3 scripts/developer_tool_storage_hygiene.py preflight \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --json
```

Apply requires a saved dry-run report and revalidates policy plus candidate state before mutation:

```sh
python3 scripts/developer_tool_storage_hygiene.py dry-run \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --json > /tmp/bolt-v2-developer-tool-storage-dry-run.json

python3 scripts/developer_tool_storage_hygiene.py apply \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --dry-run-report /tmp/bolt-v2-developer-tool-storage-dry-run.json \
  --process-snapshot-empty \
  --json
```

If `rustup.toolchains.remove_exact_names` is non-empty, dry-run/apply also require exact active and default rustup snapshots. The script does not infer these values because protecting active/default toolchains is a fail-closed safety requirement:

```sh
python3 scripts/developer_tool_storage_hygiene.py dry-run \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --active-rustup-toolchain 1.95.0-aarch64-apple-darwin \
  --default-rustup-toolchain 1.94.1-aarch64-apple-darwin \
  --json
```

The script does not collect the host process table. Apply refuses mutable Codex and Factory actions unless an explicit process snapshot is supplied. Use `--process-snapshot-empty` only after checking that no configured writer process is active, or pass exact observed process names explicitly:

```sh
python3 scripts/developer_tool_storage_hygiene.py apply \
  --policy ci/developer-tool-storage-hygiene.toml \
  --home-root "$HOME" \
  --repo-root "$PWD" \
  --dry-run-report /tmp/bolt-v2-developer-tool-storage-dry-run.json \
  --process-name codex \
  --json
```

## Safety Contract

- Apply refuses if policy validation fails after dry-run.
- Apply refuses if the current mutating candidate set differs from the saved dry-run report.
- Apply refuses mutable Codex and Factory actions when supplied process names exactly match configured active-writer process names.
- Apply never mutates report-only Codex SQLite, Codex history, or Codex archived-session surfaces.
- Apply never removes rustup toolchains protected by active/default/project-root pin/retain policy, even when the exact name appears in `remove_exact_names`.
- Symlinks are reported as refusals and are not followed for deletion.

The tests build scratch homes and repos only. They do not mutate the operator's real home directory.
