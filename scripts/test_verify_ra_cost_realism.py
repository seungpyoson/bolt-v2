#!/usr/bin/env python3
"""Self-tests for verify_ra_cost_realism.py."""

from __future__ import annotations

import importlib.util
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_cost_realism.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_cost_realism", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def compliant_run_manifest() -> str:
    return """
pub const UNSUPPORTED_NT_VENUE_SURFACES: &[&str] = &[
    "leverages",
    "margin_model",
    "modules",
    "settlement_prices",
];

pub struct ManifestFillModelConfig {
    pub kind: String,
    pub random_seed: Option<u64>,
}

pub struct ManifestLatencyModelConfig {
    pub kind: String,
}

pub struct ManifestFeeModelConfig {
    pub kind: String,
}

fn resolve_fill_model(config: Option<&ManifestFillModelConfig>) -> Result<Option<FillModelAny>, ManifestError> {
    let config = config.unwrap();
    let random_seed = config
        .random_seed
        .ok_or(ManifestError::MissingField("venue.fill_model.random_seed"))?;
    let model = ProbabilisticFillModel::new(0.7, 0.1, Some(random_seed))?;
    Ok(Some(FillModelAny::Probabilistic(model)))
}

fn resolve_latency_model(config: Option<&ManifestLatencyModelConfig>) -> Result<Option<LatencyModelAny>, ManifestError> {
    Ok(Some(LatencyModelAny::Static(StaticLatencyModel::new(a, b, c, d))))
}

fn resolve_fee_model(config: Option<&ManifestFeeModelConfig>) -> Result<Option<FeeModelAny>, ManifestError> {
    Ok(Some(FeeModelAny::MakerTaker(MakerTakerFeeModel)))
}

fn ensure_supported_enums(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    resolve_fill_model(manifest.venue.fill_model.as_ref())?;
    Ok(())
}

impl BacktestingRunManifest {
    fn to_nt_venue_config(&self) -> Result<BacktestVenueConfig, ManifestError> {
        BacktestVenueConfig::builder()
            .maybe_fill_model(resolve_fill_model(self.venue.fill_model.as_ref())?)
            .maybe_latency_model(resolve_latency_model(self.venue.latency_model.as_ref())?)
            .maybe_fee_model(resolve_fee_model(self.venue.fee_model.as_ref())?)
            .build()
    }

    fn resolved_nt_surfaces(&self) {
        resolved_surface(
            "venue.fill_model",
            NtSurfaceClassification::PassThrough,
            "BacktestVenueConfig.fill_model",
            option_value(venue.fill_model()),
        );
    }
}

fn venue_config_registers_polymarket_cost_realism_models_with_nt() {}
fn rejects_unknown_polymarket_cost_realism_model_selectors() {}
fn rejects_invalid_polymarket_cost_realism_parameters() {}
"""


def write_common(root: Path, *, run_manifest: str | None = None) -> None:
    write(
        root / "crates/backtesting-vertical-slice/src/run_manifest.rs",
        compliant_run_manifest() if run_manifest is None else run_manifest,
    )
    write(
        root / "crates/backtesting-vertical-slice/Cargo.toml",
        'nautilus-execution = { git = "https://github.com/nautechsystems/nautilus_trader.git" }\n',
    )
    write(
        root / "justfile",
        """source-fence-static:
    python3 scripts/test_verify_ra_cost_realism.py
    python3 scripts/verify_ra_cost_realism.py
""",
    )


def test_compliant_fixture_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root)
        findings = verifier.scan_root(root)
        assert findings == []


def test_comments_and_strings_only_do_not_satisfy_code_patterns() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(
            root,
            run_manifest="""
// pub struct ManifestFillModelConfig
// fn resolve_fill_model() { FillModelAny::Probabilistic(ProbabilisticFillModel::new()) }
const TOKEN_STUFFING: &str = "maybe_fill_model(resolve_fill_model(venue.fill_model.as_ref()))";
pub const UNSUPPORTED_NT_VENUE_SURFACES: &[&str] = &["leverages"];
""",
        )
        findings = verifier.scan_root(root)
        assert any("missing real ManifestFillModelConfig" in finding for finding in findings)
        assert any("missing real BTE fill registration" in finding for finding in findings)


def test_model_fields_must_not_remain_in_unsupported_surface_block() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        run_manifest = compliant_run_manifest().replace(
            '"settlement_prices",',
            '"fill_model",\n    "settlement_prices",',
        )
        write_common(root, run_manifest=run_manifest)
        findings = verifier.scan_root(root)
        assert any('"fill_model" must not remain unsupported' in finding for finding in findings)


def test_probabilistic_fill_model_seed_guard_is_required() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        run_manifest = compliant_run_manifest().replace(
            """
    let random_seed = config
        .random_seed
        .ok_or(ManifestError::MissingField("venue.fill_model.random_seed"))?;
""",
            "",
        )
        write_common(root, run_manifest=run_manifest)
        findings = verifier.scan_root(root)
        assert any("probabilistic fill model seed guard" in finding for finding in findings)


def main() -> int:
    test_compliant_fixture_passes()
    test_comments_and_strings_only_do_not_satisfy_code_patterns()
    test_model_fields_must_not_remain_in_unsupported_surface_block()
    test_probabilistic_fill_model_seed_guard_is_required()
    print("OK: verify_ra_cost_realism self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
