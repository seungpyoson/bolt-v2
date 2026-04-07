use std::process::Command;

use bolt_v2::config::Config;

#[test]
fn renderer_staged_temp_files_under_config_are_gitignored() {
    let top_level = Command::new("git")
        .args(["check-ignore", "-q", "config/.live.toml.tmp-123-456"])
        .status()
        .expect("git check-ignore should run");
    let nested = Command::new("git")
        .args(["check-ignore", "-q", "config/nested/.live.toml.tmp-123-456"])
        .status()
        .expect("git check-ignore should run");

    assert!(
        top_level.success(),
        "top-level renderer staged temp files should be gitignored"
    );
    assert!(
        nested.success(),
        "nested renderer staged temp files should be gitignored"
    );
}

#[test]
fn operator_input_example_renders_to_runtime_schema() {
    let rendered = bolt_v2::render_live_config_from_path(
        std::path::Path::new("config/live.local.example.toml"),
        std::path::Path::new("config/live.toml"),
    )
    .expect("operator input example should render");

    let cfg: Config = toml::from_str(&rendered).expect("rendered runtime config should parse");
    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 1);
}

#[test]
fn wrapper_fixture_parses_as_runtime_schema() {
    let cfg = Config::load(std::path::Path::new(
        "config/examples/polymarket-exec-tester.toml",
    ))
    .expect("wrapper fixture should parse");

    assert_eq!(cfg.data_clients.len(), 1);
    assert_eq!(cfg.exec_clients.len(), 1);
    assert_eq!(cfg.strategies.len(), 1);
}
