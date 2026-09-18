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

#[cfg(unix)]
#[test]
fn x_delegates_to_native_package_runner() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"pnpm@10.0.0"}"#,
    );
    let pnpm = bin.join("pnpm");
    write(
        &pnpm,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$1\" \"$2\"\nexit 0\n",
    );
    fs::set_permissions(&pnpm, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["x", "prettier", "--check"])
        .current_dir(&root)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), path.to_string_lossy()),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}|dlx|prettier", root.display())
    );
}

#[cfg(unix)]
#[test]
fn x_install_delegates_python_requirements_to_pip() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(&root.join("requirements.txt"), "requests==2.32.0\n");
    let pip = bin.join("pip");
    write(
        &pip,
        "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$PWD\" \"$1\" \"$2\" \"$3\"\nexit 0\n",
    );
    fs::set_permissions(&pip, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["x", "install", "-r", "requirements.txt"])
        .current_dir(&root)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), path.to_string_lossy()),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}|install|-r|requirements.txt", root.display())
    );
}

#[cfg(unix)]
#[test]
fn install_delegates_project_and_package_installation_to_npm() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"npm@10.0.0"}"#,
    );
    let npm = bin.join("npm");
    write(
        &npm,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$1\" \"$2\"\nexit 0\n",
    );
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), path.to_string_lossy());

    for args in [vec!["install"], vec!["install", "react"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_pax"))
            .args(&args)
            .current_dir(&root)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected = if args.len() == 1 {
            format!("{}|install|", root.display())
        } else {
            format!("{}|install|react", root.display())
        };
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .ends_with(&expected)
        );
    }
}

#[cfg(unix)]
#[test]
fn add_and_remove_delegate_to_native_package_manager() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"pnpm@10.0.0"}"#,
    );
    let pnpm = bin.join("pnpm");
    write(
        &pnpm,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$1\" \"$2\"\nexit 0\n",
    );
    fs::set_permissions(&pnpm, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    for args in [["add", "react"], ["remove", "react"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_pax"))
            .args(args)
            .current_dir(&root)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), path.to_string_lossy()),
            )
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}|{}|react", root.display(), args[0])
        );
    }
}

#[test]
fn dry_run_produces_machine_readable_execution_plan() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"pnpm@10.0.0"}"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "--dry-run", "x", "vite"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["operation"], "package-run");
    assert_eq!(value["tool"], "pnpm");
    assert_eq!(value["command"][0], "pnpm");
    assert_eq!(value["command"][1], "dlx");
    assert_eq!(value["working_directory"], root.to_str().unwrap());
    assert!(
        value["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "package.json#packageManager")
    );
}

#[test]
fn command_help_describes_supported_execution_and_observation_commands() {
    for (command, expected) in [
        ("run", "Run a project task"),
        ("x", "ephemeral package"),
        ("install", "Install declared"),
        ("graph", "static component"),
        ("reality", "runtime observations"),
        ("drift", "contradictions"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pax"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{command} help failed");
        assert!(String::from_utf8_lossy(&output.stdout).contains(expected));
    }
}

#[test]
fn ambiguous_javascript_tools_fail_closed_without_override() {
    let root = temp_dir();
    write(&root.join("package.json"), r#"{"name":"sample"}"#);
    write(&root.join("package-lock.json"), "{}");
    write(&root.join("pnpm-lock.yaml"), "lockfileVersion: '9'\n");
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["install"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("multiple JavaScript package managers")
    );
}

#[test]
fn exec_forwards_exact_command_without_detection() {
    let root = temp_dir();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["exec", "printf", "ok"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "ok");
}

#[cfg(unix)]
#[test]
fn install_orchestrates_declared_mixed_project_components() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"npm@10.0.0"}"#,
    );
    write(
        &root.join("services/api/pyproject.toml"),
        "[project]\nname = \"api\"\n",
    );
    write(&root.join("services/api/uv.lock"), "version = 1\n");
    write(
        &root.join("crates/worker/Cargo.toml"),
        "[package]\nname = \"worker\"\nversion = \"0.1.0\"\n",
    );
    write(&root.join("crates/worker/Cargo.lock"), "");
    for tool in ["npm", "uv", "cargo"] {
        let path = bin.join(tool);
        write(
            &path,
            "#!/bin/sh\nprintf '%s:%s\\n' \"$PWD\" \"$1\"\nexit 0\n",
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .arg("install")
        .current_dir(&root)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), path.to_string_lossy()),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("PAX install: 3 components"));
    assert!(stdout.contains(":install"));
    assert!(stdout.contains(":sync"));
    assert!(stdout.contains(":fetch"));
}

#[test]
fn deploy_dry_run_reports_provider_evidence_and_command() {
    let root = temp_dir();
    write(&root.join("fly.toml"), "app = \"sample\"\n");
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["deploy", "--dry-run", "--region", "iad"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("provider: fly"));
    assert!(stdout.contains("evidence: fly.toml"));
    assert!(stdout.contains("command: fly deploy --region iad"));
}

#[test]
fn deploy_json_dry_run_is_an_execution_plan() {
    let root = temp_dir();
    write(&root.join("fly.toml"), "app = \"sample\"\n");
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "--dry-run", "deploy"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ecosystem"], "deployment");
    assert_eq!(value["tool"], "fly");
    assert_eq!(value["command"][1], "deploy");
    assert_eq!(value["working_directory"], root.to_str().unwrap());
}

#[test]
fn deploy_rejects_ambiguous_provider_evidence() {
    let root = temp_dir();
    write(&root.join("fly.toml"), "app = \"sample\"\n");
    write(&root.join("vercel.json"), "{}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["deploy", "--dry-run"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("ambiguous deployment providers")
    );
}

#[test]
fn graph_json_preserves_ecosystem_dependency_semantics_and_evidence() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","dependencies":{"react":"^19"},"devDependencies":{"typescript":"^5"}}"#,
    );
    write(
        &root.join("services/api/requirements.txt"),
        "requests==2.32.0\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "graph"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["command"], "graph");
    assert_eq!(
        value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "react")
            .unwrap()["ecosystem"],
        "javascript"
    );
    assert!(value["edges"].as_array().unwrap().iter().any(|edge| {
        edge["from"] == "." && edge["to"] == "react" && edge["kind"] == "runtime-dependency"
    }));
    assert!(
        value["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["evidence"][0] == "package.json")
    );
}

#[test]
fn reality_json_is_static_and_reports_installed_state_conservatively() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"pnpm@10"}"#,
    );
    write(&root.join("pnpm-lock.yaml"), "lockfileVersion: '9'\n");

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "reality"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["runtime"]["observations"].as_array().unwrap().len(),
        0
    );
    assert_eq!(value["installed"]["observations"][0]["status"], "absent");
    assert_eq!(
        value["resolved"]["observations"][0]["evidence"][0],
        "pnpm-lock.yaml"
    );
}

#[test]
fn drift_json_reports_conflicts_and_nonzero_exit_without_mutating_files() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"pnpm@10","dependencies":{"react":"^19"}}"#,
    );
    write(&root.join("package-lock.json"), "{}");
    let before = fs::read_to_string(root.join("package.json")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "drift"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "drift");
    assert!(
        value["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["subject"] == ".")
    );
    assert_eq!(
        fs::read_to_string(root.join("package.json")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn deploy_delegates_with_explicit_tool_and_preserves_exit_code() {
    let root = temp_dir();
    let bin = root.join("bin");
    let fly = bin.join("fly");
    write(
        &fly,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$1\" \"$2\"\nexit 9\n",
    );
    fs::set_permissions(&fly, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--tool", "fly", "deploy", "--remote-only"])
        .current_dir(&root)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), path.to_string_lossy()),
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}|deploy|--remote-only", root.display())
    );
}

#[test]
fn dir_selects_project_root_for_inspection() {
    let root = temp_dir();
    write(
        &root.join("nested/package.json"),
        r#"{"name":"nested-project","packageManager":"npm@10"}"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "--dir", "nested", "info"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["project"]["name"], "nested-project");
    assert_eq!(
        value["project"]["root"],
        root.join("nested").to_str().unwrap()
    );
}

#[test]
fn help_and_version_are_successful_cli_queries() {
    for args in [["--help"], ["--version"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_pax"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
    }
}

#[test]
fn ambiguous_python_lockfiles_fail_closed_without_override() {
    let root = temp_dir();
    write(
        &root.join("pyproject.toml"),
        "[project]\nname = \"sample\"\n",
    );
    write(&root.join("uv.lock"), "version = 1\n");
    write(&root.join("poetry.lock"), "[[package]]\n");
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .arg("install")
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("ambiguous Python toolchain")
    );
}

#[cfg(unix)]
#[test]
fn delegated_arguments_after_separator_are_not_consumed_by_pax() {
    let root = temp_dir();
    let bin = root.join("bin");
    write(
        &root.join("package.json"),
        r#"{"name":"sample","packageManager":"npm@10"}"#,
    );
    let npm = bin.join("npm");
    write(&npm, "#!/bin/sh\nprintf '%s' \"$3\"\n");
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["run", "print", "--", "--json"])
        .current_dir(&root)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), path.to_string_lossy()),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "--json");
}
