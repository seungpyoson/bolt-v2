use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use bolt_v2::live_config::{render_runtime_config, LiveLocalConfig};
use clap::Parser;

#[derive(Parser)]
#[command(name = "render_live_config")]
struct Cli {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let input = LiveLocalConfig::load(&cli.input)?;
    let rendered = render_runtime_config(&input, &cli.input, &cli.output)?;

    let existed = cli.output.exists();
    let existing = std::fs::read_to_string(&cli.output).ok();
    let changed = existing.as_deref() != Some(rendered.as_str());

    write_output(&cli.output, &rendered)?;

    if !changed {
        println!("Generated config unchanged: {}", cli.output.display());
    } else if existed {
        println!(
            "Generated config drift detected, rewrote {} from {}",
            cli.output.display(),
            cli.input.display()
        );
    } else {
        println!(
            "Generated {} from {}",
            cli.output.display(),
            cli.input.display()
        );
    }

    Ok(())
}

fn write_output(path: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let staged = staged_output_path(path)?;
    std::fs::write(&staged, contents)?;
    set_read_only(&staged)?;

    #[cfg(windows)]
    if path.exists() {
        set_writable(path)?;
        std::fs::remove_file(path)?;
    }

    std::fs::rename(&staged, path)?;
    Ok(())
}

fn staged_output_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Output path has no parent: {}", path.display()))?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Output path has no file name: {}", path.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(parent.join(format!(
        ".{}.tmp-{}-{}",
        filename,
        std::process::id(),
        stamp
    )))
}

#[cfg(windows)]
fn set_writable(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
fn set_read_only(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o444);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_read_only(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}
