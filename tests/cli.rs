use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir() -> PathBuf {
    let unique = format!(
        "pax-cli-tests-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn info_json_reports_detected_manager() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"pnpm@10.15.0"}"#,
    );
    write(&root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n");

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "info"])
        .current_dir(&root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["command"], "info");
    assert_eq!(value["manager"]["name"], "pnpm");
    assert_eq!(value["manager"]["version"], "10.15.0");
}

#[test]
fn doctor_text_reports_expected_checks() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"npm@10.9.0"}"#,
    );
    write(&root.join("package-lock.json"), "{}");

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .arg("doctor")
        .current_dir(&root)
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("PAX Doctor"));
    assert!(stdout.contains("✓ package.json"));
    assert!(stdout.contains("✓ package/lock consistency"));
}

#[test]
fn inspection_commands_report_package_json_data_as_json() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{
          "name": "sample",
          "dependencies": {"react": "^19.0.0"},
          "devDependencies": {"typescript": "^5.0.0"},
          "scripts": {"build": "tsc"},
          "workspaces": ["packages/*"]
        }"#,
    );
    write(&root.join("package-lock.json"), "{}");

    for command in ["deps", "scripts", "workspaces", "lock"] {
        let output = Command::new(env!("CARGO_BIN_EXE_pax"))
            .args(["--json", command])
            .current_dir(&root)
            .output()
            .unwrap();

        assert!(output.status.success(), "{command} failed");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["command"], command);
        assert_eq!(value["project"]["name"], "sample");

        match command {
            "deps" => assert_eq!(value["dependencies"]["dependencies"]["react"], "^19.0.0"),
            "scripts" => assert_eq!(value["scripts"]["build"], "tsc"),
            "workspaces" => assert_eq!(value["workspaces"]["packages"][0], "packages/*"),
            "lock" => assert_eq!(value["lock"]["selected"], "package-lock.json"),
            _ => unreachable!(),
        }
    }
}
