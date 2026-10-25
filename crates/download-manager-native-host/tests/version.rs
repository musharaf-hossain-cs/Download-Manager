use std::process::Command;

#[test]
fn version_output_is_stable() {
    let output = Command::new(env!("CARGO_BIN_EXE_download-manager-native-host"))
        .arg("--version")
        .output()
        .expect("native host should start");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "download-manager-native-host 0.1.0\n"
    );
    assert_eq!(output.stderr, b"");
}
