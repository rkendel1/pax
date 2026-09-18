use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

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

#[test]
fn info_json_detects_python_rust_and_docker_components() {
    let root = temp_dir();
    write(
        &root.join("services/api/pyproject.toml"),
        "[project]\nname = \"api\"\ndependencies = [\"pytest\"]\n",
    );
    write(&root.join("services/api/uv.lock"), "version = 1\n");
    write(
        &root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core\"\n[dependencies]\nserde = \"1\"\n[dev-dependencies]\ncriterion = \"1\"\n",
    );
    write(&root.join("crates/core/Cargo.lock"), "");
    write(
        &root.join("Dockerfile"),
        "FROM rust:latest\nWORKDIR /app\nEXPOSE 8080\n",
    );
    write(
        &root.join("compose.yaml"),
        "services:\n  api:\n    image: postgres:16\n  redis:\n    image: redis:7\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "info"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let components = value["components"].as_array().unwrap();
    assert!(
        components
            .iter()
            .any(|item| item["ecosystem"] == "python" && item["tool"] == "uv")
    );
    assert!(
        components
            .iter()
            .any(|item| item["ecosystem"] == "rust" && item["tool"] == "cargo")
    );
    assert!(
        components
            .iter()
            .any(|item| item["ecosystem"] == "container" && item["tool"] == "docker")
    );
    assert_eq!(value["container"]["services"][0], "api");
    assert_eq!(value["container"]["images"][1], "redis:7");
    assert_eq!(value["container"]["directives"]["WORKDIR"][0], "/app");
}

#[cfg(unix)]
#[test]
fn run_delegates_to_detected_manager_and_preserves_exit_code() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"npm@10.9.0"}"#,
    );
    let npm = bin.join("npm");
    write(
        &npm,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$1\" \"$2\"\n[ \"$2\" = fail ] && exit 7\nexit 0\n",
    );
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), path.to_string_lossy());

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["run", "dev", "--watch"])
        .current_dir(&root)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}|run|dev", root.display())
    );

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["run", "fail"])
        .current_dir(&root)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}
