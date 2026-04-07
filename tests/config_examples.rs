use std::{fs, process::Command};

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
fn operator_input_example_identifies_itself_as_operator_input() {
    let contents = fs::read_to_string("config/live.local.example.toml")
        .expect("operator input example should be readable");

    assert!(
        contents.contains("Operator input template"),
        "operator input example should explain that it is the human-edited template"
    );
}

#[test]
fn wrapper_fixture_identifies_itself_as_test_fixture() {
    let contents = fs::read_to_string("config/examples/polymarket-exec-tester.toml")
        .expect("wrapper fixture should be readable");

    assert!(
        contents.contains("Wrapper-format test fixture"),
        "wrapper fixture should explain that it is not the operator input template"
    );
}
