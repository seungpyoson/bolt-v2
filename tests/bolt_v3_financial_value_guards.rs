use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn rustc() -> String {
    env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string())
}

fn deps_dir() -> PathBuf {
    let current_exe = env::current_exe().expect("test executable path should be available");
    current_exe
        .parent()
        .expect("test executable should live in target deps dir")
        .to_path_buf()
}

fn newest_bolt_v2_rlib(deps: &Path) -> PathBuf {
    let mut candidates = fs::read_dir(deps)
        .expect("target deps dir should be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libbolt_v2-"))
                && path.extension().and_then(|ext| ext.to_str()) == Some("rlib")
        })
        .map(|path| {
            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .expect("rlib metadata should expose modified time");
            (modified, path)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates
        .into_iter()
        .map(|(_, path)| path)
        .next()
        .expect("compiled bolt_v2 rlib should be present beside integration tests")
}

fn compile_with_bolt_v2(crate_name: &str, source: &str) -> Output {
    let temp_dir = tempfile::tempdir().expect("tempdir should be creatable");
    let source_path = temp_dir.path().join(format!("{crate_name}.rs"));
    fs::write(&source_path, source).expect("probe source should be writable");

    let deps = deps_dir();
    let rlib = newest_bolt_v2_rlib(&deps);
    Command::new(rustc())
        .arg("--edition=2024")
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--emit=metadata")
        .arg("--out-dir")
        .arg(temp_dir.path())
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("--extern")
        .arg(format!("bolt_v2={}", rlib.display()))
        .arg(source_path)
        .output()
        .expect("rustc probe should execute")
}

fn compile_standalone(crate_name: &str, source: &str) -> Output {
    let temp_dir = tempfile::tempdir().expect("tempdir should be creatable");
    let source_path = temp_dir.path().join(format!("{crate_name}.rs"));
    fs::write(&source_path, source).expect("probe source should be writable");

    Command::new(rustc())
        .arg("--edition=2024")
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--emit=metadata")
        .arg("--out-dir")
        .arg(temp_dir.path())
        .arg(source_path)
        .output()
        .expect("rustc probe should execute")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn valid_probability_value_const_constructor_compiles() {
    let output = compile_with_bolt_v2(
        "valid_probability_value_const_constructor",
        r#"
use bolt_v2::bolt_v3_numeric::ProbabilityValue;

const ZERO_PROBABILITY: ProbabilityValue = match ProbabilityValue::try_from_unit(0.0) {
    Some(value) => value,
    None => panic!("zero probability should be constructable"),
};
const ONE_PROBABILITY: ProbabilityValue = match ProbabilityValue::try_from_unit(1.0) {
    Some(value) => value,
    None => panic!("unit probability should be constructable"),
};

fn main() {
    let _ = (ZERO_PROBABILITY.get(), ONE_PROBABILITY.get());
}
"#,
    );

    assert!(
        output.status.success(),
        "valid ProbabilityValue constructor probe must compile:\n{}",
        stderr(&output)
    );
}

#[test]
fn non_finite_probability_value_const_construction_fails_closed() {
    let output = compile_with_bolt_v2(
        "non_finite_probability_value_const_construction",
        r#"
use bolt_v2::bolt_v3_numeric::ProbabilityValue;

const BAD_NAN: ProbabilityValue = match ProbabilityValue::try_from_unit(f64::NAN) {
    Some(value) => value,
    None => panic!("non-finite probability rejected"),
};

fn main() {
    let _ = BAD_NAN.get();
}
"#,
    );
    let stderr = stderr(&output);

    assert!(
        !output.status.success(),
        "non-finite probability must not compile"
    );
    assert!(
        stderr.contains("panicked") && stderr.contains("non-finite probability rejected"),
        "non-finite rejection must be the const-constructor failure, got:\n{stderr}"
    );
}

#[test]
fn out_of_range_probability_value_const_construction_fails_closed() {
    let output = compile_with_bolt_v2(
        "out_of_range_probability_value_const_construction",
        r#"
use bolt_v2::bolt_v3_numeric::ProbabilityValue;

const BAD_OVER_UNIT: ProbabilityValue = match ProbabilityValue::try_from_unit(1.000_001) {
    Some(value) => value,
    None => panic!("out-of-range probability rejected"),
};

fn main() {
    let _ = BAD_OVER_UNIT.get();
}
"#,
    );
    let stderr = stderr(&output);

    assert!(
        !output.status.success(),
        "out-of-range probability must not compile"
    );
    assert!(
        stderr.contains("panicked") && stderr.contains("out-of-range probability rejected"),
        "out-of-range rejection must be the const-constructor failure, got:\n{stderr}"
    );
}

#[test]
fn financial_value_type_default_compile_fails() {
    for (crate_name, source, expected_type) in [
        (
            "probability_value_default_probe",
            r#"
use bolt_v2::bolt_v3_numeric::ProbabilityValue;

fn main() {
    let _ = ProbabilityValue::default();
}
"#,
            "ProbabilityValue",
        ),
        (
            "usable_mu_default_probe",
            r#"
use bolt_v2::bolt_v3_maker_mu_estimator::UsableMu;

fn main() {
    let _ = UsableMu::default();
}
"#,
            "UsableMu",
        ),
        (
            "valid_realized_vol_default_probe",
            r#"
use bolt_v2::bolt_v3_realized_volatility::ValidRealizedVol;

fn main() {
    let _ = ValidRealizedVol::default();
}
"#,
            "ValidRealizedVol",
        ),
    ] {
        let output = compile_with_bolt_v2(crate_name, source);
        let stderr = stderr(&output);

        assert!(
            !output.status.success(),
            "{expected_type}::default() must not compile"
        );
        assert!(
            stderr.contains(expected_type) && stderr.contains("default"),
            "{expected_type} failure must name the missing default constructor, got:\n{stderr}"
        );
    }
}

#[test]
fn synthetic_default_readd_fence_rejects_derive_default() {
    let output = compile_standalone(
        "synthetic_default_readd_fence",
        r#"
trait NoDefaultProbe {
    fn financial_value_default_readd_fence();
}

trait DefaultProbe {
    fn financial_value_default_readd_fence();
}

impl<T: Default> DefaultProbe for T {
    fn financial_value_default_readd_fence() {}
}

use DefaultProbe as _;
use NoDefaultProbe as _;

macro_rules! macro_generated_financial_value {
    ($name:ident) => {
        #[derive(Default)]
        struct $name(f64);

        impl NoDefaultProbe for $name {
            fn financial_value_default_readd_fence() {}
        }

        const _: fn() = $name::financial_value_default_readd_fence;
    };
}

#[cfg_attr(all(), derive(Default))]
struct CfgAttrFinancialValue(f64);

impl NoDefaultProbe for CfgAttrFinancialValue {
    fn financial_value_default_readd_fence() {}
}

const _: fn() = CfgAttrFinancialValue::financial_value_default_readd_fence;

macro_generated_financial_value!(MacroFinancialValue);

fn main() {}
"#,
    );
    let stderr = stderr(&output);

    assert!(
        !output.status.success(),
        "synthetic derive(Default) re-add must be rejected"
    );
    assert!(
        stderr.contains("multiple applicable items"),
        "synthetic derive(Default) fence must fail by method-resolution ambiguity, got:\n{stderr}"
    );
}
