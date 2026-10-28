//! Tests for the canonical PAX observation contract.
//!
//! These tests pin the schema naming, outcome vocabulary, evidence
//! traceability, project-relative identities, and the rule that an
//! unsupported observation is never reported as successful.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pax-contract-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn pax(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn json(root: &Path, args: &[&str]) -> Value {
    let output = pax(root, args);
    assert!(
        output.status.success(),
        "pax {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/contract")
}

fn fixtures() -> Vec<(String, Value)> {
    let mut fixtures = fs::read_dir(fixture_dir())
        .unwrap()
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let value = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            (name, value)
        })
        .collect::<Vec<_>>();
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!fixtures.is_empty());
    fixtures
}

fn allowed_outcomes() -> BTreeSet<&'static str> {
    ["ok", "partial", "unsupported", "blocked", "failed", "stale"]
        .into_iter()
        .collect()
}

fn allowed_states() -> BTreeSet<&'static str> {
    [
        "present",
        "absent",
        "unknown",
        "unsupported",
        "blocked",
        "failed",
        "stale",
    ]
    .into_iter()
    .collect()
}

fn assert_canonical_schema(document: &Value, name: &str) {
    let schema = document["schema"]
        .as_str()
        .unwrap_or_else(|| panic!("{name} has no schema"));
    let Some((_, version)) = schema.rsplit_once(".v") else {
        panic!("{name} schema {schema:?} is not versioned");
    };
    assert!(schema.starts_with("pax."), "{name}: {schema}");
    assert!(
        version.chars().all(|character| character.is_ascii_digit()),
        "{name}: {schema}"
    );
}

fn assert_project_relative(identity: &str, root: &Path, what: &str) {
    assert!(!identity.is_empty(), "{what} is empty");
    assert!(!identity.starts_with('/'), "{what} is absolute: {identity}");
    assert!(
        !identity.contains(root.to_str().unwrap()),
        "{what} contains the machine-specific root: {identity}"
    );
    assert!(
        !identity.split('/').any(|part| part == ".."),
        "{what} traverses upward: {identity}"
    );
}

#[test]
fn fixtures_use_canonical_schema_names_and_typed_outcomes() {
    let outcomes = allowed_outcomes();
    for (name, document) in fixtures() {
        assert_canonical_schema(&document, &name);

        if let Some(version) = document
            .get("schemaVersion")
            .or_else(|| document.get("schema_version"))
        {
            assert_eq!(version, "1", "{name} legacy version");
        } else if !matches!(
            document["schema"].as_str().unwrap(),
            "pax.contract.v1" | "pax.observation.v1" | "pax.execution-result.v1" | "pax.plan.v1"
        ) {
            panic!("{name} lost its legacy schema version");
        }

        if let Some(outcome) = document.get("outcome") {
            let outcome = outcome.as_str().unwrap();
            assert!(
                outcomes.contains(outcome),
                "{name} has an untyped outcome: {outcome}"
            );
            assert_ne!(outcome, "success", "{name}");
        }
    }

    let contract =
        serde_json::from_slice::<Value>(&fs::read(fixture_dir().join("contract.json")).unwrap())
            .unwrap();
    assert_eq!(contract["schema"], "pax.contract.v1");
    assert_eq!(
        contract["outcomes"],
        serde_json::json!(["ok", "partial", "unsupported", "blocked", "failed", "stale"])
    );
    assert_eq!(
        contract["fact_states"],
        serde_json::json!([
            "present",
            "absent",
            "unknown",
            "unsupported",
            "blocked",
            "failed",
            "stale"
        ])
    );
    assert_eq!(contract["identity"]["root"], ".");
    assert_eq!(contract["identity"]["separator"], "/");

    let partial = serde_json::from_slice::<Value>(
        &fs::read(fixture_dir().join("observation-partial.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(partial["outcome"], "partial");
    assert_ne!(partial["outcome"], "ok");
    let unsupported = serde_json::from_slice::<Value>(
        &fs::read(fixture_dir().join("observation-unsupported.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(unsupported["status"], "error");
    assert_eq!(unsupported["outcome"], "unsupported");
    assert_ne!(unsupported["outcome"], "ok");
}

#[test]
fn absent_unknown_unsupported_and_failed_are_distinct() {
    let js = temp_dir();
    write(
        &js.join("package.json"),
        r#"{"name":"web","packageManager":"npm@10.9.0"}"#,
    );
    write(&js.join("package-lock.json"), "{}");
    let reality = json(&js, &["--json", "reality"]);
    assert_eq!(reality["installed"]["observations"][0]["status"], "absent");
    let javascript_reality = reality;

    let python = temp_dir();
    write(
        &python.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    );
    let python_reality = json(&python, &["--json", "reality"]);
    assert_eq!(
        python_reality["installed"]["observations"][0]["status"],
        "unknown"
    );

    let scripts = json(&python, &["--json", "scripts"]);
    assert_eq!(scripts["outcome"], "unsupported");

    let failure = pax(&python, &["--json", "observe", "--scope", "nonsense"]);
    assert_eq!(failure.status.code(), Some(2));
    let failure: Value = serde_json::from_slice(&failure.stderr).unwrap();
    assert_eq!(failure["status"], "error");
    assert_eq!(failure["outcome"], "failed");
    assert_eq!(failure["code"], "invalid_scope");

    let observed = [
        javascript_reality["installed"]["observations"][0]["status"].as_str(),
        python_reality["installed"]["observations"][0]["status"].as_str(),
        Some(scripts["outcome"].as_str().unwrap()),
        failure["outcome"].as_str(),
    ]
    .into_iter()
    .map(|state| state.unwrap().to_string())
    .collect::<BTreeSet<_>>();
    for distinct in ["absent", "unknown", "unsupported", "failed"] {
        assert!(observed.contains(distinct), "{distinct} missing");
    }
}

#[test]
fn unsupported_observation_is_never_reported_as_success() {
    let empty = temp_dir();
    for command in [
        "info",
        "deps",
        "scripts",
        "workspaces",
        "lock",
        "graph",
        "reality",
        "drift",
    ] {
        let document = json(&empty, &["--json", command]);
        assert_eq!(document["outcome"], "unsupported", "{command}");
        assert_ne!(document["outcome"], "ok", "{command}");
        if command == "drift" {
            assert_eq!(document["status"], "unsupported");
        }
    }

    let observation = json(&empty, &["--json", "observe"]);
    assert_eq!(observation["schema"], "pax.observation.v1");
    assert_eq!(observation["outcome"], "unsupported");
    assert_ne!(observation["outcome"], "ok");
    assert_ne!(observation["status"], "ok");

    let python = temp_dir();
    write(
        &python.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    );
    let scripts = json(&python, &["--json", "scripts"]);
    assert_eq!(scripts["outcome"], "unsupported");
    assert!(scripts["scripts"].as_object().unwrap().is_empty());
}

#[test]
fn every_reported_fact_is_traceable_to_a_source() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","dependencies":{"react":"^19"},"scripts":{"build":"tsc"}}"#,
    );
    write(&root.join("package-lock.json"), "{}");

    let graph = json(&root, &["--json", "graph"]);
    let evidence = graph["evidence"].as_array().unwrap();
    for edge in graph["edges"].as_array().unwrap() {
        let key = format!(
            "{} -> {}",
            edge["from"].as_str().unwrap(),
            edge["to"].as_str().unwrap()
        );
        let source = evidence
            .iter()
            .find(|item| item["edge"] == key)
            .unwrap_or_else(|| panic!("edge {key} has no evidence"));
        let sources = source["evidence"].as_array().unwrap();
        assert!(!sources.is_empty(), "edge {key} has empty evidence");
        for source in sources {
            assert_project_relative(source.as_str().unwrap(), &root, &key);
        }
    }

    let info = json(&root, &["--json", "info"]);
    for component in info["components"].as_array().unwrap() {
        assert_project_relative(component["path"].as_str().unwrap(), &root, "component path");
        assert!(!component["evidence"].as_array().unwrap().is_empty());
    }
}

#[test]
fn graph_identities_are_project_relative_and_never_absolute() {
    let root = temp_dir();
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
    );
    for name in ["a", "b"] {
        write(
            &root.join(format!("crates/{name}/Cargo.toml")),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write(&root.join(format!("crates/{name}/src/lib.rs")), "");
    }

    let graph = json(&root, &["--json", "graph"]);
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "." && node["kind"] == "component"),
        "the project root is not identified as `.`"
    );
    for node in graph["nodes"].as_array().unwrap() {
        assert_project_relative(node["id"].as_str().unwrap(), &root, "node id");
    }
    for edge in graph["edges"].as_array().unwrap() {
        assert_project_relative(edge["from"].as_str().unwrap(), &root, "edge from");
        assert_project_relative(edge["to"].as_str().unwrap(), &root, "edge to");
    }
    for source in graph["evidence"].as_array().unwrap() {
        assert_project_relative(source["edge"].as_str().unwrap(), &root, "evidence edge");
        for path in source["evidence"].as_array().unwrap() {
            assert_project_relative(path.as_str().unwrap(), &root, "evidence source");
        }
    }
    assert!(
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| { edge["kind"] == "workspace-member" && edge["from"] == "." })
    );
}

#[test]
fn single_package_cargo_is_not_reported_as_a_workspace() {
    let root = temp_dir();
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"alone\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&root.join("src/lib.rs"), "");

    let workspaces = json(&root, &["--json", "workspaces"]);
    assert_eq!(workspaces["workspaces"]["enabled"], false);
    assert_eq!(workspaces["workspaces"]["source"], Value::Null);
    assert!(
        workspaces["workspaces"]["packages"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let graph = json(&root, &["--json", "graph"]);
    assert!(
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge["kind"] != "workspace-member")
    );
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["kind"] != "workspace-member")
    );
}

#[test]
fn dry_run_plan_declares_its_schema_without_claiming_an_outcome() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"npm@10.9.0","scripts":{"test":"true"}}"#,
    );
    write(&root.join("package-lock.json"), "{}");

    let plan = json(&root, &["--json", "--dry-run", "test"]);
    assert_eq!(plan["schema"], "pax.plan.v1");
    assert!(plan.get("outcome").is_none());
    assert!(plan.get("status").is_none());
    assert_eq!(plan["operation"], "test");
    assert_eq!(plan["supported"], true);
}

#[test]
fn execution_result_keeps_its_own_typed_status() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"npm@10.9.0","scripts":{"test":"true"}}"#,
    );
    write(&root.join("package-lock.json"), "{}");

    let output = pax(&root, &["--json", "test", "--tool", "nonsense"]);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["schema"], "pax.execution-result.v1");
    assert_eq!(result["status"], "unsupported");
    assert_eq!(result["reason"], "operation-unsupported");
    assert!(result.get("outcome").is_none());
}

#[test]
fn a_script_named_drift_does_not_select_drift_exit_semantics() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"npm@10.9.0","scripts":{"drift":"true"}}"#,
    );
    write(&root.join("package-lock.json"), "{}");

    let bin = root.join("fake-bin");
    let invoked = root.join("invoked.log");
    write(
        &bin.join("npm"),
        &format!(
            "#!/bin/sh\necho \"npm $@\" >> \"{}\"\nexit 0\n",
            invoked.display()
        ),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin.join("npm"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["run", "drift"])
        .current_dir(&root)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pax run drift exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(&invoked).unwrap();
    assert!(log.contains("npm run drift"), "{log}");
}

#[test]
fn inspection_outcomes_are_never_the_untyped_word_success() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","packageManager":"npm@10.9.0"}"#,
    );
    write(&root.join("package-lock.json"), "{}");
    fs::create_dir_all(root.join("node_modules")).unwrap();
    for command in [
        "info",
        "deps",
        "scripts",
        "workspaces",
        "lock",
        "graph",
        "reality",
        "drift",
    ] {
        let document = json(&root, &["--json", command]);
        let outcome = document["outcome"].as_str().unwrap();
        assert!(allowed_outcomes().contains(outcome), "{command}");
        assert_ne!(outcome, "success", "{command}");
        assert_canonical_schema(&document, command);
    }

    let states = allowed_states();
    assert!(states.contains("absent"));
    assert!(states.contains("unknown"));
    assert!(states.contains("unsupported"));
    assert!(states.contains("failed"));
}
