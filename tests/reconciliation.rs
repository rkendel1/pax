//! Dependency-evidence reconciliation tests.
//!
//! These tests pin the rule that declared dependencies, lockfile-recorded
//! dependencies, and installed packages are separate facts, and that unknown
//! evidence is never promoted to a confirmed discrepancy.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pax-reconciliation-{}-{}",
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
        output.status.code().is_some_and(|code| code <= 1),
        "pax {args:?} exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "pax {args:?} did not emit one JSON document: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn npm_project(root: &Path, dependencies: &[&str], lock_packages: &[&str]) {
    let dependencies = dependencies
        .iter()
        .map(|name| format!("\"{name}\":\"^1.0.0\""))
        .collect::<Vec<_>>()
        .join(",");
    write(
        &root.join("package.json"),
        &format!(r#"{{"name":"web","dependencies":{{{dependencies}}}}}"#),
    );
    let packages = lock_packages
        .iter()
        .map(|name| format!(r#""node_modules/{name}":{{"version":"1.0.0"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    write(
        &root.join("package-lock.json"),
        &format!(
            r#"{{"name":"web","lockfileVersion":3,"packages":{{"":{{"name":"web"}},{packages}}}}}"#
        ),
    );
}

fn node_module(root: &Path, name: &str) {
    fs::create_dir_all(root.join("node_modules").join(name)).unwrap();
}

#[test]
fn multiple_lockfiles_are_ambiguous_not_a_confirmed_discrepancy() {
    let root = temp_dir();
    write(&root.join("package.json"), r#"{"name":"web"}"#);
    write(&root.join("package-lock.json"), "{}");
    write(&root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n");

    let output = pax(&root, &["--json", "drift"]);
    assert_eq!(output.status.code(), Some(2));
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["status"], "ambiguous");
    let issues = drift["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1, "{drift}");
    assert_eq!(issues[0]["status"], "ambiguous");

    let text = pax(&root, &["drift"]);
    assert_eq!(text.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&text.stdout).starts_with("AMBIGUOUS"));
}

#[test]
fn reality_reports_each_dependency_layer_with_its_own_source() {
    let root = temp_dir();
    npm_project(&root, &["left-pad"], &["left-pad"]);
    node_module(&root, "left-pad");

    let reality = json(&root, &["--json", "reality"]);
    assert_eq!(reality["outcome"], "ok");
    let resolved = &reality["resolved"]["observations"][0];
    assert_eq!(resolved["status"], "present");
    assert_eq!(resolved["format"], "npm");
    assert_eq!(resolved["packageCount"], 1);
    let installed = &reality["installed"]["observations"][0];
    assert_eq!(installed["status"], "present");
    assert_eq!(installed["packageCount"], 1);

    let dependency = &reality["dependencies"][0];
    assert_eq!(dependency["name"], "left-pad");
    assert_eq!(dependency["declared"]["status"], "present");
    assert_eq!(dependency["declared"]["source"], "package.json");
    assert_eq!(dependency["lockfile"]["status"], "present");
    assert_eq!(dependency["lockfile"]["source"], "package-lock.json");
    assert_eq!(dependency["installed"]["status"], "present");
    assert_eq!(dependency["installed"]["source"], "node_modules/left-pad");
    assert!(dependency.get("runtime").is_none());

    // Runtime dependency observation is not available statically and is not
    // invented by `reality`.
    assert!(
        reality["runtime"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_lockfile_is_unknown_and_never_a_confirmed_discrepancy() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","dependencies":{"left-pad":"^1.0.0"}}"#,
    );
    write(&root.join("package-lock.json"), "{}");
    node_module(&root, "left-pad");

    let output = pax(&root, &["--json", "drift"]);
    assert_eq!(output.status.code(), Some(0));
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["status"], "unknown");
    assert_eq!(drift["outcome"], "partial");
    assert!(
        drift["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|issue| issue["status"] != "drift"),
        "{drift}"
    );
    let issue = drift["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["expected"] == "lockfile-recorded dependencies")
        .expect("unknown lockfile issue");
    assert_eq!(issue["status"], "unknown");
    assert_eq!(issue["evidence"][0]["kind"], "resolved");

    let text = pax(&root, &["drift"]);
    assert_eq!(text.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("UNKNOWN"), "{text}");
    assert!(!text.contains("NO DRIFT"), "{text}");
}

#[test]
fn unparseable_lockfile_is_unknown_not_drift() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","dependencies":{"left-pad":"^1.0.0"}}"#,
    );
    write(&root.join("package-lock.json"), "{ not json");
    node_module(&root, "left-pad");

    let drift = json(&root, &["--json", "drift"]);
    assert_eq!(drift["status"], "unknown");
    assert_eq!(drift["outcome"], "partial");
    assert!(
        drift["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["status"] == "unknown")
    );
}

#[test]
fn empty_but_interpretable_lockfile_reports_the_missing_dependency_as_drift() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"web","dependencies":{"left-pad":"^1.0.0"}}"#,
    );
    write(
        &root.join("package-lock.json"),
        r#"{"name":"web","lockfileVersion":3,"packages":{"":{"name":"web"}}}"#,
    );
    node_module(&root, "left-pad");

    let output = pax(&root, &["--json", "drift"]);
    assert_eq!(output.status.code(), Some(1));
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["status"], "drift");
    let issue = drift["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["subject"] == "./left-pad")
        .expect("missing lockfile dependency");
    assert_eq!(issue["status"], "drift");
    assert_eq!(
        issue["expected"],
        "declared dependency recorded in resolvable lockfile"
    );
    assert_eq!(issue["actual"], "left-pad missing from package-lock.json");
}

#[test]
fn installed_package_missing_is_drift_even_when_the_lockfile_records_it() {
    let root = temp_dir();
    npm_project(&root, &["left-pad"], &["left-pad"]);
    node_module(&root, "other");

    let output = pax(&root, &["--json", "drift"]);
    assert_eq!(output.status.code(), Some(1));
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["status"], "drift");
    let issue = drift["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["subject"] == "./left-pad")
        .expect("missing installed package");
    assert_eq!(issue["status"], "drift");
    assert_eq!(issue["expected"], "declared dependency installed");
    assert_eq!(issue["actual"], "left-pad missing from installed packages");
    assert_eq!(issue["evidence"][1]["kind"], "installed");
}

#[test]
fn nested_components_reconcile_against_the_workspace_lockfile_and_hoisted_install() {
    let root = temp_dir();
    write(
        &root.join("package.json"),
        r#"{"name":"root","private":true,"workspaces":["apps/*"]}"#,
    );
    write(
        &root.join("package-lock.json"),
        r#"{"name":"root","lockfileVersion":3,"packages":{"":{"name":"root"},"node_modules/react":{"version":"18.3.1"}}}"#,
    );
    write(
        &root.join("apps/web/package.json"),
        r#"{"name":"web","dependencies":{"react":"^18.3.0"}}"#,
    );
    node_module(&root, "react");

    let reality = json(&root, &["--json", "reality"]);
    let dependency = reality["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|dependency| dependency["component"] == "apps/web")
        .expect("nested dependency");
    assert_eq!(dependency["lockfile"]["source"], "package-lock.json");
    assert_eq!(dependency["lockfile"]["status"], "present");
    assert_eq!(dependency["installed"]["source"], "node_modules/react");
    assert_eq!(dependency["installed"]["status"], "present");

    let drift = json(&root, &["--json", "drift"]);
    assert_eq!(drift["status"], "match");
}

#[test]
fn python_distribution_names_are_normalized_for_reconciliation() {
    let root = temp_dir();
    write(
        &root.join("pyproject.toml"),
        "[project]\nname = \"svc\"\nversion = \"1.0.0\"\ndependencies = [\"typing_extensions>=4\"]\n",
    );
    write(
        &root.join("uv.lock"),
        "version = 1\n\n[[package]]\nname = \"svc\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"typing-extensions\"\nversion = \"4.12.0\"\n",
    );
    fs::create_dir_all(
        root.join(".venv/lib/python3.12/site-packages/typing_extensions-4.12.0.dist-info"),
    )
    .unwrap();

    let reality = json(&root, &["--json", "reality"]);
    let dependency = &reality["dependencies"][0];
    assert_eq!(dependency["lockfile"]["status"], "present");
    assert_eq!(dependency["installed"]["status"], "present");
    assert_eq!(reality["installed"]["observations"][0]["packageCount"], 1);

    let drift = json(&root, &["--json", "drift"]);
    assert_eq!(drift["status"], "match");
}

#[test]
fn python_installed_drift_requires_an_established_site_packages_set() {
    let root = temp_dir();
    write(
        &root.join("pyproject.toml"),
        "[project]\nname = \"svc\"\nversion = \"1.0.0\"\ndependencies = [\"requests>=2\"]\n",
    );
    write(
        &root.join("uv.lock"),
        "version = 1\n\n[[package]]\nname = \"requests\"\nversion = \"2.32.3\"\n",
    );

    // No environment: installed state is unknown, so no confirmed drift.
    let reality = json(&root, &["--json", "reality"]);
    assert_eq!(reality["installed"]["observations"][0]["status"], "unknown");
    assert_eq!(reality["dependencies"][0]["installed"]["status"], "unknown");
    let drift = json(&root, &["--json", "drift"]);
    assert_eq!(drift["status"], "match");

    // An empty, established site-packages set proves the declared dependency is
    // absent.
    fs::create_dir_all(root.join(".venv/lib/python3.12/site-packages")).unwrap();
    let reality = json(&root, &["--json", "reality"]);
    assert_eq!(reality["installed"]["observations"][0]["status"], "absent");
    assert_eq!(reality["installed"]["observations"][0]["packageCount"], 0);
    let drift = json(&root, &["--json", "drift"]);
    assert_eq!(drift["status"], "drift");
    assert!(
        drift["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["subject"] == "./requests"
                && issue["expected"] == "declared dependency installed")
    );
}

#[test]
fn rust_target_directory_is_not_installed_dependency_evidence() {
    let root = temp_dir();
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n",
    );
    write(&root.join("src/lib.rs"), "");
    write(
        &root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"crate-a\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
    );
    fs::create_dir_all(root.join("target")).unwrap();

    let reality = json(&root, &["--json", "reality"]);
    let installed = &reality["installed"]["observations"][0];
    assert_eq!(installed["status"], "unknown");
    assert!(installed["evidence"].as_array().unwrap().is_empty());
    assert_eq!(reality["dependencies"][0]["lockfile"]["status"], "present");
    assert_eq!(reality["dependencies"][0]["installed"]["status"], "unknown");
}

#[test]
fn lockfile_formats_are_observed_and_recorded() {
    let cases = [
        (
            "pnpm",
            "pnpm-lock.yaml",
            "lockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies:\n      left-pad:\n        specifier: ^1.0.0\n        version: 1.3.0\n",
        ),
        (
            "yarn",
            "yarn.lock",
            "# yarn lockfile v1\n\nleft-pad@^1.0.0:\n  version \"1.3.0\"\n",
        ),
        (
            "bun",
            "bun.lock",
            "{\n  \"lockfileVersion\": 1,\n  \"workspaces\": {\"\": {\"dependencies\": {\"left-pad\": \"^1.0.0\"}}},\n}\n",
        ),
    ];
    for (format, file, contents) in cases {
        let root = temp_dir();
        write(
            &root.join("package.json"),
            r#"{"name":"web","dependencies":{"left-pad":"^1.0.0"}}"#,
        );
        write(&root.join(file), contents);
        node_module(&root, "left-pad");

        let reality = json(&root, &["--json", "reality"]);
        let resolved = &reality["resolved"]["observations"][0];
        assert_eq!(resolved["format"], format, "{file}");
        assert_eq!(resolved["status"], "present", "{file}");
        assert_eq!(resolved["packageCount"], 1, "{file}");
        assert_eq!(
            reality["dependencies"][0]["lockfile"]["status"], "present",
            "{file}"
        );

        let drift = json(&root, &["--json", "drift"]);
        assert_eq!(drift["status"], "match", "{file}: {drift}");
    }
}
