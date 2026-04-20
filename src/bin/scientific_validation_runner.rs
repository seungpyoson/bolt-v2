use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use toml::Value as TomlValue;

#[derive(Debug, Parser)]
#[command(name = "scientific_validation_runner")]
struct Cli {
    #[arg(long)]
    descriptor: PathBuf,
    #[arg(long)]
    subject_root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct BenchmarkDescriptor {
    benchmark_id: String,
    fixture_ref: String,
    runner_ref: String,
    expected_outcome: String,
    seed: BenchmarkSeed,
}

#[derive(Debug, Deserialize)]
struct BenchmarkSeed {
    kind: String,
    targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureSet {
    files: Vec<FixtureFile>,
}

#[derive(Debug, Deserialize)]
struct FixtureFile {
    path: String,
    contents: String,
}

#[derive(Debug, Deserialize)]
struct RunnerSpec {
    stage: String,
    expected_outcome: String,
}

#[derive(Debug)]
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("clock should move forward")?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bolt-v2-{label}-{nanos}"));
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create temp dir {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn load_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_slice(&bytes).with_context(|| format!("failed to parse TOML {}", path.display()))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn resolve_repo_relative(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root().join(path)
    }
}

fn materialize_fixture(fixture: &FixtureSet, dst: &Path) -> Result<()> {
    for file in &fixture.files {
        let path = dst.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, &file.contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

#[derive(Debug)]
struct Segment {
    key: String,
    index: Option<usize>,
}

fn parse_segments(path: &str) -> Result<Vec<Segment>> {
    path.split('.')
        .map(|segment| {
            if let Some((key, remainder)) = segment.split_once('[') {
                let index = remainder
                    .strip_suffix(']')
                    .context("path segment index must end with `]`")?
                    .parse::<usize>()
                    .context("path segment index must be numeric")?;
                Ok(Segment {
                    key: key.to_string(),
                    index: Some(index),
                })
            } else {
                Ok(Segment {
                    key: segment.to_string(),
                    index: None,
                })
            }
        })
        .collect()
}

fn descend_mut<'a>(value: &'a mut TomlValue, segment: &Segment) -> Result<&'a mut TomlValue> {
    let table = value
        .as_table_mut()
        .with_context(|| format!("expected table before `{}`", segment.key))?;
    let current = table
        .get_mut(&segment.key)
        .with_context(|| format!("missing key `{}`", segment.key))?;
    if let Some(index) = segment.index {
        let array = current
            .as_array_mut()
            .with_context(|| format!("`{}` is not an array", segment.key))?;
        array
            .get_mut(index)
            .with_context(|| format!("array index {} out of bounds for `{}`", index, segment.key))
    } else {
        Ok(current)
    }
}

fn set_toml_path(root: &mut TomlValue, path: &str, new_value: TomlValue) -> Result<()> {
    let segments = parse_segments(path)?;
    let (parent_segments, final_segment) = segments
        .split_last()
        .context("toml path must contain at least one segment")?;

    let mut current = root;
    for segment in final_segment {
        current = descend_mut(current, segment)?;
    }

    if let Some(index) = parent_segments.index {
        let table = current
            .as_table_mut()
            .with_context(|| format!("expected table before `{}`", parent_segments.key))?;
        let array_value = table
            .get_mut(&parent_segments.key)
            .with_context(|| format!("missing key `{}`", parent_segments.key))?;
        let array = array_value
            .as_array_mut()
            .with_context(|| format!("`{}` is not an array", parent_segments.key))?;
        if index >= array.len() {
            bail!(
                "array index {} out of bounds for `{}`",
                index,
                parent_segments.key
            );
        }
        array[index] = new_value;
    } else {
        let table = current
            .as_table_mut()
            .with_context(|| format!("expected table before `{}`", parent_segments.key))?;
        table.insert(parent_segments.key.clone(), new_value);
    }
    Ok(())
}

fn parse_assignment(target: &str) -> Result<(String, String, TomlValue)> {
    let (file_part, rest) = target
        .split_once('#')
        .with_context(|| format!("target `{target}` must be `<file>#<path> = <value>`"))?;
    let (toml_path, rhs) = rest
        .split_once('=')
        .with_context(|| format!("target `{target}` must contain `=`"))?;
    let rhs = rhs.trim();
    let wrapper = format!("value = {rhs}");
    let parsed = toml::from_str::<TomlValue>(&wrapper)
        .with_context(|| format!("target `{target}` has invalid TOML rhs"))?;
    let value = parsed
        .get("value")
        .cloned()
        .context("parsed rhs should contain `value`")?;
    Ok((
        file_part.trim().to_string(),
        toml_path.trim().to_string(),
        value,
    ))
}

fn apply_toml_set_targets(package_root: &Path, targets: &[String]) -> Result<()> {
    for target in targets {
        let (file_part, toml_path, new_value) = parse_assignment(target)?;
        let file_path = package_root.join(&file_part);
        let mut value = load_toml::<TomlValue>(&file_path)?;
        set_toml_path(&mut value, &toml_path, new_value)?;
        let serialized = toml::to_string(&value)
            .with_context(|| format!("failed to serialize {}", file_path.display()))?;
        fs::write(&file_path, serialized)
            .with_context(|| format!("failed to write {}", file_path.display()))?;
    }
    Ok(())
}

fn run_subject_validator(
    subject_root: &Path,
    delivery_dir: &Path,
    stage: &str,
) -> Result<std::process::Output> {
    Command::new("cargo")
        .current_dir(repo_root())
        .args([
            "run",
            "--quiet",
            "--manifest-path",
            subject_root
                .join("Cargo.toml")
                .to_str()
                .context("subject Cargo.toml path must be valid utf-8")?,
            "--bin",
            "process_validator",
            "--",
            "--delivery-dir",
            delivery_dir
                .to_str()
                .context("delivery dir path must be valid utf-8")?,
            "--stage",
            stage,
        ])
        .output()
        .context("failed to execute subject validator")
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let descriptor_path = resolve_repo_relative(&cli.descriptor);
    let descriptor = load_toml::<BenchmarkDescriptor>(&descriptor_path)?;
    let fixture_path = resolve_repo_relative(Path::new(&descriptor.fixture_ref));
    let runner_path = resolve_repo_relative(Path::new(&descriptor.runner_ref));
    let fixture = load_toml::<FixtureSet>(&fixture_path)?;
    let runner = load_toml::<RunnerSpec>(&runner_path)?;

    if descriptor.seed.kind != "toml_set" {
        bail!(
            "descriptor {} uses unsupported seed kind `{}`",
            descriptor.benchmark_id,
            descriptor.seed.kind
        );
    }
    if descriptor.expected_outcome != runner.expected_outcome {
        bail!(
            "descriptor {} expected_outcome `{}` does not match runner `{}`",
            descriptor.benchmark_id,
            descriptor.expected_outcome,
            runner.expected_outcome
        );
    }

    let temp = TempDir::new(&descriptor.benchmark_id)?;
    materialize_fixture(&fixture, temp.path())?;
    apply_toml_set_targets(temp.path(), &descriptor.seed.targets)?;

    let output = run_subject_validator(&cli.subject_root, temp.path(), &runner.stage)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    let observed_fail_closed = !output.status.success() && combined.contains("STATUS: BLOCK");
    if runner.expected_outcome == "fail_closed" && observed_fail_closed {
        print!("{stdout}");
        eprint!("{stderr}");
        return Ok(());
    }

    bail!(
        "benchmark {} did not observe expected `{}` outcome\nstdout:\n{}\nstderr:\n{}",
        descriptor.benchmark_id,
        runner.expected_outcome,
        stdout,
        stderr
    );
}
