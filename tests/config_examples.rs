use std::{fs, process::Command};

#[test]
fn renderer_staged_temp_files_are_gitignored() {
    let status = Command::new("git")
        .args(["check-ignore", "-q", "config/.live.toml.tmp-123-456"])
        .status()
        .expect("git check-ignore should run");

    assert!(
        status.success(),
        "renderer staged temp files should be gitignored"
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
