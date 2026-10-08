//! `pax observe`: bounded deterministic project-structure observation.
//! See docs/PAX_OBSERVATION.md. These tests pin what is observed, with what
//! provenance, and that every failure is explicit and typed.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pax-observe-{}-{}",
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
        .args(["--json", "observe"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn observe(root: &Path, args: &[&str]) -> Value {
    let output = pax(root, args);
    assert!(
        output.status.success(),
        "pax observe {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A refusal: exit code 2, one typed JSON document on stderr, nothing on stdout.
fn refusal(root: &Path, args: &[&str]) -> Value {
    let output = pax(root, args);
    assert_eq!(output.status.code(), Some(2), "{args:?}");
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).unwrap()
}

fn package(root: &Path, dir: &str, name: &str, extra: &str, lib: &str) {
    let base = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    write(
        &base.join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{extra}"),
    );
    write(&base.join("src/lib.rs"), lib);
}

const LIB: &str = r#"pub mod a;
mod b {
    pub mod c;
    pub fn inline_fn() {}
}
pub struct Top;
pub enum Kind { One }
#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {}
}
"#;

fn rust_project(root: &Path) {
    package(root, "", "demo", "", LIB);
    write(
        &root.join("src/a.rs"),
        "pub mod nested;\npub fn in_a() {}\n",
    );
    write(&root.join("src/a/nested.rs"), "pub const DEEP: u8 = 1;\n");
    write(&root.join("src/b/c.rs"), "pub trait Tr {}\n");
}

fn facts<'a>(observation: &'a Value, relationship: &str) -> Vec<&'a Value> {
    observation["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|fact| fact["relationship"] == relationship)
        .collect()
}

fn ids(observation: &Value, relationship: &str, side: &str) -> BTreeSet<String> {
    facts(observation, relationship)
        .iter()
        .map(|fact| fact[side]["id"].as_str().unwrap().to_string())
        .collect()
}

fn diagnostics<'a>(observation: &'a Value, code: &str) -> Vec<&'a Value> {
    observation["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| diagnostic["code"] == code)
        .collect()
}

fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((path.clone(), fs::read(path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

// --- project artifacts ---------------------------------------------------

#[test]
fn existing_artifacts_are_observed_with_provenance() {
    let root = temp_dir();
    rust_project(&root);
    let observation = observe(&root, &[]);
    assert_eq!(observation["schema"], "pax.observation.v1");
    assert_eq!(observation["status"], "ok");
    assert_eq!(observation["scope"]["kind"], "project");
    assert_eq!(observation["tool"]["name"], "pax");
    assert!(observation["observed_at"].as_u64().unwrap() > 0);
    let existing = ids(&observation, "artifact.exists", "subject");
    assert!(
        existing.contains("Cargo.toml") && existing.contains("src"),
        "{existing:?}"
    );
    assert!(
        !existing.contains("tests"),
        "absent directories are not reported"
    );
    for fact in facts(&observation, "artifact.exists") {
        assert_eq!(fact["provenance"]["strength"], "observed");
        assert_eq!(fact["provenance"]["method"], "fs.stat");
    }
}

#[test]
fn missing_artifact_is_a_typed_error_not_an_empty_success() {
    let root = temp_dir();
    rust_project(&root);
    let error = refusal(&root, &["--scope", "path:nope"]);
    assert_eq!(error["schema"], "pax.observation.v1");
    assert_eq!(error["status"], "error");
    assert_eq!(error["code"], "scope_not_found");
    assert_eq!(
        refusal(&root, &["--scope", "file:src/nope.rs"])["code"],
        "scope_not_found"
    );
    assert_eq!(
        refusal(&root, &["--scope", "crate:ghost"])["code"],
        "scope_not_found"
    );
    assert_eq!(
        refusal(&root, &["--scope", "module:demo/lib::crate::ghost"])["code"],
        "scope_not_found"
    );
}

#[test]
fn path_prefix_scope_is_bounded_to_the_prefix() {
    let root = temp_dir();
    rust_project(&root);
    let observation = observe(&root, &["--scope", "path:src/a"]);
    let files = ids(&observation, "artifact.exists", "subject");
    assert_eq!(files, BTreeSet::from(["src/a/nested.rs".to_string()]));
    assert!(
        ids(&observation, "declaration.located_at", "subject")
            .iter()
            .all(|id| id.starts_with("src/a/nested.rs::"))
    );
    assert_eq!(observation["cost"]["files_inspected"], 1);
}

// --- Rust structure --------------------------------------------------------

#[test]
fn rust_crate_module_file_declaration_chain() {
    let root = temp_dir();
    rust_project(&root);
    let observation = observe(&root, &[]);
    assert_eq!(
        observation["status"], "ok",
        "{}",
        observation["diagnostics"]
    );

    assert_eq!(
        ids(&observation, "crate.contains", "subject"),
        BTreeSet::from(["demo/lib".to_string()])
    );
    let contained = ids(&observation, "module.contains", "object");
    for module in [
        "demo/lib::crate::a",
        "demo/lib::crate::a::nested", // nested, file-backed
        "demo/lib::crate::b",         // inline
        "demo/lib::crate::b::c",      // file inside an inline module: src/b/c.rs
        "demo/lib::crate::tests",
    ] {
        assert!(
            contained.contains(module),
            "{module} missing from {contained:?}"
        );
    }
    let located = |module: &str| -> String {
        facts(&observation, "module.located_at")
            .iter()
            .find(|fact| fact["subject"]["id"] == module)
            .map(|fact| fact["location"]["path"].as_str().unwrap().to_string())
            .unwrap()
    };
    assert_eq!(located("demo/lib::crate"), "src/lib.rs");
    assert_eq!(located("demo/lib::crate::a::nested"), "src/a/nested.rs");
    assert_eq!(located("demo/lib::crate::b::c"), "src/b/c.rs");
    assert_eq!(located("demo/lib::crate::b"), "src/lib.rs"); // inline

    let declarations = ids(&observation, "declaration.located_at", "subject");
    for declaration in [
        "demo/lib::crate::Top",
        "demo/lib::crate::Kind",
        "demo/lib::crate::a::in_a",
        "demo/lib::crate::a::nested::DEEP",
        "demo/lib::crate::b::inline_fn",
        "demo/lib::crate::b::c::Tr",
    ] {
        assert!(declarations.contains(declaration), "{declaration} missing");
    }
    let top = facts(&observation, "declaration.located_at")
        .into_iter()
        .find(|fact| fact["subject"]["id"] == "demo/lib::crate::Top")
        .unwrap();
    assert_eq!(top["subject"]["attributes"]["kind"], "struct");
    assert_eq!(top["subject"]["attributes"]["visibility"], "pub");
    assert_eq!(top["location"]["path"], "src/lib.rs");
    assert_eq!(top["location"]["line"], 6);
    assert_eq!(top["provenance"]["method"], "syn.parse_file");
}

#[test]
fn module_scope_descends_only_the_requested_path() {
    let root = temp_dir();
    rust_project(&root);
    let observation = observe(&root, &["--scope", "module:demo/lib::crate::a"]);
    let declarations = ids(&observation, "declaration.located_at", "subject");
    assert!(declarations.contains("demo/lib::crate::a::in_a"));
    assert!(declarations.contains("demo/lib::crate::a::nested::DEEP"));
    assert!(
        !declarations.contains("demo/lib::crate::Top"),
        "ancestor declarations are out of scope"
    );
    assert!(
        !declarations.iter().any(|id| id.contains("::b::")),
        "siblings are not walked"
    );
    let read = observation["cost"]["files_inspected"].as_u64().unwrap();
    assert_eq!(read, 4, "lib.rs, a.rs, nested.rs, plus the manifest");
}

#[test]
fn crate_scope_reports_the_package_and_not_other_members() {
    let root = temp_dir();
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"3\"\n",
    );
    package(&root, "crates/a", "a", "", "pub fn only_a() {}\n");
    package(&root, "crates/b", "b", "", "pub fn only_b() {}\n");
    let scoped = observe(&root, &["--scope", "crate:a"]);
    let declared = ids(&scoped, "declaration.located_at", "subject");
    assert!(
        declared.contains("a/lib::crate::only_a")
            && !declared.iter().any(|id| id.contains("only_b"))
    );
    let all = observe(&root, &[]);
    assert_eq!(
        ids(&all, "workspace.member", "object"),
        BTreeSet::from(["a".to_string(), "b".to_string()])
    );
}

#[test]
fn tests_are_reported_as_syntactic_declarations_only() {
    let root = temp_dir();
    rust_project(&root);
    write(&root.join("tests/it.rs"), "#[test]\nfn integration() {}\n");
    let observation = observe(&root, &[]);
    let tests = ids(&observation, "test.declared", "subject");
    assert_eq!(
        tests,
        BTreeSet::from([
            "demo/lib::crate::tests::it_works".to_string(),
            "demo/test/it::crate::integration".to_string(),
        ])
    );
    let cfg = facts(&observation, "module.contains")
        .into_iter()
        .find(|fact| fact["object"]["id"] == "demo/lib::crate::tests")
        .unwrap();
    assert_eq!(
        cfg["object"]["attributes"]["cfg"], "test",
        "cfg is reported, never evaluated"
    );
    // No test -> source linkage and no coverage claim is made.
    assert!(facts(&observation, "test.covers").is_empty());
}

// --- malformed / unsupported / inaccessible -------------------------------

#[test]
fn malformed_source_is_reported_not_guessed() {
    let root = temp_dir();
    rust_project(&root);
    write(&root.join("src/a/nested.rs"), "pub fn (\n");
    let observation = observe(&root, &[]);
    assert_eq!(observation["status"], "partial");
    let bad = diagnostics(&observation, "syntax_error");
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0]["state"], "unparseable");
    assert_eq!(bad[0]["location"]["path"], "src/a/nested.rs");
    // The file exists and is located; nothing is invented for its contents.
    assert!(
        ids(&observation, "module.located_at", "subject").contains("demo/lib::crate::a::nested")
    );
    assert!(
        !ids(&observation, "declaration.located_at", "subject")
            .iter()
            .any(|id| id.contains("nested::"))
    );
    // Siblings are still observed.
    assert!(
        ids(&observation, "declaration.located_at", "subject").contains("demo/lib::crate::a::in_a")
    );
}

#[test]
fn unsupported_and_unresolved_constructs_are_explicit() {
    let root = temp_dir();
    package(
        &root,
        "",
        "demo",
        "[lib]\npath = \"src/lib.rs\"\n",
        "#[path = \"elsewhere.rs\"]\nmod moved;\nmod missing;\nmod both;\n",
    );
    write(&root.join("src/both.rs"), "");
    write(&root.join("src/both/mod.rs"), "");
    let observation = observe(&root, &[]);
    assert_eq!(observation["status"], "partial");
    assert_eq!(
        diagnostics(&observation, "custom_target_path")[0]["state"],
        "unsupported"
    );
    assert_eq!(
        diagnostics(&observation, "mod_path_attribute")[0]["state"],
        "unsupported"
    );
    assert_eq!(
        diagnostics(&observation, "module_file_missing")[0]["state"],
        "unresolved"
    );
    assert_eq!(
        diagnostics(&observation, "module_file_ambiguous")[0]["state"],
        "unresolved"
    );
    // A declared-but-unchecked module stays `declared`; it is not upgraded to observed.
    for name in ["moved", "missing", "both"] {
        let fact = facts(&observation, "module.contains")
            .into_iter()
            .find(|fact| fact["object"]["id"] == format!("demo/lib::crate::{name}"))
            .unwrap();
        assert_eq!(fact["provenance"]["strength"], "declared", "{name}");
    }
    assert!(
        facts(&observation, "module.located_at")
            .iter()
            .all(|fact| fact["subject"]["id"] == "demo/lib::crate")
    );
}

#[test]
fn non_rust_projects_do_not_imply_source_support() {
    let root = temp_dir();
    write(&root.join("package.json"), r#"{"name":"web"}"#);
    write(&root.join("index.js"), "export const x = 1;\n");
    let observation = observe(&root, &[]);
    assert_eq!(observation["status"], "partial");
    assert!(ids(&observation, "artifact.exists", "subject").contains("package.json"));
    let unsupported = diagnostics(&observation, "source_structure_unsupported");
    assert_eq!(unsupported[0]["state"], "unsupported");
    assert!(facts(&observation, "declaration.located_at").is_empty());
    assert_eq!(
        refusal(&root, &["--scope", "crate:web"])["code"],
        "unsupported_project"
    );
}

#[test]
fn non_utf8_source_is_unparseable() {
    let root = temp_dir();
    rust_project(&root);
    fs::write(root.join("src/b/c.rs"), [0xff, 0xfe, 0x00]).unwrap();
    let observation = observe(&root, &[]);
    assert_eq!(
        diagnostics(&observation, "not_utf8")[0]["state"],
        "unparseable"
    );
}

#[cfg(unix)]
#[test]
fn inaccessible_artifact_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_dir();
    rust_project(&root);
    let path = root.join("src/b/c.rs");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&path).is_ok() {
        return; // running with privileges that ignore permissions
    }
    let observation = observe(&root, &[]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(observation["status"], "partial");
    assert_eq!(
        diagnostics(&observation, "artifact_unreadable")[0]["state"],
        "unreadable"
    );
}

#[test]
fn invalid_scopes_and_limits_are_typed_errors() {
    let root = temp_dir();
    rust_project(&root);
    for scope in [
        "bogus",
        "crate:",
        "module:demo/lib",
        "module:demo/lib::mod::a",
        "file:../outside.rs",
        "path:/etc",
        "path:",
        "file:",
        "path:src/../..",
    ] {
        let error = refusal(&root, &["--scope", scope]);
        assert_eq!(error["code"], "invalid_scope", "{scope}: {error}");
    }
    assert_eq!(
        refusal(&root, &["--scope", "file:Cargo.toml"])["code"],
        "unsupported_scope"
    );
    assert_eq!(
        refusal(&root, &["--scope", "file:src"])["code"],
        "unsupported_scope"
    );
    assert_eq!(
        refusal(&root, &["--scope", "path:src", "--max-files", "many"])["code"],
        "invalid_limit"
    );
}

#[cfg(unix)]
#[test]
fn scope_cannot_escape_the_project_root_through_symlinks() {
    let root = temp_dir();
    rust_project(&root);
    let outside = temp_dir();
    write(&outside.join("x.rs"), "pub fn leaked() {}\n");
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    assert_eq!(
        refusal(&root, &["--scope", "path:link"])["code"],
        "invalid_scope"
    );
}

// --- dependencies and provenance ------------------------------------------

#[test]
fn dependencies_stay_declared_and_workspace_membership_is_resolved_by_cargo() {
    let root = temp_dir();
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"3\"\n",
    );
    package(
        &root,
        "crates/a",
        "a",
        "\n[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n",
        "",
    );
    let observation = observe(&root, &[]);
    let dependencies = facts(&observation, "dependency.declared");
    let kinds = dependencies
        .iter()
        .map(|fact| {
            (
                fact["object"]["id"].as_str().unwrap().to_string(),
                fact["object"]["attributes"]["kind"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                fact["object"]["attributes"]["specifier"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        kinds,
        BTreeSet::from([
            (
                "serde".to_string(),
                "dependency".to_string(),
                "^1".to_string()
            ),
            (
                "tempfile".to_string(),
                "development".to_string(),
                "^3".to_string()
            ),
        ])
    );
    for fact in &dependencies {
        assert_eq!(
            fact["provenance"]["strength"], "declared",
            "no resolution happened"
        );
    }
    let members = facts(&observation, "workspace.member");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["provenance"]["strength"], "resolved");
    assert_eq!(members[0]["provenance"]["method"], "cargo.metadata");
}

#[test]
fn every_fact_has_bounded_provenance_and_no_interpretive_vocabulary() {
    let root = temp_dir();
    rust_project(&root);
    write(&root.join("tests/it.rs"), "#[test]\nfn integration() {}\n");
    let observation = observe(&root, &[]);
    let allowed = BTreeSet::from(["declared", "observed", "resolved"]);
    for fact in observation["facts"].as_array().unwrap() {
        let provenance = &fact["provenance"];
        assert!(
            allowed.contains(provenance["strength"].as_str().unwrap()),
            "{fact}"
        );
        assert!(
            provenance["method"].as_str().is_some_and(|m| !m.is_empty()),
            "{fact}"
        );
        assert!(
            provenance["source"].as_str().is_some_and(|m| !m.is_empty()),
            "{fact}"
        );
        let relationship = fact["relationship"].as_str().unwrap();
        for word in [
            "relevan",
            "recommend",
            "likely",
            "safe",
            "impact",
            "goal",
            "fix",
            "covers",
        ] {
            assert!(!relationship.contains(word), "{relationship}");
        }
        assert!(
            fact["subject"]["type"].is_string() && fact["subject"]["id"].is_string(),
            "{fact}"
        );
    }
}

#[test]
fn observation_is_deterministic_apart_from_time_and_cost() {
    let root = temp_dir();
    rust_project(&root);
    let strip = |mut value: Value| {
        value["observed_at"] = Value::Null;
        value["cost"]["elapsed_ms"] = Value::Null;
        value
    };
    assert_eq!(strip(observe(&root, &[])), strip(observe(&root, &[])));
}

// --- boundedness -----------------------------------------------------------

#[test]
fn bounded_scope_does_not_traverse_the_rest_of_a_large_repository() {
    let root = temp_dir();
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"3\"\n",
    );
    package(&root, "crates/a", "a", "", "pub mod m;\n");
    write(&root.join("crates/a/src/m.rs"), "pub fn f() {}\n");
    package(&root, "crates/b", "b", "", "");
    for index in 0..400 {
        write(
            &root.join(format!("vendor/dir{}/file{index}.rs", index % 20)),
            "pub fn x() {}\n",
        );
        write(
            &root.join(format!("crates/b/src/stray{index}.rs")),
            "pub fn y() {}\n",
        );
    }
    for args in [
        vec!["--scope", "crate:a"],
        vec!["--scope", "module:a/lib::crate::m"],
        vec!["--scope", "file:crates/a/src/m.rs"],
    ] {
        let observation = observe(&root, &args);
        let cost = &observation["cost"];
        assert!(
            cost["entries_listed"].as_u64().unwrap() < 50,
            "{args:?}: {cost}"
        );
        assert!(
            cost["files_inspected"].as_u64().unwrap() <= 4,
            "{args:?}: {cost}"
        );
    }
    // Project scope walks only declared module trees, not stray or vendored files.
    let project = observe(&root, &[]);
    assert!(
        project["cost"]["files_inspected"].as_u64().unwrap() <= 6,
        "{}",
        project["cost"]
    );
    assert!(project["cost"]["entries_listed"].as_u64().unwrap() < 100);
}

#[test]
fn exceeding_a_limit_is_an_explicit_error_never_a_truncation() {
    let root = temp_dir();
    rust_project(&root);
    let files = refusal(&root, &["--max-files", "2"]);
    assert_eq!(files["code"], "limit_exceeded");
    assert_eq!(files["limit"], "max_files");
    assert_eq!(files["limit_value"], 2);
    assert!(files["cost"]["files_inspected"].as_u64().unwrap() <= 2);

    let bytes = refusal(&root, &["--max-bytes", "10"]);
    assert_eq!(
        (bytes["code"].as_str(), bytes["limit"].as_str()),
        (Some("limit_exceeded"), Some("max_bytes"))
    );

    let fact_limit = refusal(&root, &["--max-facts", "3"]);
    assert_eq!(fact_limit["limit"], "max_facts");

    // The same request within bounds is complete, with the cost reported.
    let ok = observe(
        &root,
        &[
            "--max-files",
            "50",
            "--max-bytes",
            "100000",
            "--max-facts",
            "500",
        ],
    );
    assert_eq!(ok["limits"]["max_files"], 50);
    assert!(ok["cost"]["bytes_read"].as_u64().unwrap() > 0);
    assert_eq!(ok["cost"]["facts"], ok["facts"].as_array().unwrap().len());
}

#[test]
fn path_scope_over_the_limit_refuses_instead_of_widening() {
    let root = temp_dir();
    rust_project(&root);
    for index in 0..30 {
        write(&root.join(format!("many/f{index}.rs")), "pub fn x() {}\n");
    }
    let error = refusal(&root, &["--scope", "path:many", "--max-files", "10"]);
    assert_eq!(error["code"], "limit_exceeded");
}

// --- non-agency and compatibility ------------------------------------------

#[test]
fn observation_writes_nothing() {
    let root = temp_dir();
    rust_project(&root);
    let before = tree(&root);
    observe(&root, &[]);
    observe(&root, &["--scope", "path:src"]);
    let _ = pax(&root, &["--scope", "path:nope"]);
    assert_eq!(before, tree(&root));
}

#[test]
fn observe_options_are_rejected_on_other_commands() {
    let root = temp_dir();
    rust_project(&root);
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "info", "--scope", "project"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn text_output_names_scope_provenance_and_cost() {
    let root = temp_dir();
    rust_project(&root);
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["observe", "--scope", "file:src/a.rs"])
        .current_dir(&root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("scope=file:src/a.rs"), "{text}");
    assert!(text.contains("[observed/syn.parse_file]"), "{text}");
    assert!(text.contains("cost:"), "{text}");
}

#[test]
fn separate_crates_may_share_a_module_file() {
    let root = temp_dir();
    rust_project(&root);
    write(&root.join("tests/common/mod.rs"), "pub fn helper() {}\n");
    write(&root.join("tests/one.rs"), "mod common;\n");
    write(&root.join("tests/two.rs"), "mod common;\n");
    let observation = observe(&root, &[]);
    assert_eq!(
        observation["status"], "ok",
        "{}",
        observation["diagnostics"]
    );
    let declared = ids(&observation, "declaration.located_at", "subject");
    assert!(declared.contains("demo/test/one::crate::common::helper"));
    assert!(declared.contains("demo/test/two::crate::common::helper"));
}

#[cfg(unix)]
#[test]
fn module_files_cannot_be_read_through_symlinks_leaving_the_root() {
    let root = temp_dir();
    let outside = temp_dir();
    write(&outside.join("leak.rs"), "pub fn secret_outside() {}\n");
    package(&root, "", "demo", "", "mod linked;\nmod inside;\n");
    write(&root.join("real/inside.rs"), "pub fn fine() {}\n");
    std::os::unix::fs::symlink(outside.join("leak.rs"), root.join("src/linked.rs")).unwrap();
    // A symlink that stays inside the project is still ordinary project content.
    std::os::unix::fs::symlink(root.join("real/inside.rs"), root.join("src/inside.rs")).unwrap();
    let observation = observe(&root, &[]);
    let declared = ids(&observation, "declaration.located_at", "subject");
    assert!(
        !declared.iter().any(|id| id.contains("secret_outside")),
        "{declared:?}"
    );
    assert!(declared.contains("demo/lib::crate::inside::fine"));
    assert_eq!(
        diagnostics(&observation, "artifact_outside_root")[0]["state"],
        "unreadable"
    );
    // Explicit scopes reach the same refusal.
    assert_eq!(
        refusal(&root, &["--scope", "file:src/linked.rs"])["code"],
        "invalid_scope"
    );
}
