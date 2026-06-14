use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
};

use anyhow::Result;
use backtesting_vertical_slice::leadlag_catalog_reader::{
    LeadLagCatalogReadConfig, read_leadlag_top_of_book_from_catalog,
    read_leadlag_trades_from_catalog,
};
use clap::{Parser, ValueEnum};

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    kind: LeadLagExtractKind,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LeadLagExtractKind {
    Tob,
    Trades,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = LeadLagCatalogReadConfig::from_toml_file(&cli.config)?;
    let mut writer: Box<dyn Write> = match cli.output {
        Some(path) => Box::new(BufWriter::new(File::create(path)?)),
        None => Box::new(BufWriter::new(io::stdout())),
    };

    match cli.kind {
        LeadLagExtractKind::Tob => {
            for row in read_leadlag_top_of_book_from_catalog(&config)? {
                serde_json::to_writer(&mut writer, &row)?;
                writer.write_all(b"\n")?;
            }
        }
        LeadLagExtractKind::Trades => {
            for row in read_leadlag_trades_from_catalog(&config)? {
                serde_json::to_writer(&mut writer, &row)?;
                writer.write_all(b"\n")?;
            }
        }
    }

    writer.flush()?;
    Ok(())
}
