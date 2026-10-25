use std::process::Command;

#[test]
fn version_output_is_stable() {
    let output = Command::new(env!("CARGO_BIN_EXE_download-manager-cli"))
        .arg("--version")
        .output()
        .expect("CLI should start");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "download-manager-cli 0.1.0\n"
    );
    assert_eq!(output.stderr, b"");
}
