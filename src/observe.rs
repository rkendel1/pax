//! Bounded, deterministic project-structure observation (`pax observe`).
//!
//! Reports facts that can be established from project artifacts and Rust source
//! files, each with provenance. It does not interpret facts: no relevance, impact,
//! safety, or next-action semantics belong here. It calls no model, runs no project
//! command (only the `cargo metadata --no-deps --offline` that the other observation
//! commands already use), and writes nothing.
//!
//! Schema: `pax.observation.v1`. See docs/PAX_OBSERVATION.md.

use super::{CargoObservation, CliError, detect_cargo};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use syn::{Attribute, Item, Visibility};

pub const SCHEMA: &str = "pax.observation.v1";
const PARSER: &str = "syn 2 (full)";
const DEFAULT_MAX_FILES: usize = 200;
const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_MAX_FACTS: usize = 5_000;
const CONVENTIONAL_DIRS: [&str; 5] = ["src", "tests", "benches", "examples", "docs"];
const NON_RUST_MANIFESTS: [(&str, &str); 2] =
    [("package.json", "javascript"), ("pyproject.toml", "python")];
const CARGO_METADATA: &str = "cargo metadata --no-deps --offline";

/// Raw, unvalidated option values; validation failures are typed observation errors.
#[derive(Debug, Default)]
pub struct ObserveOptions {
    pub scope: Option<String>,
    pub max_files: Option<String>,
    pub max_bytes: Option<String>,
    pub max_facts: Option<String>,
}

impl ObserveOptions {
    pub fn is_empty(&self) -> bool {
        self.scope.is_none()
            && self.max_files.is_none()
            && self.max_bytes.is_none()
            && self.max_facts.is_none()
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Limits {
    max_files: usize,
    max_bytes: u64,
    max_facts: usize,
}

#[derive(Clone, Debug, Serialize)]
struct Cost {
    entries_listed: usize,
    files_inspected: usize,
    files_parsed: usize,
    bytes_read: u64,
    facts: usize,
    elapsed_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
struct Entity {
    #[serde(rename = "type")]
    kind: &'static str,
    id: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    attributes: BTreeMap<&'static str, String>,
}

impl Entity {
    fn new(kind: &'static str, id: impl Into<String>) -> Self {
        Entity {
            kind,
            id: id.into(),
            attributes: BTreeMap::new(),
        }
    }

    fn with(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.attributes.insert(key, value.into());
        self
    }
}

#[derive(Debug, Serialize)]
struct Location {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
}

fn at(path: &str, line: usize) -> Option<Location> {
    Some(Location {
        path: path.to_string(),
        line: Some(line),
    })
}

fn at_file(path: &str) -> Option<Location> {
    Some(Location {
        path: path.to_string(),
        line: None,
    })
}

/// How strongly a fact is established. `verified` is deliberately absent: observation
/// never executes anything, so it can never establish a verified fact. Strength is
/// never upgraded without evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Strength {
    /// A project artifact states the relationship; nothing checked it.
    Declared,
    /// PAX read the artifact on this machine at observation time.
    Observed,
    /// A resolver (Cargo) established the relationship.
    Resolved,
}

impl Strength {
    fn as_str(self) -> &'static str {
        match self {
            Strength::Declared => "declared",
            Strength::Observed => "observed",
            Strength::Resolved => "resolved",
        }
    }
}

#[derive(Debug, Serialize)]
struct Provenance {
    strength: Strength,
    method: &'static str,
    source: String,
}

#[derive(Debug, Serialize)]
struct Fact {
    relationship: &'static str,
    subject: Entity,
    #[serde(skip_serializing_if = "Option::is_none")]
    object: Option<Entity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location>,
    provenance: Provenance,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum DiagnosticState {
    Unsupported,
    Unparseable,
    Unreadable,
    Unresolved,
}

/// An explicit statement of something PAX could not establish. Never a guess.
#[derive(Debug, Serialize)]
struct Diagnostic {
    code: &'static str,
    state: DiagnosticState,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location>,
}

#[derive(Debug, Serialize)]
struct ScopeOutput {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProjectOutput {
    root: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct ToolOutput {
    name: &'static str,
    version: &'static str,
    parser: &'static str,
}

#[derive(Debug, Serialize)]
struct Observation {
    schema: &'static str,
    status: &'static str,
    project: ProjectOutput,
    scope: ScopeOutput,
    observed_at: u64,
    tool: ToolOutput,
    limits: Limits,
    cost: Cost,
    facts: Vec<Fact>,
    diagnostics: Vec<Diagnostic>,
}

/// A typed refusal. Observation never silently widens, truncates, or guesses.
#[derive(Debug, Serialize)]
struct ObservationError {
    schema: &'static str,
    status: &'static str,
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_value: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<Cost>,
}

type Observed<T> = Result<T, Box<ObservationError>>;

fn refuse(code: &'static str, message: impl Into<String>) -> Box<ObservationError> {
    Box::new(ObservationError {
        schema: SCHEMA,
        status: "error",
        code,
        message: message.into(),
        limit: None,
        limit_value: None,
        cost: None,
    })
}

enum Scope {
    Project,
    Crate(String),
    Module {
        crate_id: String,
        chain: Vec<String>,
    },
    File(String),
    Path(String),
}

impl Scope {
    fn output(&self, spec: Option<&str>) -> ScopeOutput {
        let (kind, value) = match self {
            Scope::Project => ("project", None),
            Scope::Crate(_) => ("crate", spec.and_then(|s| s.strip_prefix("crate:"))),
            Scope::Module { .. } => ("module", spec.and_then(|s| s.strip_prefix("module:"))),
            Scope::File(_) => ("file", spec.and_then(|s| s.strip_prefix("file:"))),
            Scope::Path(_) => ("path", spec.and_then(|s| s.strip_prefix("path:"))),
        };
        ScopeOutput {
            kind,
            value: value.map(str::to_string),
        }
    }
}

fn parse_scope(spec: Option<&str>) -> Observed<Scope> {
    let Some(spec) = spec else {
        return Ok(Scope::Project);
    };
    if spec == "project" {
        return Ok(Scope::Project);
    }
    let invalid = || {
        refuse(
            "invalid_scope",
            format!(
                "invalid scope {spec:?}; expected project, crate:<package>[/<kind>[/<name>]], \
                 module:<crate-id>::crate[::<module>...], file:<path>, or path:<prefix> (use path:. for the whole project)"
            ),
        )
    };
    let (kind, value) = spec.split_once(':').ok_or_else(invalid)?;
    if value.is_empty() {
        return Err(invalid());
    }
    match kind {
        "crate" => Ok(Scope::Crate(value.to_string())),
        "module" => {
            let (crate_id, path) = value.split_once("::").ok_or_else(invalid)?;
            let mut segments = path.split("::");
            if crate_id.is_empty() || segments.next() != Some("crate") {
                return Err(invalid());
            }
            let chain = segments.map(str::to_string).collect::<Vec<_>>();
            if chain.iter().any(String::is_empty) {
                return Err(invalid());
            }
            Ok(Scope::Module {
                crate_id: crate_id.to_string(),
                chain,
            })
        }
        "file" => Ok(Scope::File(normalize_relative(value)?)),
        "path" => Ok(Scope::Path(normalize_relative(value)?)),
        _ => Err(invalid()),
    }
}

/// Normalizes a user-supplied relative path to `/`-separated components; rejects
/// absolute paths and `..`.
fn normalize_relative(value: &str) -> Observed<String> {
    if value.starts_with('/') || value.contains('\\') {
        return Err(refuse(
            "invalid_scope",
            format!("scope path {value:?} must be relative to the project root and use `/`"),
        ));
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(refuse(
                    "invalid_scope",
                    format!("scope path {value:?} must not contain `..`"),
                ));
            }
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

fn parse_limits(options: &ObserveOptions) -> Observed<Limits> {
    fn number<T: std::str::FromStr + Default + PartialOrd>(
        name: &'static str,
        raw: &Option<String>,
        default: T,
    ) -> Observed<T> {
        match raw {
            None => Ok(default),
            Some(raw) => raw
                .parse::<T>()
                .ok()
                .filter(|value| *value > T::default())
                .ok_or_else(|| {
                    refuse(
                        "invalid_limit",
                        format!("--{name} requires a positive integer, got {raw:?}"),
                    )
                }),
        }
    }
    Ok(Limits {
        max_files: number("max-files", &options.max_files, DEFAULT_MAX_FILES)?,
        max_bytes: number("max-bytes", &options.max_bytes, DEFAULT_MAX_BYTES)?,
        max_facts: number("max-facts", &options.max_facts, DEFAULT_MAX_FACTS)?,
    })
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// One Cargo target whose root file was found by Cargo's directory convention.
struct Target {
    crate_id: String,
    kind: &'static str,
    package: String,
    root_file: String,
}

struct Ctx<'c> {
    file: String,
    module_id: String,
    /// Directory that `mod x;` declarations in this module resolve against.
    child_dir: String,
    /// Remaining module path to descend; empty means "observe this module fully".
    chain: &'c [String],
    /// File scopes know no crate, so external `mod x;` files are not resolved.
    file_mode: bool,
}

struct Obs<'a> {
    root: &'a Path,
    limits: Limits,
    started: Instant,
    entries_listed: usize,
    files_inspected: usize,
    files_parsed: usize,
    bytes_read: u64,
    facts: Vec<Fact>,
    diagnostics: Vec<Diagnostic>,
    visited: BTreeSet<String>,
}

impl<'a> Obs<'a> {
    fn new(root: &'a Path, limits: Limits) -> Self {
        Obs {
            root,
            limits,
            started: Instant::now(),
            entries_listed: 0,
            files_inspected: 0,
            files_parsed: 0,
            bytes_read: 0,
            facts: Vec::new(),
            diagnostics: Vec::new(),
            visited: BTreeSet::new(),
        }
    }

    fn abs(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.to_path_buf()
        } else {
            self.root.join(rel)
        }
    }

    fn cost(&self) -> Cost {
        Cost {
            entries_listed: self.entries_listed,
            files_inspected: self.files_inspected,
            files_parsed: self.files_parsed,
            bytes_read: self.bytes_read,
            facts: self.facts.len(),
            elapsed_ms: self.started.elapsed().as_millis(),
        }
    }

    fn limit_error(&self, limit: &'static str, value: u64, what: &str) -> Box<ObservationError> {
        Box::new(ObservationError {
            schema: SCHEMA,
            status: "error",
            code: "limit_exceeded",
            message: format!(
                "scope exceeds {limit} ({value}) while {what}; narrow the scope or raise --{}",
                limit.replace('_', "-")
            ),
            limit: Some(limit),
            limit_value: Some(value),
            cost: Some(self.cost()),
        })
    }

    fn count_entry(&mut self, what: &str) -> Observed<()> {
        if self.entries_listed >= self.limits.max_files {
            return Err(self.limit_error("max_files", self.limits.max_files as u64, what));
        }
        self.entries_listed += 1;
        Ok(())
    }

    fn push(&mut self, fact: Fact) -> Observed<()> {
        if self.facts.len() >= self.limits.max_facts {
            return Err(self.limit_error(
                "max_facts",
                self.limits.max_facts as u64,
                "collecting facts",
            ));
        }
        self.facts.push(fact);
        Ok(())
    }

    fn diagnose(
        &mut self,
        code: &'static str,
        state: DiagnosticState,
        message: impl Into<String>,
        location: Option<Location>,
    ) {
        self.diagnostics.push(Diagnostic {
            code,
            state,
            message: message.into(),
            location,
        });
    }

    /// `Some(is_dir)` if the path exists, `None` if it does not (or cannot be read,
    /// which is reported as a diagnostic).
    fn stat(&mut self, rel: &str) -> Observed<Option<bool>> {
        self.count_entry(&format!("inspecting {rel}"))?;
        match fs::metadata(self.abs(rel)) {
            Ok(meta) => Ok(Some(meta.is_dir())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                self.diagnose(
                    "artifact_unreadable",
                    DiagnosticState::Unreadable,
                    format!("cannot inspect {rel}: {error}"),
                    at_file(rel),
                );
                Ok(None)
            }
        }
    }

    /// Directory entries (files and directories, never symlinks), sorted by name.
    fn list_dir(&mut self, rel: &str) -> Observed<Vec<(String, bool)>> {
        let entries = match fs::read_dir(self.abs(rel)) {
            Ok(entries) => entries,
            Err(error) => {
                self.diagnose(
                    "artifact_unreadable",
                    DiagnosticState::Unreadable,
                    format!("cannot list {rel}: {error}"),
                    at_file(rel),
                );
                return Ok(Vec::new());
            }
        };
        let mut listed = Vec::new();
        for entry in entries.flatten() {
            self.count_entry(&format!("listing {rel}"))?;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                listed.push((name.to_string(), kind.is_dir()));
            }
        }
        listed.sort();
        Ok(listed)
    }

    fn read_source(&mut self, rel: &str) -> Observed<Option<String>> {
        if self.files_inspected >= self.limits.max_files {
            return Err(self.limit_error(
                "max_files",
                self.limits.max_files as u64,
                &format!("reading {rel}"),
            ));
        }
        let path = self.abs(rel);
        // Never read through a symlink that leaves the project root (e.g. `mod x;`
        // resolving to a linked file).
        let contained = match (fs::canonicalize(&path), fs::canonicalize(self.root)) {
            (Ok(path), Ok(root)) => path.starts_with(root),
            _ => true, // unreadable paths are reported by the metadata check below
        };
        if !contained {
            self.diagnose(
                "artifact_outside_root",
                DiagnosticState::Unreadable,
                format!("{rel} resolves outside the project root; not read"),
                at_file(rel),
            );
            return Ok(None);
        }
        let size = match fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(error) => {
                self.diagnose(
                    "artifact_unreadable",
                    DiagnosticState::Unreadable,
                    format!("cannot read {rel}: {error}"),
                    at_file(rel),
                );
                return Ok(None);
            }
        };
        if self.bytes_read.saturating_add(size) > self.limits.max_bytes {
            return Err(self.limit_error(
                "max_bytes",
                self.limits.max_bytes,
                &format!("reading {rel} ({size} bytes)"),
            ));
        }
        match fs::read(&path) {
            Ok(bytes) => {
                self.files_inspected += 1;
                self.bytes_read += bytes.len() as u64;
                match String::from_utf8(bytes) {
                    Ok(text) => Ok(Some(text)),
                    Err(_) => {
                        self.diagnose(
                            "not_utf8",
                            DiagnosticState::Unparseable,
                            format!("{rel} is not valid UTF-8; not parsed"),
                            at_file(rel),
                        );
                        Ok(None)
                    }
                }
            }
            Err(error) => {
                self.diagnose(
                    "artifact_unreadable",
                    DiagnosticState::Unreadable,
                    format!("cannot read {rel}: {error}"),
                    at_file(rel),
                );
                Ok(None)
            }
        }
    }

    fn parse(&mut self, rel: &str, source: &str) -> Option<syn::File> {
        self.files_parsed += 1;
        match syn::parse_file(source) {
            Ok(file) => Some(file),
            Err(error) => {
                let line = error.span().start().line;
                self.diagnose(
                    "syntax_error",
                    DiagnosticState::Unparseable,
                    format!("{rel} is not parseable Rust: {error}"),
                    Some(Location {
                        path: rel.to_string(),
                        line: (line > 0).then_some(line),
                    }),
                );
                None
            }
        }
    }

    fn exists_fact(&mut self, kind: &'static str, rel: &str) -> Observed<()> {
        let id = if rel.is_empty() { "." } else { rel };
        self.push(Fact {
            relationship: "artifact.exists",
            subject: Entity::new(kind, id),
            object: None,
            location: at_file(id),
            provenance: Provenance {
                strength: Strength::Observed,
                method: "fs.stat",
                source: id.to_string(),
            },
        })
    }

    fn observed_source_fact(
        &mut self,
        relationship: &'static str,
        subject: Entity,
        object: Option<Entity>,
        location: Option<Location>,
        method: &'static str,
        file: &str,
    ) -> Observed<()> {
        self.push(Fact {
            relationship,
            subject,
            object,
            location,
            provenance: Provenance {
                strength: Strength::Observed,
                method,
                source: file.to_string(),
            },
        })
    }

    /// Walks the items of one parsed module. Module files are resolved by Rust's
    /// path rules (not by rustc); `#[path]`, `cfg` evaluation, and macro-generated
    /// items are not followed or evaluated.
    fn walk_items(&mut self, items: &[Item], ctx: &Ctx, found: &mut bool) -> Observed<()> {
        for item in items {
            if let Item::Mod(module) = item {
                self.walk_mod(module, ctx, found)?;
                continue;
            }
            if !ctx.chain.is_empty() {
                continue;
            }
            let Some(decl) = declaration(item) else {
                continue;
            };
            let mut entity =
                Entity::new("declaration", format!("{}::{}", ctx.module_id, decl.name))
                    .with("kind", decl.kind)
                    .with("visibility", decl.visibility)
                    .with("module", ctx.module_id.clone());
            if let Some(cfg) = cfg_of(decl.attrs) {
                entity = entity.with("cfg", cfg);
            }
            let test_attribute = (decl.kind == "fn")
                .then(|| test_attribute(decl.attrs))
                .flatten();
            let test_entity = test_attribute.map(|attribute| {
                let mut test = Entity::new("test", format!("{}::{}", ctx.module_id, decl.name))
                    .with("attribute", attribute)
                    .with("module", ctx.module_id.clone());
                if let Some(cfg) = cfg_of(decl.attrs) {
                    test = test.with("cfg", cfg);
                }
                test
            });
            self.observed_source_fact(
                "declaration.located_at",
                entity,
                None,
                at(&ctx.file, decl.line),
                "syn.parse_file",
                &ctx.file,
            )?;
            if let Some(test) = test_entity {
                self.observed_source_fact(
                    "test.declared",
                    test,
                    None,
                    at(&ctx.file, decl.line),
                    "syn.parse_file",
                    &ctx.file,
                )?;
            }
        }
        Ok(())
    }

    fn walk_mod(&mut self, module: &syn::ItemMod, ctx: &Ctx, found: &mut bool) -> Observed<()> {
        let name = module
            .ident
            .to_string()
            .trim_start_matches("r#")
            .to_string();
        if ctx.chain.first().is_some_and(|next| *next != name) {
            return Ok(());
        }
        let remaining = if ctx.chain.is_empty() {
            ctx.chain
        } else {
            &ctx.chain[1..]
        };
        if !ctx.chain.is_empty() && remaining.is_empty() {
            *found = true;
        }
        let line = module.ident.span().start().line;
        let child_id = format!("{}::{}", ctx.module_id, name);
        let child_dir = join(&ctx.child_dir, &name);
        let mut child = Entity::new("module", child_id.clone());
        if let Some(cfg) = cfg_of(&module.attrs) {
            child = child.with("cfg", cfg);
        }
        let contains = |strength: Strength, method: &'static str| Fact {
            relationship: "module.contains",
            subject: Entity::new("module", ctx.module_id.clone()),
            object: Some(child.clone()),
            location: at(&ctx.file, line),
            provenance: Provenance {
                strength,
                method,
                source: ctx.file.clone(),
            },
        };

        if has_path_attribute(&module.attrs) {
            self.push(contains(Strength::Declared, "syn.parse_file"))?;
            self.diagnose(
                "mod_path_attribute",
                DiagnosticState::Unsupported,
                format!("module {child_id} uses #[path]; its file is not resolved"),
                at(&ctx.file, line),
            );
            return Ok(());
        }

        if let Some((_, items)) = &module.content {
            self.push(contains(Strength::Observed, "syn.parse_file"))?;
            self.observed_source_fact(
                "module.located_at",
                Entity::new("module", child_id.clone()),
                None,
                at(&ctx.file, line),
                "syn.parse_file",
                &ctx.file,
            )?;
            let inner = Ctx {
                file: ctx.file.clone(),
                module_id: child_id,
                child_dir,
                chain: remaining,
                file_mode: ctx.file_mode,
            };
            return self.walk_items(items, &inner, found);
        }

        if ctx.file_mode {
            // The declaration is in source, but nothing checked that its file exists.
            return self.push(contains(Strength::Declared, "syn.parse_file"));
        }

        let flat = join(&ctx.child_dir, &format!("{name}.rs"));
        let nested = join(&join(&ctx.child_dir, &name), "mod.rs");
        let flat_exists = self.stat(&flat)? == Some(false);
        let nested_exists = self.stat(&nested)? == Some(false);
        let resolved = match (flat_exists, nested_exists) {
            (true, false) => flat,
            (false, true) => nested,
            (false, false) => {
                self.push(contains(Strength::Declared, "syn.parse_file"))?;
                self.diagnose(
                    "module_file_missing",
                    DiagnosticState::Unresolved,
                    format!("module {child_id} is declared but neither {flat} nor {nested} exists"),
                    at(&ctx.file, line),
                );
                return Ok(());
            }
            (true, true) => {
                self.push(contains(Strength::Declared, "syn.parse_file"))?;
                self.diagnose(
                    "module_file_ambiguous",
                    DiagnosticState::Unresolved,
                    format!("module {child_id} matches both {flat} and {nested}; neither followed"),
                    at(&ctx.file, line),
                );
                return Ok(());
            }
        };
        self.push(contains(Strength::Observed, "syn.parse_file"))?;
        self.observed_source_fact(
            "module.located_at",
            Entity::new("module", child_id.clone()),
            None,
            at(&resolved, 1),
            "rust.module-file-rule",
            &ctx.file,
        )?;
        if !self.visited.insert(resolved.clone()) {
            self.diagnose(
                "module_file_revisited",
                DiagnosticState::Unresolved,
                format!(
                    "{resolved} was already observed through another declaration; not walked again"
                ),
                at(&ctx.file, line),
            );
            return Ok(());
        }
        let Some(source) = self.read_source(&resolved)? else {
            return Ok(());
        };
        let Some(file) = self.parse(&resolved, &source) else {
            return Ok(());
        };
        let inner = Ctx {
            file: resolved,
            module_id: child_id,
            child_dir,
            chain: remaining,
            file_mode: false,
        };
        self.walk_items(&file.items, &inner, found)
    }

    fn observe_target(
        &mut self,
        target: &Target,
        chain: &[String],
        found: &mut bool,
    ) -> Observed<()> {
        // The revisit guard is per crate: separate test crates may share `mod common;`.
        self.visited.clear();
        self.visited.insert(target.root_file.clone());
        let root_module = format!("{}::crate", target.crate_id);
        if chain.is_empty() {
            *found = true;
        }
        let Some(source) = self.read_source(&target.root_file)? else {
            return Ok(());
        };
        let crate_entity = Entity::new("crate", target.crate_id.clone())
            .with("package", target.package.clone())
            .with("target", target.kind);
        self.observed_source_fact(
            "crate.contains",
            crate_entity,
            Some(Entity::new("module", root_module.clone())),
            at(&target.root_file, 1),
            "cargo.target-convention",
            &target.root_file,
        )?;
        self.observed_source_fact(
            "module.located_at",
            Entity::new("module", root_module.clone()),
            None,
            at(&target.root_file, 1),
            "cargo.target-convention",
            &target.root_file,
        )?;
        let Some(file) = self.parse(&target.root_file, &source) else {
            return Ok(());
        };
        let ctx = Ctx {
            file: target.root_file.clone(),
            module_id: root_module,
            child_dir: parent(&target.root_file).to_string(),
            chain,
            file_mode: false,
        };
        self.walk_items(&file.items, &ctx, found)
    }

    /// Observes one Rust file without knowing which crate it belongs to.
    fn observe_file(&mut self, rel: &str) -> Observed<()> {
        self.exists_fact("file", rel)?;
        let Some(source) = self.read_source(rel)? else {
            return Ok(());
        };
        let Some(file) = self.parse(rel, &source) else {
            return Ok(());
        };
        let ctx = Ctx {
            file: rel.to_string(),
            module_id: rel.to_string(),
            child_dir: String::new(),
            chain: &[],
            file_mode: true,
        };
        let mut found = true;
        self.walk_items(&file.items, &ctx, &mut found)
    }

    fn package_targets(&mut self, package: &str, dir: &str) -> Observed<Vec<Target>> {
        let mut targets = Vec::new();
        let mut add = |kind: &'static str, name: Option<&str>, root_file: String| {
            let crate_id = match name {
                Some(name) if kind != "lib" => format!("{package}/{kind}/{name}"),
                _ => format!("{package}/{kind}"),
            };
            targets.push(Target {
                crate_id,
                kind,
                package: package.to_string(),
                root_file,
            });
        };
        let lib = join(dir, "src/lib.rs");
        if self.stat(&lib)? == Some(false) {
            add("lib", None, lib);
        }
        let main = join(dir, "src/main.rs");
        if self.stat(&main)? == Some(false) {
            add("bin", Some(package), main);
        }
        for (kind, folder) in [
            ("bin", "src/bin"),
            ("test", "tests"),
            ("bench", "benches"),
            ("example", "examples"),
        ] {
            let folder = join(dir, folder);
            if self.stat(&folder)? != Some(true) {
                continue;
            }
            for (name, is_dir) in self.list_dir(&folder)? {
                if is_dir {
                    let nested = join(&join(&folder, &name), "main.rs");
                    if self.stat(&nested)? == Some(false) {
                        add(kind, Some(&name), nested);
                    }
                } else if let Some(stem) = name.strip_suffix(".rs") {
                    add(kind, Some(stem), join(&folder, &name));
                }
            }
        }
        Ok(targets)
    }

    fn observe_package(
        &mut self,
        name: &str,
        manifest: &str,
        filter: &dyn Fn(&Target) -> bool,
        chain: &[String],
        found: &mut bool,
        with_artifacts: bool,
    ) -> Observed<()> {
        let dir = parent(manifest).to_string();
        if with_artifacts {
            self.exists_fact("manifest", manifest)?;
            for conventional in CONVENTIONAL_DIRS {
                let rel = join(&dir, conventional);
                if self.stat(&rel)? == Some(true) {
                    self.exists_fact("directory", &rel)?;
                }
            }
        }
        if let Some(text) = self.read_source(manifest)? {
            let mut section = String::new();
            for (index, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.starts_with('[') {
                    section = line.trim_matches(['[', ']']).trim().to_string();
                } else if matches!(
                    section.as_str(),
                    "lib" | "bin" | "test" | "bench" | "example"
                ) && line
                    .split_once('=')
                    .is_some_and(|(key, _)| key.trim() == "path")
                {
                    self.diagnose(
                        "custom_target_path",
                        DiagnosticState::Unsupported,
                        format!(
                            "{manifest} sets a custom path for [{section}]; only conventional target roots are observed"
                        ),
                        at(manifest, index + 1),
                    );
                }
            }
        }
        for target in self.package_targets(name, &dir)? {
            if filter(&target) {
                self.observe_target(&target, chain, found)?;
            }
        }
        Ok(())
    }

    fn observe_path(&mut self, prefix: &str) -> Observed<()> {
        match self.stat(prefix)? {
            None => Err(refuse(
                "scope_not_found",
                format!("path {prefix:?} does not exist under the project root"),
            )),
            Some(false) => self.observe_path_file(prefix),
            Some(true) => {
                let mut stack = vec![prefix.to_string()];
                while let Some(dir) = stack.pop() {
                    let mut subdirs = Vec::new();
                    for (name, is_dir) in self.list_dir(&dir)? {
                        let rel = join(&dir, &name);
                        if is_dir {
                            if !name.starts_with('.')
                                && !matches!(name.as_str(), "target" | "node_modules")
                            {
                                subdirs.push(rel);
                            }
                        } else {
                            self.observe_path_file(&rel)?;
                        }
                    }
                    stack.extend(subdirs.into_iter().rev());
                }
                Ok(())
            }
        }
    }

    fn observe_path_file(&mut self, rel: &str) -> Observed<()> {
        if rel.ends_with(".rs") {
            self.observe_file(rel)
        } else if matches!(
            rel.rsplit('/').next(),
            Some("Cargo.toml" | "Cargo.lock" | "package.json" | "pyproject.toml")
        ) {
            self.exists_fact("manifest", rel)
        } else {
            Ok(())
        }
    }
}

struct Declaration<'i> {
    kind: &'static str,
    name: String,
    visibility: &'static str,
    attrs: &'i [Attribute],
    line: usize,
}

fn declaration(item: &Item) -> Option<Declaration<'_>> {
    fn visibility(vis: &Visibility) -> &'static str {
        match vis {
            Visibility::Public(_) => "pub",
            Visibility::Restricted(_) => "restricted",
            Visibility::Inherited => "private",
        }
    }
    fn make<'i>(
        kind: &'static str,
        ident: &syn::Ident,
        vis: &Visibility,
        attrs: &'i [Attribute],
    ) -> Option<Declaration<'i>> {
        Some(Declaration {
            kind,
            name: ident.to_string().trim_start_matches("r#").to_string(),
            visibility: visibility(vis),
            attrs,
            line: ident.span().start().line,
        })
    }
    match item {
        Item::Fn(i) => make("fn", &i.sig.ident, &i.vis, &i.attrs),
        Item::Struct(i) => make("struct", &i.ident, &i.vis, &i.attrs),
        Item::Enum(i) => make("enum", &i.ident, &i.vis, &i.attrs),
        Item::Union(i) => make("union", &i.ident, &i.vis, &i.attrs),
        Item::Trait(i) => make("trait", &i.ident, &i.vis, &i.attrs),
        Item::Type(i) => make("type", &i.ident, &i.vis, &i.attrs),
        Item::Const(i) => make("const", &i.ident, &i.vis, &i.attrs),
        Item::Static(i) => make("static", &i.ident, &i.vis, &i.attrs),
        Item::Macro(i) if i.mac.path.is_ident("macro_rules") => {
            let ident = i.ident.as_ref()?;
            make("macro_rules", ident, &Visibility::Inherited, &i.attrs)
        }
        _ => None,
    }
}

fn path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// The attribute path (e.g. `test`, `tokio::test`) when the last segment is `test`.
/// A syntactic fact about the attribute only; it says nothing about what the test covers.
fn test_attribute(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        let path = attr.path();
        (path.segments.last()?.ident == "test").then(|| path_string(path))
    })
}

fn has_path_attribute(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("path"))
}

/// Raw `cfg(...)` tokens, reported but never evaluated.
fn cfg_of(attrs: &[Attribute]) -> Option<String> {
    let cfgs = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::List(list) => Some(list.tokens.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    (!cfgs.is_empty()).then(|| cfgs.join("; "))
}

fn existing_scope_path(root: &Path, rel: &str) -> Observed<()> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    match fs::canonicalize(root.join(rel)) {
        Ok(path) if !path.starts_with(&canonical_root) => Err(refuse(
            "invalid_scope",
            format!("scope path {rel:?} resolves outside the project root"),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(refuse(
            "scope_not_found",
            format!("path {rel:?} does not exist under the project root"),
        )),
        Err(error) => Err(refuse(
            "artifact_unreadable",
            format!("cannot access {rel:?}: {error}"),
        )),
    }
}

fn project_name(root: &Path, cargo: Option<&CargoObservation>) -> String {
    cargo
        .and_then(|cargo| {
            cargo
                .packages
                .iter()
                .find(|package| package.manifest_path == cargo.manifest)
                .map(|package| package.name.clone())
        })
        .or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown-project".to_string())
}

fn observe_project(obs: &mut Obs, cargo: Option<&CargoObservation>) -> Observed<()> {
    let Some(cargo) = cargo else {
        let mut recognized = false;
        for (manifest, ecosystem) in NON_RUST_MANIFESTS {
            if obs.stat(manifest)? == Some(false) {
                recognized = true;
                obs.exists_fact("manifest", manifest)?;
                obs.diagnose(
                    "source_structure_unsupported",
                    DiagnosticState::Unsupported,
                    format!("{manifest} detected ({ecosystem}); source structure is observed for Rust only"),
                    at_file(manifest),
                );
            }
        }
        if !recognized {
            obs.diagnose(
                "no_supported_project",
                DiagnosticState::Unsupported,
                "no Cargo.toml, package.json, or pyproject.toml at the project root",
                at_file("."),
            );
        }
        return Ok(());
    };
    let metadata = cargo.source.starts_with("cargo metadata");
    let mut found = true;
    obs.exists_fact("manifest", &cargo.manifest)?;
    if let Some(lockfile) = &cargo.lockfile {
        obs.exists_fact("manifest", lockfile)?;
    }
    for conventional in CONVENTIONAL_DIRS {
        if obs.stat(conventional)? == Some(true) {
            obs.exists_fact("directory", conventional)?;
        }
    }
    for (manifest, ecosystem) in NON_RUST_MANIFESTS {
        if obs.stat(manifest)? == Some(false) {
            obs.exists_fact("manifest", manifest)?;
            obs.diagnose(
                "source_structure_unsupported",
                DiagnosticState::Unsupported,
                format!(
                    "{manifest} detected ({ecosystem}); source structure is observed for Rust only"
                ),
                at_file(manifest),
            );
        }
    }
    if !metadata {
        obs.diagnose(
            "cargo_metadata_unavailable",
            DiagnosticState::Unresolved,
            "cargo metadata could not run; workspace members come from a text fallback and dependencies are not reported",
            at_file(&cargo.manifest),
        );
    }
    let members = cargo
        .packages
        .iter()
        .filter(|package| package.workspace_member)
        .collect::<Vec<_>>();
    if cargo.workspace {
        for package in &members {
            obs.push(Fact {
                relationship: "workspace.member",
                subject: Entity::new("workspace", "."),
                object: Some(Entity::new("package", package.name.clone())),
                location: at_file(&package.manifest_path),
                provenance: Provenance {
                    strength: if metadata {
                        Strength::Resolved
                    } else {
                        Strength::Declared
                    },
                    method: if metadata {
                        "cargo.metadata"
                    } else {
                        "cargo.toml.text"
                    },
                    source: if metadata {
                        CARGO_METADATA.to_string()
                    } else {
                        cargo.manifest.clone()
                    },
                },
            })?;
        }
    }
    // Aggregated across workspace members by existing PAX detection; not attributable
    // to a single package. "Declared" is all `cargo metadata --no-deps` can say.
    for dependency in &cargo.dependencies {
        obs.push(Fact {
            relationship: "dependency.declared",
            subject: Entity::new("workspace", "."),
            object: Some(
                Entity::new("dependency", dependency.name.clone())
                    .with("specifier", dependency.specifier.clone())
                    .with("kind", dependency.kind.clone()),
            ),
            location: None,
            provenance: Provenance {
                strength: Strength::Declared,
                method: "cargo.metadata",
                source: CARGO_METADATA.to_string(),
            },
        })?;
    }
    for package in members {
        obs.observe_package(
            &package.name,
            &package.manifest_path,
            &|_| true,
            &[],
            &mut found,
            package.manifest_path != cargo.manifest,
        )?;
    }
    Ok(())
}

fn run_scope(
    obs: &mut Obs,
    root: &Path,
    scope: &Scope,
    cargo_out: &mut Option<CargoObservation>,
) -> Observed<()> {
    match scope {
        Scope::Project => {
            let cargo = detect_cargo(root);
            let result = observe_project(obs, cargo.as_ref());
            *cargo_out = cargo;
            result
        }
        Scope::Crate(_) | Scope::Module { .. } => {
            let (crate_id, chain): (&str, &[String]) = match scope {
                Scope::Crate(id) => (id, &[]),
                Scope::Module { crate_id, chain } => (crate_id, chain),
                _ => unreachable!(),
            };
            let cargo = detect_cargo(root).ok_or_else(|| {
                refuse(
                    "unsupported_project",
                    "crate and module scopes require a Cargo project (no Cargo.toml at the project root)",
                )
            })?;
            let package_name = crate_id.split('/').next().unwrap_or(crate_id);
            let package = cargo
                .packages
                .iter()
                .find(|package| package.workspace_member && package.name == package_name)
                .ok_or_else(|| {
                    refuse(
                        "scope_not_found",
                        format!("no workspace package named {package_name:?}"),
                    )
                })?;
            if cargo.source.starts_with("cargo metadata") {
                // Nothing to add: provenance for member facts is carried per fact.
            } else {
                obs.diagnose(
                    "cargo_metadata_unavailable",
                    DiagnosticState::Unresolved,
                    "cargo metadata could not run; package location comes from a text fallback",
                    at_file(&cargo.manifest),
                );
            }
            let exact = crate_id.contains('/');
            let mut found = false;
            let wanted = crate_id.to_string();
            let manifest = package.manifest_path.clone();
            let name = package.name.clone();
            let is_module = matches!(scope, Scope::Module { .. });
            obs.observe_package(
                &name,
                &manifest,
                &|target| !exact || target.crate_id == wanted,
                chain,
                &mut found,
                !is_module,
            )?;
            if !found {
                return Err(refuse(
                    "scope_not_found",
                    if is_module {
                        format!(
                            "module {crate_id}::crate::{} was not found",
                            chain.join("::")
                        )
                    } else {
                        format!(
                            "no crate target {crate_id:?} was found by Cargo's target convention"
                        )
                    },
                ));
            }
            *cargo_out = Some(cargo);
            Ok(())
        }
        Scope::File(rel) => {
            existing_scope_path(root, rel)?;
            if !rel.ends_with(".rs") {
                return Err(refuse(
                    "unsupported_scope",
                    format!("file scope supports Rust (.rs) files only; got {rel:?}"),
                ));
            }
            if obs.stat(rel)? != Some(false) {
                return Err(refuse(
                    "invalid_scope",
                    format!("{rel:?} is not a file; use path:{rel} for a directory"),
                ));
            }
            obs.observe_file(rel)
        }
        Scope::Path(rel) => {
            if !rel.is_empty() {
                existing_scope_path(root, rel)?;
            }
            obs.observe_path(rel)
        }
    }
}

pub fn observe(root: &Path, options: &ObserveOptions, json: bool) -> Result<String, CliError> {
    let mut obs_slot: Option<Obs> = None;
    let result = (|| -> Observed<Observation> {
        let limits = parse_limits(options)?;
        let scope = parse_scope(options.scope.as_deref())?;
        let obs = obs_slot.insert(Obs::new(root, limits));
        let mut cargo = None;
        run_scope(obs, root, &scope, &mut cargo)?;
        let partial = !obs.diagnostics.is_empty();
        Ok(Observation {
            schema: SCHEMA,
            status: if partial { "partial" } else { "ok" },
            project: ProjectOutput {
                root: root.display().to_string(),
                name: project_name(root, cargo.as_ref()),
            },
            scope: scope.output(options.scope.as_deref()),
            observed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            tool: ToolOutput {
                name: "pax",
                version: env!("CARGO_PKG_VERSION"),
                parser: PARSER,
            },
            limits,
            cost: obs.cost(),
            facts: std::mem::take(&mut obs.facts),
            diagnostics: std::mem::take(&mut obs.diagnostics),
        })
    })();
    match result {
        Ok(observation) => {
            if json {
                serde_json::to_string_pretty(&observation).map_err(|error| CliError {
                    message: format!("failed to serialize output: {error}"),
                    exit_code: 1,
                })
            } else {
                Ok(render(&observation))
            }
        }
        Err(mut error) => {
            if error.cost.is_none() {
                error.cost = obs_slot.as_ref().map(Obs::cost);
            }
            let message = if json {
                serde_json::to_string_pretty(&error).unwrap_or_else(|_| error.message.clone())
            } else {
                format!("error[{}]: {}", error.code, error.message)
            };
            Err(CliError {
                message,
                exit_code: 2,
            })
        }
    }
}

fn describe(entity: &Entity) -> String {
    format!("{} {}", entity.kind, entity.id)
}

fn render(observation: &Observation) -> String {
    let mut lines = vec![format!(
        "{} {} scope={}{} status={}",
        observation.schema,
        observation.project.name,
        observation.scope.kind,
        observation
            .scope
            .value
            .as_ref()
            .map_or(String::new(), |value| format!(":{value}")),
        observation.status
    )];
    for fact in &observation.facts {
        let mut line = format!("{}  {}", fact.relationship, describe(&fact.subject));
        if let Some(object) = &fact.object {
            line.push_str(&format!(" -> {}", describe(object)));
        }
        if !fact.subject.attributes.is_empty() {
            line.push_str(&format!(" {:?}", fact.subject.attributes));
        }
        if let Some(object) = fact
            .object
            .as_ref()
            .filter(|object| !object.attributes.is_empty())
        {
            line.push_str(&format!(" {:?}", object.attributes));
        }
        if let Some(location) = &fact.location {
            line.push_str(&format!(" @ {}", location.path));
            if let Some(number) = location.line {
                line.push_str(&format!(":{number}"));
            }
        }
        line.push_str(&format!(
            " [{}/{}]",
            fact.provenance.strength.as_str(),
            fact.provenance.method
        ));
        lines.push(line);
    }
    for diagnostic in &observation.diagnostics {
        lines.push(format!(
            "{:?} {}: {}",
            diagnostic.state, diagnostic.code, diagnostic.message
        ));
    }
    lines.push(format!(
        "cost: {} facts, {} files read ({} bytes), {} entries listed, {} ms",
        observation.cost.facts,
        observation.cost.files_inspected,
        observation.cost.bytes_read,
        observation.cost.entries_listed,
        observation.cost.elapsed_ms
    ));
    lines.join("\n")
}
