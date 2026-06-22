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

#[test]
fn install_script_repairs_deploy_toml_when_present() {
    // deploy.toml is host-specific (the operator places it; install.sh does not
    // copy it), but the bolt service user reads it at TargetVerify. A restrictive
    // deploy-shell umask would otherwise lock it out (same #768 lockout class as
    // the rest of the config bundle). install.sh must conditionally append it to
    // `config_bundle_files` so the repair loop fixes its perms when present.
    let script_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/install.sh");
    let script = fs::read_to_string(&script_path).expect("install script should exist");
    assert!(
        script.contains("if [[ -f \"${BOLT_INSTALL_ROOT}/config/deploy.toml\" ]]; then")
            && script
                .contains("config_bundle_files+=(\"${BOLT_INSTALL_ROOT}/config/deploy.toml\")"),
        "install.sh must conditionally repair config/deploy.toml when present (#768 lockout class)"
    );
}
