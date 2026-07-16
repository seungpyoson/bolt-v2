#!/usr/bin/env python3
"""Fence the disabled, lease-bound Polymarket redemption preparation surface."""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import tomllib


OWNER = pathlib.Path("src/bolt_v3_polymarket_redemption.rs")
GENERATED = pathlib.Path("src/bolt_v3_polymarket_redemption_generated.rs")
RUNTIME = pathlib.Path("config/polymarket-redemption.toml")
EVIDENCE = pathlib.Path("config/polymarket-redemption-source-evidence.toml")
COMPILE_FAIL = pathlib.Path("tests/polymarket_redemption_preparation_compile_fail.rs")
GENERATOR = pathlib.Path("scripts/generate_polymarket_redemption_config.py")
RUNTIME_AUTHORITY_KEYS = frozenset(
    {
        "standard_adapter_target",
        "negative_risk_adapter_target",
        "signer_private_key_ssm_path",
        "builder_api_key_ssm_path",
        "builder_api_secret_ssm_path",
        "builder_passphrase_ssm_path",
    }
)
EXPECTED_EVIDENCE = {
    "adapter_repository": "https://github.com/Polymarket/ctf-exchange-v2",
    "adapter_revision": "ccc0596074f4dfd62c944fbca4de252893b82b4b",
    "function_signature": "redeemPositions(address,bytes32,bytes32,uint256[])",
    "function_selector": "0x01b7037c",
    "request_repository": "https://github.com/Polymarket/builder-relayer-client",
    "request_revision": "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290",
}


def _read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def _production_owner(text: str) -> str:
    marker = "#[cfg(test)]"
    return text.split(marker, 1)[0]


def _toml(path: pathlib.Path) -> dict[str, object]:
    return tomllib.loads(_read(path))


def _repository_toml(root: pathlib.Path) -> list[pathlib.Path]:
    ignored = {".git", ".worktrees", "target"}
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not ignored.intersection(path.relative_to(root).parts)
    )


def _prepare_signature(text: str) -> str:
    start = text.find("pub fn prepare_redemption_request(")
    if start < 0:
        return ""
    body = text.find("{", start)
    return text[start:body] if body >= 0 else text[start:]


def boundary_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    required = [OWNER, GENERATED, RUNTIME, EVIDENCE, COMPILE_FAIL, pathlib.Path("Cargo.toml")]
    missing = [str(path) for path in required if not (root / path).is_file()]
    if missing:
        return [f"missing required redemption preparation artifact(s): {missing}"]

    try:
        owner_text = _read(root / OWNER)
        generated_text = _read(root / GENERATED)
        runtime = _toml(root / RUNTIME)
        evidence = _toml(root / EVIDENCE)
        compile_fail_text = _read(root / COMPILE_FAIL)
        cargo_text = _read(root / "Cargo.toml")
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        return [f"cannot inspect redemption preparation artifacts: {error}"]

    production = _production_owner(owner_text)
    signature = _prepare_signature(production)

    authorities: dict[str, list[pathlib.Path]] = {key: [] for key in RUNTIME_AUTHORITY_KEYS}
    for path in _repository_toml(root):
        try:
            text = _read(path)
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect TOML authority {path.relative_to(root)}: {error}")
            continue
        relative = path.relative_to(root)
        for key in RUNTIME_AUTHORITY_KEYS:
            if re.search(rf"(?m)^\s*{re.escape(key)}\s*=", text):
                authorities[key].append(relative)
    for key, paths in authorities.items():
        if paths != [RUNTIME]:
            errors.append(
                f"runtime field {key} must have one TOML authority at {RUNTIME}; found {paths}"
            )

    if runtime.get("production_activation_enabled") is not False:
        errors.append("production_activation_enabled must remain false")
    evidence_text = _read(root / EVIDENCE)
    duplicated_keys = sorted(
        key
        for key in RUNTIME_AUTHORITY_KEYS
        if re.search(rf"(?m)^\s*{re.escape(key)}\s*=", evidence_text)
    )
    if duplicated_keys:
        errors.append(f"source evidence duplicates runtime TOML keys: {duplicated_keys}")

    adapter = evidence.get("adapter_abi")
    safe_request = evidence.get("safe_request")
    if not isinstance(adapter, dict) or not isinstance(safe_request, dict):
        errors.append("source evidence must contain adapter_abi and safe_request tables")
    else:
        observed = {
            "adapter_repository": adapter.get("repository"),
            "adapter_revision": adapter.get("revision"),
            "function_signature": adapter.get("function_signature"),
            "function_selector": adapter.get("function_selector"),
            "request_repository": safe_request.get("repository"),
            "request_revision": safe_request.get("revision"),
        }
        if observed != EXPECTED_EVIDENCE:
            errors.append(
                "source evidence must remain pinned to the reviewed adapter/request revisions and ABI: "
                f"{observed}"
            )

    if "pub enum AttemptKind" not in production or not all(
        variant in production for variant in ("Original", "Fence")
    ):
        errors.append("AttemptKind must expose Original and Fence")
    if "lease: &mut RiskClosureWorkspaceLease" not in signature:
        errors.append("request preparation must accept &mut RiskClosureWorkspaceLease")
    if "impl for<'request> FnOnce(PreparedRequest<'request>)" not in signature:
        errors.append("request bytes must be exposed through a callback-scoped PreparedRequest")
    if "&mut [u8]" in signature:
        errors.append("public request preparation must not accept a caller-provided raw buffer")
    if re.search(r"lease\s*\.\s*with_workspace_mut", production) is None:
        errors.append("request encoding must occur inside lease.with_workspace_mut")

    prepared_match = re.search(
        r"pub\s+struct\s+PreparedRequest<'request>\s*\{(?P<body>.*?)\}",
        production,
        flags=re.DOTALL,
    )
    if prepared_match is None:
        errors.append("PreparedRequest<'request> must be declared")
    else:
        body = prepared_match.group("body")
        if "&'request [u8]" not in body or re.search(r"\b(?:Vec|String)\b", body):
            errors.append("PreparedRequest bytes must remain a borrowed slice without owned storage")

    forbidden_authority = (
        "RiskClosureWorkspaceAuthority",
        "RiskClosureWorkspaceReservation",
        "checkout_new_risk",
        "TerminalReleasePermit",
        "release_terminal",
    )
    if any(token in production for token in forbidden_authority):
        errors.append("production owner contains a forbidden new-risk or authority surface")

    forbidden_geometry = (
        "arena_bytes",
        "slot_bytes",
        "workspace_len",
        "SLOT_BYTES",
        "ARENA_BYTES",
        "WORKSPACE_CAPACITY",
    )
    if any(token in production for token in forbidden_geometry):
        errors.append("production owner contains copied or independent workspace geometry")
    if any(token in generated_text for token in ("arena_bytes", "slot_bytes", "capacity: 10")):
        errors.append("generated redemption config must not copy #1430 workspace geometry")

    forbidden_sinks = (
        "reqwest",
        "HttpClient",
        "TcpStream",
        "UdpSocket",
        "SocketAddr",
        "std::fs",
        "OpenOptions",
        "tokio::",
        "async fn",
    )
    if any(token in production for token in forbidden_sinks):
        errors.append("production owner contains a network or durable sink")
    if "SsmResolverSession::new" in production:
        errors.append("production owner constructs a forbidden second SSM session")
    forbidden_secret_backends = (
        "std::env",
        "Command::new",
        "aws ssm",
        "op item",
        "1password",
    )
    if any(token.lower() in production.lower() for token in forbidden_secret_backends):
        errors.append("production owner contains an alternate secret backend")
    if "SsmResolverSession" not in production or "SsmSecretResolver" not in production:
        errors.append("production owner must reuse SsmResolverSession and SsmSecretResolver")

    forbidden_observability = (
        "println!",
        "eprintln!",
        "log::",
        "tracing::",
        "info!",
        "debug!",
        "warn!",
        "error!",
    )
    if any(token in production for token in forbidden_observability):
        errors.append("production owner contains a forbidden logging or observability sink")
    if re.search(r"derive\([^)]*Serialize", production):
        errors.append("prepared requests and resolved credentials must not derive serialization")

    if "pub const POLYMARKET_REDEMPTION" in generated_text:
        errors.append("generated runtime and protocol projections must remain private")
    if "POLYMARKET_REDEMPTION_PREPARATION_CONFIG" not in generated_text:
        errors.append("generated runtime projection is missing")
    if "POLYMARKET_REDEMPTION_PROTOCOL" not in generated_text:
        errors.append("generated protocol projection is missing")

    for dependency in ('alloy-signer = "=2.1.0"', 'alloy-signer-local = "=2.1.0"'):
        if dependency not in cargo_text:
            errors.append(f"direct signer dependency must remain exact and locked: {dependency}")
    if (
        'name = "polymarket_redemption_preparation"' not in cargo_text
        or 'path = "tests/polymarket_redemption_preparation.rs"' not in cargo_text
    ):
        errors.append("compile-fail test target is not wired")
    for marker in (
        "RiskClosureWorkspaceReservation",
        "prepared_request_cannot_escape",
        "serde_json::to_string",
    ):
        if marker not in compile_fail_text:
            errors.append(f"compile-fail proof is missing marker {marker}")

    for path in sorted((root / "src").rglob("*.rs")):
        relative = path.relative_to(root)
        if relative in {OWNER, GENERATED, pathlib.Path("src/lib.rs")}:
            continue
        try:
            text = _read(path)
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect production caller surface {relative}: {error}")
            continue
        if "prepare_redemption_request" in text:
            errors.append(f"active production caller found outside disabled module: {relative}")

    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = boundary_errors(root)
    generation = subprocess.run(
        [
            sys.executable,
            str(root / GENERATOR),
            "--runtime-source",
            str(root / RUNTIME),
            "--evidence-source",
            str(root / EVIDENCE),
            "--output",
            str(root / GENERATED),
            "--check",
        ],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if generation.returncode != 0:
        errors.append(generation.stderr.strip() or "generated redemption config is stale")
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: Polymarket redemption preparation remains disabled and lease-bound.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
