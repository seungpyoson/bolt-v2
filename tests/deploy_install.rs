use std::{fs, path::PathBuf};

#[test]
fn install_script_assigns_bolt_home_to_runtime_user() {
    let script_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let script = fs::read_to_string(&script_path).expect("install script should exist");

    assert!(
        script.contains("chown \"${BOLT_USER}:${BOLT_GROUP}\" \"${BOLT_HOME}\""),
        "install script must make the working directory writable by the runtime user"
    );
}

#[test]
fn install_script_has_failclosed_prologue() {
    // Pin install.sh's fail-closed prologue: a regression dropping `set -u`
    // (an unset BOLT_USER would expand to empty and proceed into host
    // mutation) or the `${BOLT_DATA_DEVICE:?}` required-input guard must
    // fail CI even where the script cannot be executed in the sandbox.
    let script_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let script = fs::read_to_string(&script_path).expect("install script should exist");
    assert!(
        script.starts_with("#!/usr/bin/env bash\nset -euo pipefail"),
        "install.sh must open with the fail-closed prologue `set -euo pipefail`"
    );
    assert!(
        script.contains("${BOLT_DATA_DEVICE:?"),
        "install.sh must guard BOLT_DATA_DEVICE as a required input (`:?`) before any mutation"
    );
}
