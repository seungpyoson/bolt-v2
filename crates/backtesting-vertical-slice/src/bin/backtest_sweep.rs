use std::path::PathBuf;

use anyhow::{Result, ensure};
use backtesting_vertical_slice::research_analytics::{
    BacktestSweepPublicationPlan, BacktestSweepSourcePair, run_backtest_sweep_publication,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    about = "Run a backtest sweep from run-spec/object pairs and publish a run-pointer index"
)]
struct Cli {
    #[arg(long)]
    input_dir: PathBuf,
    #[arg(long)]
    run_spec_dir: PathBuf,
    #[arg(long)]
    run_output_dir: PathBuf,
    #[arg(long)]
    artifact_root: String,
    #[arg(long)]
    index_path: PathBuf,
    #[arg(long = "source", value_names = ["RUN_SPEC_TOML", "OBJECT"], num_args = 2, required = true)]
    sources: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let plan = BacktestSweepPublicationPlan {
        input_dir: cli.input_dir,
        run_spec_dir: cli.run_spec_dir,
        run_output_dir: cli.run_output_dir,
        artifact_root: cli.artifact_root,
        index_path: cli.index_path,
        sources: source_pairs(&cli.sources)?,
    };
    let publication = run_backtest_sweep_publication(&plan)?;

    println!(
        "run_pointer_index = {}",
        publication.index_artifact.path.display()
    );
    println!("content_hash = {}", publication.index_artifact.content_hash);
    println!("bytes = {}", publication.index_artifact.bytes);
    println!("runs = {}", publication.index.runs.len());
    Ok(())
}

fn source_pairs(paths: &[PathBuf]) -> Result<Vec<BacktestSweepSourcePair>> {
    ensure!(
        paths.len() % 2 == 0,
        "--source arguments must be run-spec/object pairs"
    );
    paths
        .chunks_exact(2)
        .map(|chunk| {
            Ok(BacktestSweepSourcePair {
                run_spec_path: chunk[0].clone(),
                object_path: chunk[1].clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn source_pairs_chunks_paths_in_run_spec_object_order() {
        let pairs = super::source_pairs(&[
            PathBuf::from("first.toml"),
            PathBuf::from("first.object"),
            PathBuf::from("second.toml"),
            PathBuf::from("second.object"),
        ])
        .expect("pairs parse");

        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].run_spec_path, PathBuf::from("first.toml"));
        assert_eq!(pairs[0].object_path, PathBuf::from("first.object"));
        assert_eq!(pairs[1].run_spec_path, PathBuf::from("second.toml"));
        assert_eq!(pairs[1].object_path, PathBuf::from("second.object"));
    }

    #[test]
    fn source_pairs_rejects_odd_path_count() {
        let err = super::source_pairs(&[
            PathBuf::from("first.toml"),
            PathBuf::from("first.object"),
            PathBuf::from("second.toml"),
        ])
        .expect_err("odd path count must fail");

        assert!(err.to_string().contains("run-spec/object pairs"), "{err}");
    }
}
