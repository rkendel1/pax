//! Boundary tests: what PAX observes, what it does not do, and what consumers
//! (Compute's adapter, Chip's result parser) may rely on. See docs/PAX_BOUNDARY.md.
//!
//! Tests prefixed `known_limitation_` pin current behavior that docs/PAX_AUDIT.md
//! records as an overclaim or gap. They are tripwires, not endorsements: if one
//! fails because PAX improved, update the audit and delete the test.

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pax-boundary-{}-{}",
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

fn js_project(root: &Path) {
    write(
        &root.join("package.json"),
        r#"{"name":"web","scripts":{"start":"node index.js"},"dependencies":{"left-pad":"^1.0.0"},"engines":{"node":">=24"}}"#,
    );
    write(
        &root.join("package-lock.json"),
        r#"{"packages":{"node_modules/left-pad":{}}}"#,
    );
}

fn rust_project(root: &Path) {
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(&root.join("src/lib.rs"), "");
}

fn python_project(root: &Path) {
    write(&root.join("uv.lock"), "");
    write(
        &root.join("pyproject.toml"),
        "[project]\nname = \"svc\"\nversion = \"1.0\"\ndependencies = [\"requests>=2\"]\n",
    );
}

fn container_project(root: &Path) {
    write(&root.join("Dockerfile"), "FROM alpine:3\nCMD [\"true\"]\n");
}

// 1. PAX can inspect supported project types.
#[test]
fn inspects_each_supported_project_type() {
    for (setup, ecosystem, tool) in [
        (js_project as fn(&Path), "javascript", "npm"),
        (rust_project, "rust", "cargo"),
        (python_project, "python", "uv"),
        (container_project, "container", "docker"),
    ] {
        let root = temp_dir();
        setup(&root);
        let info = json(&root, &["--json", "info"]);
        assert_eq!(info["ecosystem"], ecosystem);
        let component = &info["components"][0];
        assert_eq!(component["ecosystem"], ecosystem);
        assert_eq!(component["tool"], tool);
        // every component names the files it was detected from
        assert!(!component["evidence"].as_array().unwrap().is_empty());
    }
}

// 2 + 5. Deterministic, explicitly versioned output.
#[test]
fn structured_output_is_versioned_and_deterministic() {
    let root = temp_dir();
    js_project(&root);
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
        let first = json(&root, &["--json", command]);
        let second = json(&root, &["--json", command]);
        assert_eq!(first, second, "{command} is not deterministic");
        // The version key is `schemaVersion` for inspection documents and
        // `schema_version` for graph/reality/drift (audit: inconsistent casing).
        let version = first
            .get("schemaVersion")
            .or_else(|| first.get("schema_version"))
            .unwrap_or_else(|| panic!("{command} has no schema version"));
        assert_eq!(version, "1");
        assert_eq!(first["command"], command);
        // No clock-derived fields exist anywhere in observation output.
        let text = first.to_string();
        for forbidden in ["timestamp", "observedAt", "observed_at"] {
            assert!(!text.contains(forbidden), "{command} contains {forbidden}");
        }
    }
}

// 3 + 9. Unknown is distinct from known; declared is distinct from observed.
#[test]
fn reality_separates_layers_and_keeps_unknown_distinct_from_absent() {
    let root = temp_dir();
    js_project(&root); // declared + resolved, but no node_modules
    let reality = json(&root, &["--json", "reality"]);
    assert_eq!(reality["declared"]["observations"][0]["status"], "present");
    assert_eq!(reality["resolved"]["observations"][0]["status"], "present");
    assert_eq!(reality["installed"]["observations"][0]["status"], "absent");
    assert_eq!(reality["live"], false);
    assert!(
        reality["runtime"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Python without a conventional environment directory is `unknown`, not `absent`.
    let py = temp_dir();
    python_project(&py);
    let reality = json(&py, &["--json", "reality"]);
    assert_eq!(reality["installed"]["observations"][0]["status"], "unknown");

    // Drift evidence labels which layer each source belongs to.
    let drift = json(&root, &["--json", "drift"]);
    let kinds: Vec<&str> = drift["issues"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|issue| issue["evidence"].as_array().unwrap())
        .map(|evidence| evidence["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"declared") && kinds.contains(&"installed"));
}

fn fake_tools(dir: &Path, names: &[&str]) -> PathBuf {
    let bin = dir.join("fake-bin");
    for name in names {
        let path = bin.join(name);
        write(
            &path,
            &format!(
                "#!/bin/sh\necho \"{name} $@\" >> \"{}\"\nexit 0\n",
                dir.join("invoked.log").display()
            ),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    bin
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, path: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap().flatten() {
            let path = entry.path();
            let key = path.strip_prefix(base).unwrap().display().to_string();
            if path.is_dir() {
                out.insert(format!("{key}/"), Vec::new());
                walk(base, &path, out);
            } else {
                out.insert(key, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

// 7 + 8. Observation commands never execute tools, resolve dependencies, or write files,
// including `--live` (which today runs nothing).
#[cfg(unix)]
#[test]
fn observation_commands_execute_no_tool_and_write_nothing() {
    let sandbox = temp_dir();
    let tools = fake_tools(
        &sandbox,
        &[
            "npm", "pnpm", "yarn", "bun", "node", "uv", "pip", "python", "python3", "poetry",
            "pdm", "docker",
        ],
    );
    let project = sandbox.join("project");
    js_project(&project);
    python_project(&project.join("svc"));
    container_project(&project.join("box"));
    let before = snapshot(&project);
    let path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
    for args in [
        &["--json", "info"][..],
        &["--json", "doctor"],
        &["--json", "deps"],
        &["--json", "scripts"],
        &["--json", "workspaces"],
        &["--json", "lock"],
        &["--json", "graph"],
        &["--json", "observe"],
        &["--json", "reality"],
        &["--json", "reality", "--live"],
        &["--json", "drift", "--live"],
        &["--dry-run", "--json", "install"],
        &["--dry-run", "--json", "test"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pax"))
            .args(args)
            .current_dir(&project)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(
            output.status.code().is_some_and(|code| code <= 2),
            "{args:?}"
        );
    }
    assert!(
        !sandbox.join("invoked.log").exists(),
        "a tool was executed: {}",
        fs::read_to_string(sandbox.join("invoked.log")).unwrap_or_default()
    );
    assert_eq!(
        snapshot(&project),
        before,
        "observation modified the project"
    );
}

// `--live` claims a runtime layer but performs no runtime observation.
#[test]
fn known_limitation_live_names_a_command_it_never_runs() {
    let root = temp_dir();
    container_project(&root);
    let reality = json(&root, &["--json", "reality", "--live"]);
    let runtime = &reality["runtime"]["observations"][0];
    assert_eq!(runtime["status"], "unknown");
    assert_eq!(runtime["evidence"][0], "docker compose ps");
}

// 10. Invalid metadata produces an explicit error, not invented observations.
#[test]
fn invalid_package_json_is_an_explicit_error() {
    let root = temp_dir();
    write(&root.join("package.json"), "{ not json");
    let output = pax(&root, &["--json", "info"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to parse"));
}

// 4 + 10 (negative): malformed non-JSON manifests currently produce observations
// and no diagnostic. Recorded in the audit as a gap.
#[test]
fn known_limitation_malformed_manifests_emit_no_diagnostic() {
    let root = temp_dir();
    write(&root.join("pyproject.toml"), "[project\nname = ");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["ecosystem"], "python");
    let diagnostics = info["diagnostics"].to_string();
    assert!(!diagnostics.contains("pyproject"), "{diagnostics}");

    let cargo = temp_dir();
    write(&cargo.join("Cargo.toml"), "[package\nname=");
    let info = json(&cargo, &["--json", "info"]);
    // Provenance is retained: the source names the fallback parser.
    assert_eq!(info["cargo"]["source"], "Cargo.toml fallback");
}

// Declared requirements that PAX does not read are not reported at all.
#[test]
fn known_limitation_runtime_requirements_are_not_observed_or_reported() {
    let root = temp_dir();
    js_project(&root); // declares engines.node ">=24"
    for command in ["info", "doctor", "deps", "reality", "drift", "graph"] {
        let output = pax(&root, &["--json", command]);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            !text.contains(">=24"),
            "{command} unexpectedly reports engines.node"
        );
    }
    let doctor = json(&root, &["--json", "doctor"]);
    assert!(doctor["result"]["runtime"].is_null());
}

// Parsers that are heuristic, not ecosystem-authoritative (audit table).
#[test]
fn known_limitation_pyproject_metadata_is_reported_as_dependencies() {
    let root = temp_dir();
    python_project(&root);
    let deps = json(&root, &["--json", "deps"]);
    let names: Vec<&str> = deps["dependencies"]["nativeDependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dependency| dependency["name"].as_str().unwrap())
        .collect();
    // `name` and `version` are project metadata; `requests` is never extracted.
    assert!(
        names.contains(&"name") && names.contains(&"version"),
        "{names:?}"
    );
    assert!(!names.contains(&"requests"), "{names:?}");
}

#[test]
fn known_limitation_requirements_txt_alone_yields_no_dependencies() {
    let root = temp_dir();
    write(&root.join("requirements.txt"), "requests>=2\n");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["ecosystem"], "python");
    assert!(info["nativeDependencies"].as_array().unwrap().is_empty());
}

#[test]
fn known_limitation_compose_keys_are_reported_as_services() {
    let root = temp_dir();
    write(
        &root.join("compose.yaml"),
        "services:\n  web:\n    image: nginx\n    ports:\n      - \"80:80\"\n    build:\n      context: .\n",
    );
    let info = json(&root, &["--json", "info"]);
    let services = info["container"]["services"].to_string();
    assert!(
        services.contains("web") && services.contains("ports"),
        "{services}"
    );
}

#[test]
fn known_limitation_empty_node_modules_counts_as_installed() {
    let root = temp_dir();
    js_project(&root);
    fs::create_dir(root.join("node_modules")).unwrap();
    let reality = json(&root, &["--json", "reality"]);
    assert_eq!(reality["installed"]["observations"][0]["status"], "present");
}

// 6. The fields Compute's adapter reads (compute-project/src/pax.rs) exist and keep
// their shape. Compute reads `info`, `deps`, and `scripts`, nothing else.
#[test]
fn consumer_contract_for_compute_adapter() {
    let root = temp_dir();
    js_project(&root);
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["schemaVersion"], "1");
    assert_eq!(info["command"], "info");
    assert!(info["project"]["name"].is_string());
    assert!(info["ecosystem"].is_string());
    assert!(info["manager"]["name"].is_string());
    assert!(info["components"][0]["ecosystem"].is_string());
    assert!(info["components"][0]["tool"].is_string());

    let deps = json(&root, &["--json", "deps"]);
    assert_eq!(deps["schemaVersion"], "1");
    for group in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        assert!(deps["dependencies"][group].is_object(), "{group}");
    }
    assert!(deps["dependencies"]["nativeDependencies"].is_array());
    assert_eq!(deps["dependencies"]["dependencies"]["left-pad"], "^1.0.0");

    let scripts = json(&root, &["--json", "scripts"]);
    assert_eq!(scripts["schemaVersion"], "1");
    assert_eq!(scripts["scripts"]["start"], "node index.js");

    // A mixed-ecosystem repository reports no single ecosystem; a consumer must
    // not guess one.
    let mixed = temp_dir();
    js_project(&mixed);
    container_project(&mixed);
    let info = json(&mixed, &["--json", "info"]);
    assert!(info["ecosystem"].is_null());
    assert_eq!(info["components"].as_array().unwrap().len(), 2);
}

// Machine-specific data is present in observation output (audit: not a stable identity).
#[test]
fn known_limitation_info_embeds_the_absolute_project_root() {
    let root = temp_dir();
    js_project(&root);
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["project"]["root"], root.display().to_string());
}

// `unknown` is never promoted to `drift`, but it is promoted to a top-level `ambiguous`
// (and exit code 2) even though nothing is ambiguous.
#[test]
fn known_limitation_live_unknown_surfaces_as_ambiguous() {
    let root = temp_dir();
    container_project(&root);
    let static_drift = json(&root, &["--json", "drift"]);
    assert_eq!(static_drift["status"], "match");
    let output = pax(&root, &["--json", "drift", "--live"]);
    assert_eq!(output.status.code(), Some(2));
    let live: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(live["status"], "ambiguous");
    assert_eq!(live["issues"][0]["status"], "unknown");
}
