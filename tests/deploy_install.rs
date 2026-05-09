use std::fs;

#[test]
fn install_script_assigns_bolt_home_to_runtime_user() {
    let script_path = std::env::current_dir()
        .expect("cargo should run tests from the package root")
        .join("deploy/install.sh");
    let script = fs::read_to_string(&script_path).expect("install script should exist");

    assert!(
        script.contains("chown \"${BOLT_USER}:${BOLT_GROUP}\" \"${BOLT_HOME}\""),
        "install script must make the working directory writable by the runtime user"
    );
}
