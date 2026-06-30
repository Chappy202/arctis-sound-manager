use std::process::Command;

#[test]
fn version_flag_prints_crate_version() {
    let bin = env!("CARGO_BIN_EXE_asm-cli");
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .expect("run asm-cli");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected version in output, got: {stdout:?}"
    );
}
