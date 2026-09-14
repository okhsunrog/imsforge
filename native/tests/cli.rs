use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn cli_defaults_validation_and_atomic_save_round_trip() {
    let dir = std::env::temp_dir().join(format!("imsforge-cli-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let config = dir.join("carriers.json");
    let binary = env!("CARGO_BIN_EXE_imsforge");
    let read = Command::new(binary)
        .args(["read-config", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(read.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&read.stdout).unwrap()["auto"],
        true
    );
    let save = |text: &str| {
        let mut child = Command::new(binary)
            .args(["save-config", "--config"])
            .arg(&config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let good =
        save(r#"{"auto":false,"carriers":[{"canonical_name":"25001","ims_apn_name":"МТС IMS"}]}"#);
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let before = fs::read(&config).unwrap();
    for invalid in [
        r#"{"carriers":[{"canonical_name":"../outside"}]}"#,
        "{bad",
        r#"{"unknown":true}"#,
    ] {
        assert!(!save(invalid).status.success());
        assert_eq!(before, fs::read(&config).unwrap());
    }
    assert!(save("{}").status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&config).unwrap()).unwrap()["auto"],
        true
    );
    fs::remove_dir_all(dir).unwrap();
}
