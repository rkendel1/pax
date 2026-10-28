//! Project discovery and declaration tests.
//!
//! These tests use real fixture projects (including malformed and ambiguous
//! ones) to pin what PAX discovers for Python, Rust, JavaScript, and Compose
//! projects, and where every declaration came from.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pax-discovery-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/discovery")
        .join(name)
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap().flatten() {
        let path = entry.path();
        let target = destination.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

fn project(name: &str) -> PathBuf {
    let root = temp_dir();
    copy_dir(&fixture(name), &root);
    root
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

fn native_dependencies(document: &Value) -> &[Value] {
    document["dependencies"]["nativeDependencies"]
        .as_array()
        .unwrap_or_else(|| panic!("no dependencies.nativeDependencies in {document}"))
        .as_slice()
}

fn dependency<'a>(dependencies: &'a [Value], name: &str, source: &str) -> &'a Value {
    dependencies
        .iter()
        .find(|dependency| dependency["name"] == name && dependency["source"] == source)
        .unwrap_or_else(|| panic!("missing dependency {name} from {source} in {dependencies:?}"))
}

fn component<'a>(document: &'a Value, path: &str) -> &'a Value {
    document["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["path"] == path)
        .unwrap_or_else(|| panic!("missing component {path} in {document}"))
}

fn diagnostics(document: &Value) -> String {
    document["diagnostics"].to_string()
}

#[test]
fn python_pyproject_declarations_are_read_with_provenance() {
    let root = project("python-pyproject");
    let deps = json(&root, &["--json", "deps"]);
    assert_eq!(deps["outcome"], "ok");
    let dependencies = native_dependencies(&deps);

    let requests = dependency(dependencies, "requests", "pyproject.toml");
    assert_eq!(requests["specifier"], ">=2.31");
    assert_eq!(requests["kind"], "dependency");
    assert_eq!(requests["ecosystem"], "python");

    // Extras are part of the requirement, not the package name.
    let httpx = dependency(dependencies, "httpx", "pyproject.toml");
    assert_eq!(httpx["specifier"], "==0.27.0");

    let pytest = dependency(dependencies, "pytest", "pyproject.toml");
    assert_eq!(pytest["kind"], "development");
    assert_eq!(pytest["nativeKind"], "optional-dependency");

    let ruff = dependency(dependencies, "ruff", "pyproject.toml");
    assert_eq!(ruff["kind"], "development");
    assert_eq!(ruff["nativeKind"], "dependency-group");

    let hatchling = dependency(dependencies, "hatchling", "pyproject.toml");
    assert_eq!(hatchling["kind"], "build");
    assert_eq!(hatchling["nativeKind"], "build-requirement");

    // Project metadata is not a dependency.
    assert!(
        !dependencies
            .iter()
            .any(|dependency| dependency["name"] == "name" || dependency["name"] == "version"),
        "{dependencies:?}"
    );

    let lock = json(&root, &["--json", "lock"]);
    assert_eq!(lock["lock"]["found"], true);
    assert_eq!(lock["lock"]["selected"], "uv.lock");
    assert_eq!(lock["outcome"], "ok");
}

#[test]
fn standalone_and_nested_requirements_are_discovered_independently() {
    let root = project("python-requirements");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["outcome"], "ok");
    let dependencies = info["nativeDependencies"].as_array().unwrap();

    assert_eq!(
        dependency(dependencies, "flask", "requirements.txt")["specifier"],
        ">=3.0"
    );
    assert_eq!(
        dependency(dependencies, "requests", "requirements.txt")["specifier"],
        ">=2"
    );
    dependency(dependencies, "pytest", "requirements-dev.txt");
    dependency(dependencies, "black", "requirements-dev.txt");
    dependency(dependencies, "attrs", "requirements/base.txt");

    // A pip option is not a declaration, and a URL is reported as uninterpreted.
    assert!(
        !dependencies
            .iter()
            .any(|dependency| dependency["name"] == "requirements-dev.txt"),
        "{dependencies:?}"
    );
    let diagnostics = diagnostics(&info);
    assert!(diagnostics.contains("URL requirement"), "{diagnostics}");
    assert!(!dependencies.iter().any(|dependency| {
        dependency["name"]
            .as_str()
            .is_some_and(|name| name.contains("git+"))
    }));
}

#[test]
fn pipfile_declarations_are_visible_and_pipenv_is_explicitly_unsupported() {
    let root = project("python-pipfile");
    let info = json(&root, &["--json", "info"]);
    let python = component(&info, ".");
    assert_eq!(python["ecosystem"], "python");
    assert_eq!(python["tool"], "pipenv");
    assert_eq!(python["lockfiles"], serde_json::json!(["Pipfile.lock"]));

    let dependencies = info["nativeDependencies"].as_array().unwrap();
    assert_eq!(
        dependency(dependencies, "requests", "Pipfile")["specifier"],
        "*"
    );
    assert_eq!(
        dependency(dependencies, "pytest", "Pipfile")["kind"],
        "development"
    );

    let output = pax(&root, &["install"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pipenv is not supported"), "{stderr}");
}

#[test]
fn ambiguous_python_manifests_are_reported_and_one_wins() {
    let root = project("python-ambiguous");
    let info = json(&root, &["--json", "info"]);
    let text = diagnostics(&info);
    assert!(
        text.contains("declares both pyproject.toml and Pipfile"),
        "{text}"
    );

    let dependencies = info["nativeDependencies"].as_array().unwrap();
    // pyproject.toml is interpreted; Pipfile declarations are not duplicated.
    dependency(dependencies, "requests", "pyproject.toml");
    assert!(
        !dependencies
            .iter()
            .any(|dependency| dependency["source"] == "Pipfile"),
        "{dependencies:?}"
    );
}

#[test]
fn malformed_python_manifest_is_an_error_not_an_empty_success() {
    let root = project("python-malformed");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["outcome"], "partial");
    let text = diagnostics(&info);
    assert!(text.contains("failed to parse pyproject.toml"), "{text}");
    assert!(text.contains("\"level\":\"error\""), "{text}");
    // The manifest is still reported, but no dependencies were invented.
    assert_eq!(
        component(&info, ".")["manifests"],
        serde_json::json!(["pyproject.toml"])
    );
    assert_eq!(info["nativeDependencies"], serde_json::json!([]));
}

#[test]
fn malformed_cargo_manifest_is_an_error_and_keeps_fallback_provenance() {
    let root = project("cargo-malformed");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["outcome"], "partial");
    let text = diagnostics(&info);
    assert!(text.contains("failed to parse Cargo.toml"), "{text}");
    assert_eq!(info["cargo"]["source"], "Cargo.toml fallback");
}

#[test]
fn cargo_workspace_members_with_trailing_comments_are_detected() {
    let root = project("cargo-workspace-comment");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(info["cargo"]["workspace"], true);
    assert_eq!(info["cargo"]["workspaceRoot"], ".");
    assert_eq!(info["project"]["workspace"], true);

    let workspaces = json(&root, &["--json", "workspaces"]);
    let packages = workspaces["workspaces"]["packages"].as_array().unwrap();
    for expected in ["app", "lib"] {
        assert!(
            packages.iter().any(|package| package == expected),
            "missing {expected} in {packages:?}"
        );
    }
    for package in packages {
        assert!(
            !package.as_str().unwrap().starts_with('/'),
            "absolute workspace identity: {package}"
        );
    }

    let graph = json(&root, &["--json", "graph"]);
    let members = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "workspace-member")
        .map(|node| node["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(members.contains(&"app"), "{members:?}");
    assert!(members.contains(&"lib"), "{members:?}");
}

#[test]
fn nested_javascript_manifests_are_discovered_with_provenance() {
    let root = project("javascript-nested");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(component(&info, ".")["ecosystem"], "javascript");
    let nested = component(&info, "apps/web");
    assert_eq!(nested["ecosystem"], "javascript");
    assert_eq!(nested["manifests"], serde_json::json!(["package.json"]));
    assert!(
        diagnostics(&info).contains("apps/web/package.json"),
        "{}",
        diagnostics(&info)
    );

    let deps = json(&root, &["--json", "deps"]);
    // Root JavaScript declarations stay in the grouped root fields.
    assert_eq!(deps["dependencies"]["dependencies"]["typescript"], "^5.4.0");
    let dependencies = native_dependencies(&deps);
    assert_eq!(
        dependency(dependencies, "react", "apps/web/package.json")["kind"],
        "dependency"
    );
    assert_eq!(
        dependency(dependencies, "vite", "apps/web/package.json")["kind"],
        "development"
    );
    assert!(
        !dependencies
            .iter()
            .any(|dependency| dependency["name"] == "typescript"),
        "root declarations must not be duplicated as native dependencies: {dependencies:?}"
    );

    let graph = json(&root, &["--json", "graph"]);
    let edges = graph["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|edge| { edge["from"] == "apps/web" && edge["to"] == "react" })
    );
    assert!(
        !edges
            .iter()
            .any(|edge| { edge["from"] == "." && edge["to"] == "react" })
    );

    let graph_evidence = graph["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|evidence| evidence["edge"] == "apps/web -> react")
        .unwrap();
    assert_eq!(
        graph_evidence["evidence"],
        serde_json::json!(["apps/web/package.json"])
    );
}

#[test]
fn hoisted_node_modules_counts_as_installed_for_nested_packages() {
    let root = project("javascript-nested");
    let reality = json(&root, &["--json", "reality"]);
    let observations = reality["installed"]["observations"].as_array().unwrap();
    let web = observations
        .iter()
        .find(|observation| observation["subject"] == "apps/web")
        .unwrap();
    assert_eq!(web["status"], "present");
    assert_eq!(web["evidence"], serde_json::json!(["node_modules/"]));

    let drift = json(&root, &["--json", "drift"]);
    assert!(
        !drift["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["subject"] == "apps/web"),
        "{drift}"
    );
}

#[test]
fn compose_services_are_limited_to_direct_children_of_the_services_key() {
    let root = project("compose-services");
    let info = json(&root, &["--json", "info"]);
    assert_eq!(
        info["container"]["services"],
        serde_json::json!(["api", "db"])
    );
    assert_eq!(
        info["container"]["images"],
        serde_json::json!(["example/api:1", "postgres:16"])
    );
    assert_eq!(
        info["container"]["directives"]["WORKDIR"],
        serde_json::json!(["/app"])
    );
}

#[test]
fn declaration_and_component_identities_are_project_relative() {
    for name in [
        "python-pyproject",
        "python-requirements",
        "python-pipfile",
        "python-ambiguous",
        "cargo-workspace-comment",
        "javascript-nested",
        "compose-services",
    ] {
        let root = project(name);
        let info = json(&root, &["--json", "info"]);
        let root_text = root.to_str().unwrap();
        for dependency in info["nativeDependencies"].as_array().unwrap() {
            let source = dependency["source"].as_str().unwrap();
            assert!(!source.starts_with('/'), "{name}: {source}");
            assert!(!source.contains(root_text), "{name}: {source}");
        }
        for component in info["components"].as_array().unwrap() {
            let path = component["path"].as_str().unwrap();
            assert!(!path.starts_with('/'), "{name}: {path}");
            assert!(!path.contains(root_text), "{name}: {path}");
        }
    }
}
