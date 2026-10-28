//! Runtime observation boundary tests.
//!
//! Docker is replaced by a fake executable so each state is deterministic:
//! missing CLI, inaccessible daemon, successful empty Compose output, running
//! services, failed Compose inspection, and unsupported Dockerfile-only scopes.

#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pax-runtime-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn compose_project(root: &Path) {
    write(
        &root.join("compose.yaml"),
        "services:\n  web:\n    image: nginx:1\n  db:\n    image: postgres:16\n",
    );
}

struct FakeDocker {
    info_exit: i32,
    info_stderr: &'static str,
    compose_exit: i32,
    compose_stderr: &'static str,
    compose_output: &'static str,
}

impl Default for FakeDocker {
    fn default() -> Self {
        Self {
            info_exit: 0,
            info_stderr: "",
            compose_exit: 0,
            compose_stderr: "",
            compose_output: "[]",
        }
    }
}

fn fake_docker(root: &Path, fake: FakeDocker) -> PathBuf {
    let bin = root.join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let log = root.join("invoked.log");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$1\" in\n  info)\n    echo '{}' >&2\n    exit {}\n    ;;\n  compose)\n    if [ {} -ne 0 ]; then echo '{}' >&2; exit {}; fi\n    printf '%s' '{}'\n    exit 0\n    ;;\nesac\nexit 2\n",
        log.display(),
        fake.info_stderr.replace('\'', "'\\''"),
        fake.info_exit,
        fake.compose_exit,
        fake.compose_stderr.replace('\'', "'\\''"),
        fake.compose_exit,
        fake.compose_output.replace('\'', "'\\''"),
    );
    let docker = bin.join("docker");
    fs::write(&docker, script).unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn empty_bin(root: &Path) -> PathBuf {
    let bin = root.join("empty-bin");
    fs::create_dir_all(&bin).unwrap();
    bin
}

fn pax(root: &Path, bin: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(args)
        .current_dir(root)
        .env("PATH", bin)
        .output()
        .unwrap()
}

fn json(root: &Path, bin: &Path, args: &[&str]) -> Value {
    let output = pax(root, bin, args);
    assert!(
        output.status.code().is_some_and(|code| code <= 1),
        "pax {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "pax {args:?} did not emit one JSON document: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn text(root: &Path, bin: &Path, args: &[&str]) -> String {
    let output = pax(root, bin, args);
    assert!(
        output.status.code().is_some_and(|code| code <= 1),
        "pax {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn invocations(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("invoked.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn static_reality_does_not_run_docker_and_never_claims_inspection() {
    let root = temp_dir();
    compose_project(&root);
    let bin = fake_docker(&root, FakeDocker::default());

    let reality = json(&root, &bin, &["--json", "reality"]);
    assert_eq!(reality["live"], false);
    assert!(
        reality["runtime"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(reality["runtime"].get("observedAt").is_none());
    assert!(reality["runtime"].get("permission").is_none());
    assert!(invocations(&root).is_empty(), "{:?}", invocations(&root));

    let rendered = text(&root, &bin, &["reality"]);
    assert!(rendered.contains("not inspected (pass --live to authorize"));
    assert!(!rendered.contains("RUNTIME\n  inspected"));
    assert!(invocations(&root).is_empty(), "{:?}", invocations(&root));
}

#[test]
fn live_reports_scope_time_permission_and_running_services() {
    let root = temp_dir();
    compose_project(&root);
    let bin = fake_docker(
        &root,
        FakeDocker {
            compose_output: r#"[{"Service":"web","State":"running","Name":"demo-web-1"},{"Service":"db","State":"exited","ExitCode":1}]"#,
            ..FakeDocker::default()
        },
    );

    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    let runtime = &reality["runtime"];
    assert_eq!(runtime["scope"], "compose:compose.yaml");
    assert_eq!(runtime["permission"], "authorized (--live)");
    let observed_at = runtime["observedAt"].as_str().unwrap();
    assert_eq!(observed_at.len(), 20, "{observed_at}");
    assert!(observed_at.ends_with('Z'), "{observed_at}");
    assert!(observed_at.contains('T'), "{observed_at}");
    assert_eq!(
        runtime["commands"],
        serde_json::json!([
            "docker info --format {{.ServerVersion}}",
            "docker compose -f compose.yaml ps --format json"
        ])
    );

    let observations = runtime["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0]["subject"], "./web");
    assert_eq!(observations[0]["status"], "present");
    assert_eq!(observations[0]["detail"], "running");
    assert_eq!(observations[0]["scope"], "compose:compose.yaml");
    assert_eq!(
        observations[0]["evidence"],
        serde_json::json!(["compose.yaml"])
    );
    assert_eq!(observations[1]["subject"], "./db");
    assert_eq!(observations[1]["status"], "absent");
    assert_eq!(observations[1]["detail"], "not running (exited)");
    assert_eq!(
        invocations(&root),
        [
            "info --format {{.ServerVersion}}",
            "compose -f compose.yaml ps --format json"
        ]
    );

    let rendered = text(&root, &bin, &["reality", "--live"]);
    assert!(
        rendered.contains("scope compose:compose.yaml"),
        "{rendered}"
    );
    assert!(rendered.contains("observed 20"), "{rendered}");
    assert!(rendered.contains("Z,"), "{rendered}");
    assert!(
        rendered.contains("permission authorized (--live)"),
        "{rendered}"
    );
    assert!(rendered.contains("./web: present (running)"), "{rendered}");
    assert!(
        rendered.contains("./db: absent (not running (exited))"),
        "{rendered}"
    );

    let drift = json(&root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(drift["status"], "match");
    assert_eq!(drift["outcome"], "ok");
    assert!(drift["issues"].as_array().unwrap().is_empty());
    assert_eq!(drift["runtime"]["scope"], "compose:compose.yaml");
    assert_eq!(
        drift["runtime"]["observations"].as_array().unwrap(),
        observations
    );
    let rendered = text(&root, &bin, &["drift", "--live"]);
    assert!(rendered.starts_with("NO DRIFT"), "{rendered}");
    assert!(rendered.contains("./web: present (running)"), "{rendered}");
}

#[test]
fn no_running_containers_is_absent_not_an_unavailable_runtime() {
    let root = temp_dir();
    compose_project(&root);
    let bin = fake_docker(&root, FakeDocker::default());

    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    let observations = reality["runtime"]["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    for observation in observations {
        assert_eq!(observation["status"], "absent");
        assert_eq!(observation["detail"], "not running");
    }

    let drift = json(&root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(drift["status"], "match");
    assert_eq!(drift["outcome"], "ok");
    assert!(drift["issues"].as_array().unwrap().is_empty());

    let rendered = text(&root, &bin, &["drift", "--live"]);
    assert!(rendered.starts_with("NO DRIFT"), "{rendered}");
    assert!(!rendered.contains("UNKNOWN"), "{rendered}");
}

#[test]
fn missing_docker_cli_is_blocked_and_is_not_absence() {
    let root = temp_dir();
    compose_project(&root);
    let bin = empty_bin(&root);

    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    let runtime = &reality["runtime"];
    assert_eq!(runtime["failure"], "docker CLI not found on PATH");
    assert!(runtime.get("commands").is_none());
    let observation = &runtime["observations"][0];
    assert_eq!(observation["status"], "blocked");
    assert_eq!(observation["source"], "environment");
    assert_eq!(observation["failure"], "docker CLI not found on PATH");
    assert_eq!(observation["evidence"], serde_json::json!(["PATH"]));
    assert_ne!(observation["status"], "absent");
    assert_eq!(reality["outcome"], "partial");

    let rendered = text(&root, &bin, &["reality", "--live"]);
    assert!(rendered.contains("failure: docker CLI not found on PATH"));
    assert!(rendered.contains(".: blocked (docker CLI not found on PATH)"));
    assert!(!rendered.contains("absent"));
    assert!(invocations(&root).is_empty());

    let output = pax(&root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(output.status.code(), Some(0));
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["status"], "unknown");
    assert_eq!(drift["outcome"], "partial");
    assert_eq!(drift["issues"][0]["status"], "unknown");
    assert_eq!(drift["issues"][0]["actual"], "docker CLI not found on PATH");
}

#[test]
fn inaccessible_daemon_is_blocked_and_does_not_confirm_drift() {
    let root = temp_dir();
    compose_project(&root);
    let bin = fake_docker(
        &root,
        FakeDocker {
            info_exit: 1,
            info_stderr: "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
            ..FakeDocker::default()
        },
    );

    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    let runtime = &reality["runtime"];
    let failure = runtime["failure"].as_str().unwrap();
    assert!(failure.contains("daemon inaccessible"), "{failure}");
    assert!(failure.contains("Cannot connect"), "{failure}");
    assert_eq!(runtime["observations"][0]["status"], "blocked");
    assert_eq!(runtime["observations"][0]["source"], "docker");
    assert_eq!(
        runtime["observations"][0]["evidence"],
        serde_json::json!(["docker info"])
    );
    assert_eq!(
        runtime["commands"],
        serde_json::json!(["docker info --format {{.ServerVersion}}"])
    );
    // The Compose command is not attempted after the daemon probe fails.
    assert_eq!(invocations(&root), ["info --format {{.ServerVersion}}"]);
    assert_eq!(reality["outcome"], "partial");

    let rendered = text(&root, &bin, &["reality", "--live"]);
    assert!(rendered.contains(failure), "{rendered}");
    assert!(rendered.contains(".: blocked ("), "{rendered}");

    let output = pax(&root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(output.status.code(), Some(0));
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["status"], "unknown");
    assert_eq!(drift["outcome"], "partial");
    assert_eq!(drift["issues"][0]["status"], "unknown");
    assert!(
        drift["issues"][0]["actual"]
            .as_str()
            .is_some_and(|actual| actual.contains("daemon inaccessible"))
    );
}

#[test]
fn compose_failure_is_failed_and_invalid_output_is_failed() {
    let failed_root = temp_dir();
    compose_project(&failed_root);
    let bin = fake_docker(
        &failed_root,
        FakeDocker {
            compose_exit: 1,
            compose_stderr: "compose is not a docker command",
            ..FakeDocker::default()
        },
    );
    let reality = json(&failed_root, &bin, &["--json", "reality", "--live"]);
    let observation = &reality["runtime"]["observations"][0];
    assert_eq!(observation["status"], "failed");
    assert!(
        observation["failure"]
            .as_str()
            .is_some_and(|failure| failure.contains("compose ps failed"))
    );
    assert!(
        observation["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("compose is not a docker command"))
    );
    let rendered = text(&failed_root, &bin, &["reality", "--live"]);
    assert!(rendered.contains(&format!(
        "failure: {}",
        reality["runtime"]["failure"].as_str().unwrap()
    )));
    let drift = json(&failed_root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(drift["status"], "unknown");

    let invalid_root = temp_dir();
    compose_project(&invalid_root);
    let bin = fake_docker(
        &invalid_root,
        FakeDocker {
            compose_output: "this is not JSON",
            ..FakeDocker::default()
        },
    );
    let reality = json(&invalid_root, &bin, &["--json", "reality", "--live"]);
    let observation = &reality["runtime"]["observations"][0];
    assert_eq!(observation["status"], "failed");
    assert!(
        observation["failure"]
            .as_str()
            .is_some_and(|failure| failure.contains("could not parse"))
    );
    assert_eq!(
        json(&invalid_root, &bin, &["--json", "drift", "--live"])["status"],
        "unknown"
    );
}

#[test]
fn newline_delimited_compose_output_is_parsed() {
    let root = temp_dir();
    compose_project(&root);
    let bin = fake_docker(
        &root,
        FakeDocker {
            compose_output: "{\"Service\":\"web\",\"State\":\"running\"}\n{\"Service\":\"db\",\"Status\":\"Up 3 minutes\"}\n",
            ..FakeDocker::default()
        },
    );
    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    let observations = reality["runtime"]["observations"].as_array().unwrap();
    assert_eq!(observations[0]["subject"], "./web");
    assert_eq!(observations[0]["status"], "present");
    assert_eq!(observations[1]["subject"], "./db");
    assert_eq!(observations[1]["status"], "present");
    assert_eq!(observations[1]["detail"], "running (Up 3 minutes)");
    assert_eq!(
        json(&root, &bin, &["--json", "drift", "--live"])["status"],
        "match"
    );
}

#[test]
fn dockerfile_only_scope_is_unsupported_not_observed() {
    let root = temp_dir();
    write(&root.join("Dockerfile"), "FROM alpine:3\n");
    let bin = fake_docker(&root, FakeDocker::default());

    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    let runtime = &reality["runtime"];
    assert_eq!(
        runtime["scope"],
        "container components without Compose files"
    );
    let observation = &runtime["observations"][0];
    assert_eq!(observation["subject"], ".");
    assert_eq!(observation["status"], "unsupported");
    assert!(
        observation["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Compose"))
    );
    assert_eq!(observation["evidence"], serde_json::json!(["Dockerfile"]));
    assert_eq!(invocations(&root), ["info --format {{.ServerVersion}}"]);
    assert_eq!(reality["outcome"], "partial");

    let drift = json(&root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(drift["status"], "unknown");
    assert_eq!(drift["outcome"], "partial");
    assert_eq!(drift["issues"][0]["status"], "unknown");
}

#[test]
fn live_on_a_project_without_container_components_reports_nothing_to_inspect() {
    let root = temp_dir();
    write(&root.join("package.json"), r#"{"name":"web"}"#);
    let bin = fake_docker(&root, FakeDocker::default());

    let reality = json(&root, &bin, &["--json", "reality", "--live"]);
    assert_eq!(reality["runtime"]["scope"], "no container components");
    assert!(
        reality["runtime"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(invocations(&root).is_empty());

    let rendered = text(&root, &bin, &["reality", "--live"]);
    assert!(rendered.contains("no container components to inspect"));

    let drift = json(&root, &bin, &["--json", "drift", "--live"]);
    assert_eq!(drift["status"], "match");
    assert_eq!(drift["runtime"]["scope"], "no container components");
    assert!(drift["issues"].as_array().unwrap().is_empty());
}

#[test]
fn live_is_rejected_for_commands_that_cannot_observe_runtime() {
    let root = temp_dir();
    write(&root.join("package.json"), r#"{"name":"web"}"#);
    let bin = fake_docker(&root, FakeDocker::default());

    for command in ["info", "graph", "doctor", "build"] {
        let output = pax(&root, &bin, &[command, "--live"]);
        assert_eq!(output.status.code(), Some(2), "{command}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("--live applies only to `pax reality` and `pax drift`"),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(invocations(&root).is_empty());
}
