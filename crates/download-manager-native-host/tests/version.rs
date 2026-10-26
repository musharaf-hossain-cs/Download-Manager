use std::{
    io::Write,
    process::{Command, Stdio},
};

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

#[test]
fn authorized_browser_origin_receives_health_response() {
    const EXTENSION_ID: &str = "abcdefghijklmnopabcdefghijklmnop";
    let body = format!(
        r#"{{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","nonce":"abcdefghijklmnop","extension_id":"{EXTENSION_ID}","payload":{{"type":"health"}}}}"#
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_download-manager-native-host"))
        .arg(format!("chrome-extension://{EXTENSION_ID}/"))
        .arg("--parent-window=0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("native host should start");
    let mut stdin = child.stdin.take().expect("child stdin");
    stdin
        .write_all(
            &u32::try_from(body.len())
                .expect("fixture length")
                .to_le_bytes(),
        )
        .expect("write frame length");
    stdin.write_all(body.as_bytes()).expect("write frame body");
    drop(stdin);

    let output = child.wait_with_output().expect("native host should exit");
    assert!(output.status.success());
    let length = u32::from_le_bytes(output.stdout[..4].try_into().expect("response length"));
    assert_eq!(
        usize::try_from(length).expect("response length"),
        output.stdout.len() - 4
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout[4..]).expect("response JSON");
    assert_eq!(response["payload"]["type"], "healthy");
    assert_eq!(response["payload"]["host_version"], "0.1.0");
}
