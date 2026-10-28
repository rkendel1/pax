use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod contract;
mod execution_result;
mod lockfile;
mod observe;
use contract::{LEGACY_SCHEMA_VERSION, Outcome, inspection_schema, project_relative_path};
use execution_result::execute_with_result;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
enum Ecosystem {
    JavaScript,
    Python,
    Rust,
    Container,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Component {
    path: String,
    ecosystem: Ecosystem,
    tool: Option<String>,
    manifests: Vec<String>,
    lockfiles: Vec<String>,
    evidence: Vec<EvidenceItem>,
    workspace_packages: Vec<String>,
    dependency_sources: Vec<DependencySource>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DependencySource {
    kind: String,
    path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceItem {
    kind: String,
    path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeDependency {
    name: String,
    specifier: String,
    kind: String,
    ecosystem: Ecosystem,
    native_kind: String,
    /// Project-relative manifest or requirements file that declared this dependency.
    source: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CargoPackage {
    name: String,
    manifest_path: String,
    workspace_member: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CargoObservation {
    manifest: String,
    lockfile: Option<String>,
    workspace: bool,
    workspace_root: String,
    workspace_members: Vec<String>,
    packages: Vec<CargoPackage>,
    dependencies: Vec<NativeDependency>,
    /// Declared dependencies per package, keyed by manifest path. Not serialized, so
    /// existing `info`/`graph` JSON is unchanged. Empty without `cargo metadata`.
    #[serde(skip)]
    package_dependencies: BTreeMap<String, Vec<NativeDependency>>,
    source: String,
}

#[derive(Clone, Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ContainerInfo {
    dockerfiles: Vec<String>,
    compose_files: Vec<String>,
    services: Vec<String>,
    images: Vec<String>,
    directives: BTreeMap<String, Vec<String>>,
}

#[derive(Debug)]
pub struct CliError {
    pub message: String,
    pub exit_code: u8,
}

#[derive(Clone, Copy, Debug)]
enum CommandName {
    Build,
    Test,
    Lint,
    Typecheck,
    Run,
    X,
    Install,
    Add,
    Remove,
    Exec,
    Deploy,
    Info,
    Doctor,
    Deps,
    Scripts,
    Workspaces,
    Lock,
    Graph,
    Reality,
    Drift,
    Observe,
}

#[derive(Clone, Debug)]
enum Operation {
    Build,
    Test,
    Lint,
    Typecheck,
    Install,
    Add,
    Remove,
    Deploy,
    Custom(String),
}

impl Operation {
    fn name(&self) -> String {
        match self {
            Self::Build => "build".to_string(),
            Self::Test => "test".to_string(),
            Self::Lint => "lint".to_string(),
            Self::Typecheck => "typecheck".to_string(),
            Self::Install => "install".to_string(),
            Self::Add => "add".to_string(),
            Self::Remove => "remove".to_string(),
            Self::Deploy => "deploy".to_string(),
            Self::Custom(name) => format!("custom:{name}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
enum PackageManager {
    Npm,
    Pnpm,
    Bun,
    Yarn,
}

impl PackageManager {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "bun" => Some(Self::Bun),
            "yarn" => Some(Self::Yarn),
            _ => None,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Yarn => "yarn",
        }
    }

    fn expected_lockfiles(self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["package-lock.json"],
            Self::Pnpm => &["pnpm-lock.yaml"],
            Self::Bun => &["bun.lock", "bun.lockb"],
            Self::Yarn => &["yarn.lock"],
        }
    }
}

impl fmt::Display for PackageManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[derive(Clone, Debug)]
struct PackageJsonData {
    name: Option<String>,
    package_manager_field: Option<String>,
    has_workspaces: bool,
    dependencies: BTreeMap<String, String>,
    dev_dependencies: BTreeMap<String, String>,
    optional_dependencies: BTreeMap<String, String>,
    peer_dependencies: BTreeMap<String, String>,
    scripts: BTreeMap<String, String>,
    workspace_packages: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandOutput {
    schema: &'static str,
    schema_version: &'static str,
    outcome: Outcome,
    command: &'static str,
    project: ProjectInfo,
    manager: ManagerInfo,
    result: CommandResult,
    diagnostics: Vec<Diagnostic>,
    evidence: Evidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependencies: Option<DependencyGroups>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scripts: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspaces: Option<WorkspaceOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lock: Option<LockOutput>,
    ecosystem: Option<Ecosystem>,
    components: Vec<Component>,
    native_dependencies: Vec<NativeDependency>,
    container: Option<ContainerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cargo: Option<CargoObservation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DependencyGroups {
    dependencies: BTreeMap<String, String>,
    dev_dependencies: BTreeMap<String, String>,
    optional_dependencies: BTreeMap<String, String>,
    peer_dependencies: BTreeMap<String, String>,
    native_dependencies: Vec<NativeDependency>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceOutput {
    enabled: bool,
    source: Option<String>,
    packages: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LockOutput {
    found: bool,
    files: Vec<String>,
    selected: Option<String>,
    manager: Option<PackageManager>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectInfo {
    root: String,
    name: String,
    package_json: bool,
    workspace: bool,
    workspace_source: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagerInfo {
    name: Option<String>,
    version: Option<String>,
    lockfile: Option<String>,
    selected_by: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandResult {
    summary: String,
    runtime: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GraphNode {
    id: String,
    kind: String,
    ecosystem: Ecosystem,
}

#[derive(Clone, Debug, Serialize)]
struct GraphEdge {
    from: String,
    to: String,
    kind: String,
}

#[derive(Clone, Debug, Serialize)]
struct GraphEvidence {
    edge: String,
    evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GraphOutput {
    schema: &'static str,
    schema_version: &'static str,
    outcome: Outcome,
    command: &'static str,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    evidence: Vec<GraphEvidence>,
    cargo: Option<CargoObservation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealityObservation {
    subject: String,
    status: String,
    source: String,
    evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    /// Project-relative scope inspected for this fact. Runtime observations only.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    /// The explicit failure that prevented this fact from being established.
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct RealityLayer {
    observations: Vec<RealityObservation>,
}

/// A runtime observation layer. Static inspection leaves every optional field
/// empty and serializes only `observations`, preserving the existing shape.
#[derive(Clone, Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct RuntimeLayer {
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
    observations: Vec<RealityObservation>,
}

#[derive(Clone, Debug, Serialize)]
struct DependencyFact {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct DependencyObservation {
    name: String,
    specifier: String,
    kind: String,
    ecosystem: Ecosystem,
    component: String,
    declared: DependencyFact,
    lockfile: DependencyFact,
    installed: DependencyFact,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<DependencyFact>,
}

#[derive(Clone, Debug, Serialize)]
struct RealityOutput {
    schema: &'static str,
    schema_version: &'static str,
    outcome: Outcome,
    command: &'static str,
    live: bool,
    declared: RealityLayer,
    resolved: RealityLayer,
    installed: RealityLayer,
    runtime: RuntimeLayer,
    dependencies: Vec<DependencyObservation>,
    cargo: Option<CargoObservation>,
}

#[derive(Clone, Debug, Serialize)]
struct DriftItem {
    subject: String,
    status: String,
    expected: String,
    actual: String,
    evidence: Vec<DriftEvidence>,
}

#[derive(Clone, Debug, Serialize)]
struct DriftEvidence {
    source: String,
    kind: String,
}

#[derive(Clone, Debug, Serialize)]
struct DriftOutput {
    schema: &'static str,
    schema_version: &'static str,
    outcome: Outcome,
    command: &'static str,
    live: bool,
    status: String,
    issues: Vec<DriftItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<RuntimeLayer>,
    cargo: Option<CargoObservation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostic {
    level: DiagnosticLevel,
    check: &'static str,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum DiagnosticLevel {
    Ok,
    Warn,
    Error,
}

impl DiagnosticLevel {
    fn symbol(&self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "⚠",
            Self::Error => "✗",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    package_manager_field: Option<String>,
    lockfiles: Vec<String>,
    workspace_files: Vec<String>,
    selection_notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct RepositoryDetection {
    root: PathBuf,
    project_name: String,
    package_json: bool,
    package_json_data: Option<PackageJsonData>,
    workspace: bool,
    workspace_source: Option<String>,
    manager: Option<DetectedManager>,
    lockfiles: Vec<String>,
    workspace_files: Vec<String>,
    workspace_packages: Vec<String>,
    selection_notes: Vec<String>,
    components: Vec<Component>,
    native_dependencies: Vec<NativeDependency>,
    container: Option<ContainerInfo>,
    cargo: Option<CargoObservation>,
    /// Problems found while reading manifests. They are reported alongside the
    /// built-in JavaScript/Cargo diagnostics.
    detection_diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
struct DetectedManager {
    name: PackageManager,
    version: Option<String>,
    lockfile: Option<String>,
    selected_by: String,
}

#[derive(Clone, Debug)]
struct RunCommand {
    program: String,
    args: Vec<String>,
    working_directory: PathBuf,
}

#[derive(Clone, Debug)]
struct ResolvedOperation {
    operation: Operation,
    command: RunCommand,
    selection_reason: String,
    ecosystem: Option<String>,
    evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ExecutionPlan {
    schema: &'static str,
    operation: String,
    ecosystem: String,
    tool: String,
    runner: String,
    command: Vec<String>,
    evidence: Vec<String>,
    working_directory: String,
    project_root: String,
    workspace: Option<String>,
    environment: BTreeMap<String, String>,
    supported: bool,
    selection_reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeployProvider {
    Fly,
    Vercel,
    Netlify,
}

impl DeployProvider {
    fn name(self) -> &'static str {
        match self {
            Self::Fly => "fly",
            Self::Vercel => "vercel",
            Self::Netlify => "netlify",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "fly" | "flyctl" => Some(Self::Fly),
            "vercel" => Some(Self::Vercel),
            "netlify" => Some(Self::Netlify),
            _ => None,
        }
    }

    fn command(self) -> (&'static str, &'static str) {
        match self {
            Self::Fly => ("fly", "deploy"),
            Self::Vercel => ("vercel", "deploy"),
            Self::Netlify => ("netlify", "deploy"),
        }
    }
}

#[derive(Clone, Debug)]
struct DeploySelection {
    provider: DeployProvider,
    evidence: Vec<String>,
}

pub fn run<I, S>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let cli = parse_args(args)?;
    if cli.live && !matches!(cli.command, CommandName::Reality | CommandName::Drift) {
        return Err(CliError {
            message: "--live applies only to `pax reality` and `pax drift`".to_string(),
            exit_code: 2,
        });
    }
    if cli.json && !cli.dry_run && matches!(cli.command, CommandName::Test) {
        return execute_with_result(&cli, Operation::Test);
    }
    let working_directory = resolve_working_directory(&cli)?;

    if let CommandName::Observe = cli.command {
        return observe::observe(&working_directory, &cli.observe, cli.json);
    }
    if !cli.observe.is_empty() {
        return Err(CliError {
            message:
                "--scope, --max-files, --max-bytes, and --max-facts apply only to `pax observe`"
                    .to_string(),
            exit_code: 2,
        });
    }

    if let CommandName::Exec = cli.command {
        let (program, args) = cli.run_args.split_first().ok_or_else(|| CliError {
            message: format!("pax exec requires a command\n\n{}", usage()),
            exit_code: 2,
        })?;
        return dispatch_execution(
            RunCommand {
                program: program.clone(),
                args: args.to_vec(),
                working_directory,
            },
            cli.dry_run,
            cli.json,
        );
    }
    let detection = detect_repository(&working_directory)?;
    if matches!(
        cli.command,
        CommandName::Graph | CommandName::Reality | CommandName::Drift
    ) {
        return dispatch_observation(&detection, cli.command, cli.live, cli.json);
    }
    if let Some(message) = selection_ambiguity(&cli, &detection) {
        return Err(CliError {
            message,
            exit_code: 2,
        });
    }
    let operation = match cli.command {
        CommandName::Build => Some(Operation::Build),
        CommandName::Test => Some(Operation::Test),
        CommandName::Lint => Some(Operation::Lint),
        CommandName::Typecheck => Some(Operation::Typecheck),
        CommandName::Run => Some(Operation::Custom(cli.run_args[0].clone())),
        CommandName::Install => Some(Operation::Install),
        CommandName::Add => Some(Operation::Add),
        CommandName::Remove => Some(Operation::Remove),
        CommandName::Deploy => Some(Operation::Deploy),
        _ => None,
    };
    if let Some(operation) = operation {
        let operation_args = if matches!(cli.command, CommandName::Run) {
            &cli.run_args[1..]
        } else {
            &cli.run_args[..]
        };
        let resolved =
            resolve_operation(&detection, operation, operation_args, cli.tool.as_deref())?;
        return dispatch_resolved_operation(resolved, &detection, cli.dry_run, cli.json);
    }
    if let CommandName::X = cli.command {
        let command = build_x_command(&detection, &cli.run_args, cli.tool.as_deref())?;
        return dispatch_execution(command, cli.dry_run, cli.json);
    }
    let output = match cli.command {
        CommandName::Build => unreachable!(),
        CommandName::Test => unreachable!(),
        CommandName::Lint => unreachable!(),
        CommandName::Typecheck => unreachable!(),
        CommandName::Run => unreachable!(),
        CommandName::X => unreachable!(),
        CommandName::Install => unreachable!(),
        CommandName::Add => unreachable!(),
        CommandName::Remove => unreachable!(),
        CommandName::Exec => unreachable!(),
        CommandName::Deploy => unreachable!(),
        CommandName::Info => build_output("info", detection, false),
        CommandName::Doctor => build_output("doctor", detection, true),
        CommandName::Deps => build_output("deps", detection, false),
        CommandName::Scripts => build_output("scripts", detection, false),
        CommandName::Workspaces => build_output("workspaces", detection, false),
        CommandName::Lock => build_output("lock", detection, false),
        CommandName::Graph | CommandName::Reality | CommandName::Drift | CommandName::Observe => {
            unreachable!()
        }
    };

    if cli.json {
        serde_json::to_string_pretty(&output).map_err(|error| CliError {
            message: format!("failed to serialize output: {error}"),
            exit_code: 1,
        })
    } else {
        Ok(render_human(&output))
    }
}

fn resolve_working_directory(cli: &ParsedCli) -> Result<PathBuf, CliError> {
    let cwd = env::current_dir().map_err(|error| CliError {
        message: format!("failed to determine current directory: {error}"),
        exit_code: 1,
    })?;
    Ok(cli
        .dir
        .as_deref()
        .map(|dir| {
            let path = if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                cwd.join(dir)
            };
            fs::canonicalize(&path).map_err(|error| CliError {
                message: format!("invalid project directory {}: {error}", path.display()),
                exit_code: 2,
            })
        })
        .transpose()?
        .unwrap_or(cwd))
}

/// Returns the fail-closed ambiguity message when tool selection is not deterministic.
fn selection_ambiguity(cli: &ParsedCli, detection: &RepositoryDetection) -> Option<String> {
    let selects_tool = cli.tool.is_none()
        && matches!(
            cli.command,
            CommandName::Build
                | CommandName::Test
                | CommandName::Lint
                | CommandName::Typecheck
                | CommandName::Run
                | CommandName::X
                | CommandName::Install
                | CommandName::Add
                | CommandName::Remove
        );
    let ambiguous_python = detection
        .components
        .iter()
        .any(|component| component.ecosystem == Ecosystem::Python && component.lockfiles.len() > 1);
    let ambiguous_javascript = detection.package_json
        && detection
            .package_json_data
            .as_ref()
            .and_then(|data| data.package_manager_field.as_ref())
            .is_none()
        && detection
            .lockfiles
            .iter()
            .filter_map(|lockfile| manager_from_lockfile(lockfile))
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1;
    if !(selects_tool && (ambiguous_javascript || ambiguous_python)) {
        return None;
    }
    Some(if ambiguous_python {
        "ambiguous Python toolchain: multiple lockfiles detected; use --tool uv, --tool poetry, --tool pdm, or --tool pip".to_string()
    } else {
        format!(
            "multiple JavaScript package managers detected: {}\nuse --tool npm, --tool pnpm, --tool yarn, or --tool bun",
            detection.lockfiles.join(", ")
        )
    })
}

fn dispatch_observation(
    detection: &RepositoryDetection,
    command: CommandName,
    live: bool,
    json: bool,
) -> Result<String, CliError> {
    match command {
        CommandName::Graph => {
            let output = build_graph(detection);
            if json {
                serde_json::to_string_pretty(&output).map_err(|error| CliError {
                    message: error.to_string(),
                    exit_code: 1,
                })
            } else {
                Ok(render_graph(&output))
            }
        }
        CommandName::Reality => {
            let output = build_reality(detection, live);
            if json {
                serde_json::to_string_pretty(&output).map_err(|error| CliError {
                    message: error.to_string(),
                    exit_code: 1,
                })
            } else {
                Ok(render_reality(&output))
            }
        }
        CommandName::Drift => {
            let output = build_drift(detection, live);
            if json {
                serde_json::to_string_pretty(&output).map_err(|error| CliError {
                    message: error.to_string(),
                    exit_code: 1,
                })
            } else {
                Ok(render_drift(&output))
            }
        }
        _ => unreachable!(),
    }
}

/// Returns the component whose directory owns a project-relative dependency
/// source, using the most specific (longest) matching component path.
fn dependency_owner<'a>(
    source: &str,
    components: &'a [Component],
    ecosystem: Ecosystem,
) -> Option<&'a Component> {
    components
        .iter()
        .filter(|component| component.ecosystem == ecosystem)
        .filter(|component| {
            if component.path == "." {
                true
            } else {
                source
                    .strip_prefix(&component.path)
                    .is_some_and(|rest| rest.starts_with('/'))
            }
        })
        .max_by_key(|component| component.path.len())
}

fn build_graph(detection: &RepositoryDetection) -> GraphOutput {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut evidence = Vec::new();
    if let Some(data) = &detection.package_json_data {
        for (dependencies, kind) in [
            (&data.dependencies, "runtime-dependency"),
            (&data.dev_dependencies, "development-dependency"),
            (&data.optional_dependencies, "optional-dependency"),
            (&data.peer_dependencies, "peer-dependency"),
        ] {
            for name in dependencies.keys() {
                nodes.push(GraphNode {
                    id: name.clone(),
                    kind: "dependency".to_string(),
                    ecosystem: Ecosystem::JavaScript,
                });
                edges.push(GraphEdge {
                    from: ".".to_string(),
                    to: name.clone(),
                    kind: kind.to_string(),
                });
                evidence.push(GraphEvidence {
                    edge: format!(". -> {name}"),
                    evidence: vec!["package.json".to_string()],
                });
            }
        }
    }
    for component in &detection.components {
        nodes.push(GraphNode {
            id: component.path.clone(),
            kind: "component".to_string(),
            ecosystem: component.ecosystem,
        });
        // Cargo metadata knows which package declares what; the ecosystem-wide union
        // must not be attributed to every member.
        let manifest_path = if component.path == "." {
            "Cargo.toml".to_string()
        } else {
            format!("{}/Cargo.toml", component.path)
        };
        let per_package = detection
            .cargo
            .as_ref()
            .filter(|_| component.ecosystem == Ecosystem::Rust)
            .filter(|cargo| !cargo.package_dependencies.is_empty())
            .map(|cargo| {
                cargo
                    .package_dependencies
                    .get(&manifest_path)
                    .map_or(&[][..], Vec::as_slice)
            });
        let owned_dependencies = detection
            .native_dependencies
            .iter()
            .filter(|dependency| dependency.ecosystem == component.ecosystem)
            .filter(|dependency| {
                dependency_owner(
                    &dependency.source,
                    &detection.components,
                    dependency.ecosystem,
                )
                .is_some_and(|owner| owner.path == component.path)
            })
            .collect::<Vec<_>>();
        for dependency in per_package.map_or(owned_dependencies, |dependencies| {
            dependencies.iter().collect()
        }) {
            let id = dependency.name.clone();
            nodes.push(GraphNode {
                id: id.clone(),
                kind: "dependency".to_string(),
                ecosystem: dependency.ecosystem,
            });
            let kind = match dependency.kind.as_str() {
                "development" | "dev" => "development-dependency",
                "build" => "build-dependency",
                "optional" => "optional-dependency",
                "peer" => "peer-dependency",
                _ => "runtime-dependency",
            }
            .to_string();
            edges.push(GraphEdge {
                from: component.path.clone(),
                to: id.clone(),
                kind: kind.clone(),
            });
            evidence.push(GraphEvidence {
                edge: format!("{} -> {}", component.path, id),
                evidence: vec![dependency.source.clone()],
            });
        }
    }
    if let Some(cargo) = &detection.cargo
        && cargo.workspace
    {
        for member in &cargo.workspace_members {
            nodes.push(GraphNode {
                id: member.clone(),
                kind: "workspace-member".to_string(),
                ecosystem: Ecosystem::Rust,
            });
            edges.push(GraphEdge {
                from: cargo.workspace_root.clone(),
                to: member.clone(),
                kind: "workspace-member".to_string(),
            });
            evidence.push(GraphEvidence {
                edge: format!("{} -> {member}", cargo.workspace_root),
                evidence: vec![cargo.manifest.clone()],
            });
        }
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id).then(a.kind.cmp(&b.kind)));
    nodes.dedup_by(|a, b| a.id == b.id && a.kind == b.kind);
    edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then(a.to.cmp(&b.to))
            .then(a.kind.cmp(&b.kind))
    });
    edges.dedup_by(|a, b| a.from == b.from && a.to == b.to && a.kind == b.kind);
    evidence.sort_by(|a, b| a.edge.cmp(&b.edge));
    GraphOutput {
        schema: contract::GRAPH_SCHEMA,
        schema_version: LEGACY_SCHEMA_VERSION,
        outcome: if nodes.is_empty() {
            Outcome::Unsupported
        } else {
            Outcome::Ok
        },
        command: "graph",
        nodes,
        edges,
        evidence,
        cargo: detection.cargo.clone(),
    }
}

/// Returns the project-relative `node_modules/` location visible to a
/// JavaScript component, walking ancestors so hoisted workspace installs are
/// recognized.
fn javascript_node_modules(
    detection: &RepositoryDetection,
    component: &Component,
) -> Option<String> {
    let mut dir = if component.path == "." {
        detection.root.clone()
    } else {
        detection.root.join(&component.path)
    };
    loop {
        if dir.join("node_modules").is_dir() {
            let relative = dir
                .strip_prefix(&detection.root)
                .ok()
                .and_then(Path::to_str)
                .filter(|relative| !relative.is_empty());
            return Some(match relative {
                Some(relative) => format!("{relative}/node_modules/"),
                None => "node_modules/".to_string(),
            });
        }
        if dir == detection.root {
            return None;
        }
        let parent = dir.parent()?.to_path_buf();
        if !parent.starts_with(&detection.root) {
            return None;
        }
        dir = parent;
    }
}

fn component_directory(detection: &RepositoryDetection, component: &Component) -> PathBuf {
    if component.path == "." {
        detection.root.clone()
    } else {
        detection.root.join(&component.path)
    }
}

fn component_file(component: &Component, file: &str) -> String {
    join_project_path(&component.path, file)
}

/// Returns the lockfiles that govern a component, walking ancestors for
/// workspace ecosystems so a nested package uses the workspace-root lockfile.
fn effective_component_lockfiles(
    detection: &RepositoryDetection,
    component: &Component,
) -> Vec<String> {
    let names: &[&str] = match component.ecosystem {
        Ecosystem::JavaScript => &JAVASCRIPT_LOCKFILES,
        Ecosystem::Rust => &["Cargo.lock"],
        _ => return component.lockfiles.clone(),
    };
    let mut directory = component_directory(detection, component);
    loop {
        let files = names
            .iter()
            .filter(|name| directory.join(name).is_file())
            .map(|name| {
                directory
                    .strip_prefix(&detection.root)
                    .ok()
                    .and_then(Path::to_str)
                    .filter(|relative| !relative.is_empty())
                    .map_or_else(
                        || (*name).to_string(),
                        |relative| format!("{relative}/{name}"),
                    )
            })
            .collect::<Vec<_>>();
        if !files.is_empty() {
            return files;
        }
        if directory == detection.root {
            return Vec::new();
        }
        let Some(parent) = directory.parent() else {
            return Vec::new();
        };
        if !parent.starts_with(&detection.root) {
            return Vec::new();
        }
        directory = parent.to_path_buf();
    }
}

#[derive(Clone, Debug)]
struct LockfileSelection {
    files: Vec<String>,
    selected: Option<(String, lockfile::Observation)>,
    ambiguous: bool,
    detail: Option<String>,
}

impl LockfileSelection {
    fn dependency_fact(&self, ecosystem: Ecosystem, name: &str) -> DependencyFact {
        if let Some((source, observation)) = &self.selected {
            let present = lockfile_contains(ecosystem, &observation.packages, name);
            return DependencyFact {
                status: if present { "present" } else { "absent" }.to_string(),
                source: Some(source.clone()),
                evidence: vec![source.clone()],
                detail: None,
            };
        }
        if self.files.is_empty() {
            return DependencyFact {
                status: "absent".to_string(),
                source: None,
                evidence: Vec::new(),
                detail: Some("no lockfile recorded".to_string()),
            };
        }
        DependencyFact {
            status: "unknown".to_string(),
            source: self.files.first().cloned(),
            evidence: self.files.clone(),
            detail: self.detail.clone(),
        }
    }
}

fn select_component_lockfile(
    detection: &RepositoryDetection,
    component: &Component,
) -> LockfileSelection {
    let files = effective_component_lockfiles(detection, component);
    if files.is_empty() {
        return LockfileSelection {
            files,
            selected: None,
            ambiguous: false,
            detail: None,
        };
    }
    if component.ecosystem == Ecosystem::JavaScript {
        let managers = files
            .iter()
            .filter_map(|file| manager_from_lockfile(file))
            .collect::<std::collections::BTreeSet<_>>();
        if managers.len() > 1 {
            return LockfileSelection {
                files,
                selected: None,
                ambiguous: true,
                detail: Some("multiple package-manager lockfiles".to_string()),
            };
        }
    }
    if component.ecosystem == Ecosystem::Python && files.len() > 1 {
        return LockfileSelection {
            files,
            selected: None,
            ambiguous: true,
            detail: Some("multiple Python lockfiles".to_string()),
        };
    }
    let observations = files
        .iter()
        .map(|file| {
            (
                file.clone(),
                lockfile::observe(&detection.root.join(file), file),
            )
        })
        .collect::<Vec<_>>();
    let established = observations
        .iter()
        .filter(|(_, observation)| observation.established())
        .collect::<Vec<_>>();
    if established.is_empty() {
        let detail = observations
            .iter()
            .filter_map(|(_, observation)| observation.detail.clone())
            .collect::<Vec<_>>();
        return LockfileSelection {
            files,
            selected: None,
            ambiguous: false,
            detail: Some(if detail.is_empty() {
                "lockfile dependency set not established".to_string()
            } else {
                detail.join("; ")
            }),
        };
    }
    let first = &established[0].1.packages;
    if established
        .iter()
        .all(|(_, observation)| observation.packages == *first)
    {
        let (source, observation) = established[0];
        return LockfileSelection {
            files,
            selected: Some((source.clone(), observation.clone())),
            ambiguous: false,
            detail: None,
        };
    }
    LockfileSelection {
        files,
        selected: None,
        ambiguous: true,
        detail: Some("lockfiles record different dependency sets".to_string()),
    }
}

/// Normalizes a Python distribution name per PEP 503: lowercase, with runs of
/// `-`, `_`, and `.` collapsed to a single hyphen.
fn normalize_python_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            separator = true;
            continue;
        }
        if separator && !normalized.is_empty() {
            normalized.push('-');
        }
        separator = false;
        normalized.push(character.to_ascii_lowercase());
    }
    normalized
}

fn lockfile_contains(
    ecosystem: Ecosystem,
    packages: &std::collections::BTreeSet<String>,
    name: &str,
) -> bool {
    if ecosystem == Ecosystem::Python {
        let name = normalize_python_name(name);
        packages
            .iter()
            .any(|package| normalize_python_name(package) == name)
    } else {
        packages.contains(name)
    }
}

fn declared_dependencies(
    detection: &RepositoryDetection,
    component: &Component,
) -> Vec<NativeDependency> {
    let mut dependencies = detection
        .native_dependencies
        .iter()
        .filter(|dependency| dependency.ecosystem == component.ecosystem)
        .filter(|dependency| {
            dependency_owner(
                &dependency.source,
                &detection.components,
                dependency.ecosystem,
            )
            .is_some_and(|owner| owner.path == component.path)
        })
        .cloned()
        .collect::<Vec<_>>();
    // Root JavaScript declarations live in `package_json_data`; nested manifests
    // were already collected by component detection.
    if component.ecosystem == Ecosystem::JavaScript
        && component.path == "."
        && let Some(data) = &detection.package_json_data
    {
        for (group, kind, native_kind) in [
            (&data.dependencies, "dependency", "runtime-dependency"),
            (
                &data.dev_dependencies,
                "development",
                "development-dependency",
            ),
            (
                &data.optional_dependencies,
                "optional",
                "optional-dependency",
            ),
            (&data.peer_dependencies, "peer", "peer-dependency"),
        ] {
            for (name, specifier) in group {
                dependencies.push(NativeDependency {
                    name: name.clone(),
                    specifier: specifier.clone(),
                    kind: kind.to_string(),
                    ecosystem: Ecosystem::JavaScript,
                    native_kind: native_kind.to_string(),
                    source: "package.json".to_string(),
                });
            }
        }
    }
    sort_dependencies(&mut dependencies);
    dependencies
}

fn ancestor_node_modules(
    detection: &RepositoryDetection,
    component: &Component,
) -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    let mut directory = component_directory(detection, component);
    loop {
        let node_modules = directory.join("node_modules");
        if node_modules.is_dir() {
            let source = directory
                .strip_prefix(&detection.root)
                .ok()
                .and_then(Path::to_str)
                .filter(|relative| !relative.is_empty())
                .map_or_else(
                    || "node_modules/".to_string(),
                    |relative| format!("{relative}/node_modules/"),
                );
            result.push((source, node_modules));
        }
        if directory == detection.root {
            break;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        if !parent.starts_with(&detection.root) {
            break;
        }
        directory = parent.to_path_buf();
    }
    result
}

fn count_node_modules_packages(directory: &Path) -> std::io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if name.starts_with('@') && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                if !child.file_name().to_string_lossy().starts_with('.') {
                    count += 1;
                }
            }
            continue;
        }
        count += 1;
    }
    Ok(count)
}

fn javascript_installed_fact(
    detection: &RepositoryDetection,
    component: &Component,
    name: &str,
) -> DependencyFact {
    let parents = ancestor_node_modules(detection, component);
    let first_source = parents.first().map(|(source, _)| source.clone());
    let mut unreadable = None;
    for (source, directory) in &parents {
        match fs::metadata(directory.join(name)) {
            Ok(metadata) if metadata.is_dir() || metadata.is_file() => {
                return DependencyFact {
                    status: "present".to_string(),
                    source: Some(format!("{source}{name}")),
                    evidence: vec![format!("{source}{name}")],
                    detail: None,
                };
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => unreadable = Some((source.clone(), error.kind())),
        }
    }
    if let Some((source, kind)) = unreadable {
        return DependencyFact {
            status: "unknown".to_string(),
            source: Some(source),
            evidence: Vec::new(),
            detail: Some(format!("installed location not readable: {kind:?}")),
        };
    }
    DependencyFact {
        status: "absent".to_string(),
        source: first_source.or_else(|| Some("node_modules/".to_string())),
        evidence: Vec::new(),
        detail: None,
    }
}

fn python_site_packages(environment: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for library in ["lib", "lib64"] {
        let library_dir = environment.join(library);
        if !library_dir.is_dir() {
            continue;
        }
        let direct = library_dir.join("site-packages");
        if direct.is_dir() {
            directories.push(direct);
        }
        if let Ok(entries) = fs::read_dir(&library_dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with("python") {
                    let site_packages = entry.path().join("site-packages");
                    if site_packages.is_dir() {
                        directories.push(site_packages);
                    }
                }
            }
        }
    }
    let windows = environment.join("Lib").join("site-packages");
    if windows.is_dir() {
        directories.push(windows);
    }
    directories.sort();
    directories.dedup();
    directories
}

fn python_site_packages_entries(
    directories: &[PathBuf],
) -> Option<std::collections::BTreeSet<String>> {
    let mut packages = std::collections::BTreeSet::new();
    let mut readable = false;
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        readable = true;
        let mut saw_metadata = false;
        let mut plain = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(base) = name
                .strip_suffix(".dist-info")
                .or_else(|| name.strip_suffix(".egg-info"))
            {
                saw_metadata = true;
                let package = distribution_name(base);
                if !package.is_empty() {
                    packages.insert(package.to_string());
                }
            } else if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !name.starts_with('.')
                && !name.starts_with('_')
                && !matches!(name.as_str(), "bin" | "include")
            {
                plain.push(name);
            }
        }
        if !saw_metadata {
            packages.extend(plain);
        }
    }
    readable.then_some(packages)
}

/// Extracts a distribution name from a `*.dist-info` or `*.egg-info` directory.
/// Version suffixes begin with a digit, and older egg metadata may append an
/// interpreter tag after the version, so scan from the right for the first
/// hyphen followed by a digit.
fn distribution_name(base: &str) -> &str {
    let base = base.strip_prefix("__editable__.").unwrap_or(base);
    for (index, character) in base.char_indices().rev() {
        if character == '-'
            && base[index + 1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_digit())
        {
            return &base[..index];
        }
    }
    base
}

enum PythonInstall {
    Established {
        packages: std::collections::BTreeSet<String>,
        source: String,
    },
    Unknown {
        source: Option<String>,
        detail: String,
    },
}

fn python_install(detection: &RepositoryDetection, component: &Component) -> PythonInstall {
    let directory = component_directory(detection, component);
    for name in [".venv", "venv", ".env"] {
        let environment = directory.join(name);
        if !environment.is_dir() {
            continue;
        }
        let source = component_file(component, &format!("{name}/"));
        let site_packages = python_site_packages(&environment);
        if site_packages.is_empty() {
            return PythonInstall::Unknown {
                source: Some(source),
                detail: "Python environment has no site-packages directory".to_string(),
            };
        }
        return match python_site_packages_entries(&site_packages) {
            Some(packages) => PythonInstall::Established { packages, source },
            None => PythonInstall::Unknown {
                source: Some(source),
                detail: "site-packages is not readable".to_string(),
            },
        };
    }
    PythonInstall::Unknown {
        source: None,
        detail: "no conventional Python environment directory".to_string(),
    }
}

fn python_installed_fact(
    detection: &RepositoryDetection,
    component: &Component,
    name: &str,
) -> DependencyFact {
    match python_install(detection, component) {
        PythonInstall::Established { packages, source } => {
            let name = normalize_python_name(name);
            let present = packages
                .iter()
                .any(|package| normalize_python_name(package) == name);
            DependencyFact {
                status: if present { "present" } else { "absent" }.to_string(),
                source: Some(source),
                evidence: Vec::new(),
                detail: None,
            }
        }
        PythonInstall::Unknown { source, detail } => DependencyFact {
            status: "unknown".to_string(),
            source,
            evidence: Vec::new(),
            detail: Some(detail),
        },
    }
}

fn installed_dependency_fact(
    detection: &RepositoryDetection,
    component: &Component,
    name: &str,
) -> DependencyFact {
    match component.ecosystem {
        Ecosystem::JavaScript => javascript_installed_fact(detection, component, name),
        Ecosystem::Python => python_installed_fact(detection, component, name),
        Ecosystem::Rust => DependencyFact {
            status: "unknown".to_string(),
            source: None,
            evidence: Vec::new(),
            detail: Some(
                "Cargo installs packages outside the project; no static installed-package observer"
                    .to_string(),
            ),
        },
        Ecosystem::Container => DependencyFact {
            status: "unknown".to_string(),
            source: None,
            evidence: Vec::new(),
            detail: Some("container dependencies are not observed statically".to_string()),
        },
    }
}

fn installed_observation(
    detection: &RepositoryDetection,
    component: &Component,
) -> (String, Vec<String>, Option<usize>, Option<String>) {
    match component.ecosystem {
        Ecosystem::JavaScript => match javascript_node_modules(detection, component) {
            Some(source) => match count_node_modules_packages(&detection.root.join(&source)) {
                Ok(0) => (
                    "absent".to_string(),
                    vec![source],
                    Some(0),
                    Some("node_modules exists but contains no packages".to_string()),
                ),
                Ok(count) => ("present".to_string(), vec![source], Some(count), None),
                Err(error) => (
                    "unknown".to_string(),
                    vec![source],
                    None,
                    Some(format!("node_modules not readable: {:?}", error.kind())),
                ),
            },
            None => (
                "absent".to_string(),
                vec!["node_modules/".to_string()],
                Some(0),
                None,
            ),
        },
        Ecosystem::Rust => (
            "unknown".to_string(),
            Vec::new(),
            None,
            Some(
                "Cargo installs packages outside the project; target/ is build output".to_string(),
            ),
        ),
        Ecosystem::Python => match python_install(detection, component) {
            PythonInstall::Established { packages, source } if packages.is_empty() => (
                "absent".to_string(),
                vec![source],
                Some(0),
                Some("site-packages contains no packages".to_string()),
            ),
            PythonInstall::Established { packages, source } => (
                "present".to_string(),
                vec![source],
                Some(packages.len()),
                None,
            ),
            PythonInstall::Unknown { source, detail } => (
                "unknown".to_string(),
                source.into_iter().collect(),
                None,
                Some(detail),
            ),
        },
        Ecosystem::Container => (
            "unknown".to_string(),
            Vec::new(),
            None,
            Some("container installation is not observed statically".to_string()),
        ),
    }
}

fn build_reality(detection: &RepositoryDetection, live: bool) -> RealityOutput {
    let declared: Vec<RealityObservation> = detection
        .components
        .iter()
        .map(|component| RealityObservation {
            subject: component.path.clone(),
            status: "present".to_string(),
            source: "filesystem".to_string(),
            evidence: component
                .manifests
                .iter()
                .map(|path| component_file(component, path))
                .collect(),
            format: None,
            package_count: None,
            detail: None,
            scope: None,
            failure: None,
        })
        .collect();
    let resolved: Vec<RealityObservation> = detection
        .components
        .iter()
        .flat_map(|component| {
            let directory = component_directory(detection, component);
            component.lockfiles.iter().map(move |path| {
                let display = component_file(component, path);
                let observation = lockfile::observe(&directory.join(path), &display);
                RealityObservation {
                    subject: component.path.clone(),
                    status: observation.status.to_string(),
                    source: "filesystem".to_string(),
                    evidence: vec![display],
                    format: observation.format.map(|format| format.name().to_string()),
                    package_count: observation
                        .established()
                        .then_some(observation.packages.len()),
                    detail: observation.detail,
                    scope: None,
                    failure: None,
                }
            })
        })
        .collect();
    let installed: Vec<RealityObservation> = detection
        .components
        .iter()
        .map(|component| {
            let (status, evidence, package_count, detail) =
                installed_observation(detection, component);
            RealityObservation {
                subject: component.path.clone(),
                status,
                source: "filesystem".to_string(),
                evidence,
                format: None,
                package_count,
                detail,
                scope: None,
                failure: None,
            }
        })
        .collect();
    let runtime = if live {
        inspect_runtime(detection)
    } else {
        RuntimeLayer::default()
    };
    let mut dependencies = Vec::new();
    for component in &detection.components {
        let lockfile = select_component_lockfile(detection, component);
        for dependency in declared_dependencies(detection, component) {
            let lockfile_fact = lockfile.dependency_fact(dependency.ecosystem, &dependency.name);
            let installed_fact = installed_dependency_fact(detection, component, &dependency.name);
            let runtime_fact = live.then(|| DependencyFact {
                status: "unknown".to_string(),
                source: Some("runtime".to_string()),
                evidence: Vec::new(),
                detail: Some("no runtime dependency observer for this ecosystem".to_string()),
            });
            dependencies.push(DependencyObservation {
                name: dependency.name.clone(),
                specifier: dependency.specifier.clone(),
                kind: dependency.kind.clone(),
                ecosystem: dependency.ecosystem,
                component: component.path.clone(),
                declared: DependencyFact {
                    status: "present".to_string(),
                    source: Some(dependency.source.clone()),
                    evidence: vec![dependency.source.clone()],
                    detail: None,
                },
                lockfile: lockfile_fact,
                installed: installed_fact,
                runtime: runtime_fact,
            });
        }
    }
    dependencies.sort_by(|a, b| {
        a.component
            .cmp(&b.component)
            .then_with(|| format!("{:?}", a.ecosystem).cmp(&format!("{:?}", b.ecosystem)))
            .then(a.name.cmp(&b.name))
            .then(a.kind.cmp(&b.kind))
            .then(a.specifier.cmp(&b.specifier))
    });
    let unsupported = detection.components.is_empty();
    let observation_undetermined = declared
        .iter()
        .chain(resolved.iter())
        .chain(installed.iter())
        .chain(runtime.observations.iter())
        .any(|observation| !matches!(observation.status.as_str(), "present" | "absent"));
    let dependency_undetermined = dependencies.iter().any(|dependency| {
        [
            Some(&dependency.declared),
            Some(&dependency.lockfile),
            Some(&dependency.installed),
        ]
        .into_iter()
        .flatten()
        .chain(dependency.runtime.iter())
        .any(|fact| !matches!(fact.status.as_str(), "present" | "absent"))
    });
    RealityOutput {
        schema: contract::REALITY_SCHEMA,
        schema_version: LEGACY_SCHEMA_VERSION,
        outcome: if unsupported {
            Outcome::Unsupported
        } else if observation_undetermined || dependency_undetermined {
            Outcome::Partial
        } else {
            Outcome::Ok
        },
        command: "reality",
        live,
        declared: RealityLayer {
            observations: declared,
        },
        resolved: RealityLayer {
            observations: resolved,
        },
        installed: RealityLayer {
            observations: installed,
        },
        runtime,
        dependencies,
        cargo: detection.cargo.clone(),
    }
}

fn drift_issue_exists(issues: &[DriftItem], subject: &str, status: &str, expected: &str) -> bool {
    issues.iter().any(|issue| {
        issue.subject == subject && issue.status == status && issue.expected == expected
    })
}

fn push_drift_issue(issues: &mut Vec<DriftItem>, issue: DriftItem) {
    if !drift_issue_exists(issues, &issue.subject, &issue.status, &issue.expected) {
        issues.push(issue);
    }
}

fn dependency_subject(component: &Component, name: &str) -> String {
    if component.path == "." {
        format!("./{name}")
    } else {
        format!("{}/{}", component.path, name)
    }
}

fn reconcile_in_lockfile(dependency: &NativeDependency) -> bool {
    match dependency.ecosystem {
        Ecosystem::JavaScript => dependency.kind != "peer",
        Ecosystem::Python => dependency.kind != "build",
        Ecosystem::Rust => true,
        Ecosystem::Container => false,
    }
}

fn build_drift(detection: &RepositoryDetection, live: bool) -> DriftOutput {
    let mut issues = Vec::new();
    let package_manager = detection
        .package_json_data
        .as_ref()
        .and_then(|data| data.package_manager_field.as_deref())
        .and_then(parse_package_manager_field)
        .map(|(manager, _)| manager);
    let js_locks = detection
        .lockfiles
        .iter()
        .filter_map(|file| manager_from_lockfile(file).map(|manager| (manager, file)))
        .collect::<Vec<_>>();
    if let Some(manager) = package_manager
        && js_locks.iter().any(|(found, _)| *found != manager)
    {
        issues.push(DriftItem {
            subject: ".".to_string(),
            status: "drift".to_string(),
            expected: manager.to_string(),
            actual: js_locks
                .iter()
                .map(|(_, file)| file.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            evidence: js_locks
                .iter()
                .map(|(_, file)| DriftEvidence {
                    source: (*file).clone(),
                    kind: "resolved".to_string(),
                })
                .chain(std::iter::once(DriftEvidence {
                    source: "package.json".to_string(),
                    kind: "declared".to_string(),
                }))
                .collect(),
        });
    }

    for component in &detection.components {
        let declared = declared_dependencies(detection, component);
        let selection = select_component_lockfile(detection, component);
        if selection.ambiguous {
            push_drift_issue(
                &mut issues,
                DriftItem {
                    subject: component.path.clone(),
                    status: "ambiguous".to_string(),
                    expected: "one dependency resolution authority".to_string(),
                    actual: selection.files.join(", "),
                    evidence: selection
                        .files
                        .iter()
                        .map(|source| DriftEvidence {
                            source: source.clone(),
                            kind: "resolved".to_string(),
                        })
                        .collect(),
                },
            );
        }

        let rust_declarations_authoritative = component.ecosystem != Ecosystem::Rust
            || detection
                .cargo
                .as_ref()
                .is_some_and(|cargo| cargo.source.starts_with("cargo metadata"));
        let lockfile_dependencies = declared
            .iter()
            .filter(|dependency| reconcile_in_lockfile(dependency))
            .collect::<Vec<_>>();
        if !lockfile_dependencies.is_empty() && rust_declarations_authoritative {
            if let Some((source, observation)) = &selection.selected {
                for dependency in &lockfile_dependencies {
                    if !lockfile_contains(
                        dependency.ecosystem,
                        &observation.packages,
                        &dependency.name,
                    ) {
                        push_drift_issue(
                            &mut issues,
                            DriftItem {
                                subject: dependency_subject(component, &dependency.name),
                                status: "drift".to_string(),
                                expected: "declared dependency recorded in resolvable lockfile"
                                    .to_string(),
                                actual: format!("{} missing from {source}", dependency.name),
                                evidence: vec![
                                    DriftEvidence {
                                        source: dependency.source.clone(),
                                        kind: "declared".to_string(),
                                    },
                                    DriftEvidence {
                                        source: source.clone(),
                                        kind: "resolved".to_string(),
                                    },
                                ],
                            },
                        );
                    }
                }
            } else if !selection.files.is_empty() && !selection.ambiguous {
                let mut evidence = selection
                    .files
                    .iter()
                    .map(|source| DriftEvidence {
                        source: source.clone(),
                        kind: "resolved".to_string(),
                    })
                    .collect::<Vec<_>>();
                for dependency in &lockfile_dependencies {
                    if !evidence.iter().any(|item| item.source == dependency.source) {
                        evidence.push(DriftEvidence {
                            source: dependency.source.clone(),
                            kind: "declared".to_string(),
                        });
                    }
                }
                push_drift_issue(
                    &mut issues,
                    DriftItem {
                        subject: component.path.clone(),
                        status: "unknown".to_string(),
                        expected: "lockfile-recorded dependencies".to_string(),
                        actual: format!("{} could not be interpreted", selection.files.join(", ")),
                        evidence,
                    },
                );
            }
        }

        if component.ecosystem == Ecosystem::JavaScript {
            let runtime = declared
                .iter()
                .filter(|dependency| dependency.kind == "dependency")
                .collect::<Vec<_>>();
            if !runtime.is_empty() {
                match javascript_node_modules(detection, component) {
                    None => issues.push(DriftItem {
                        subject: component.path.clone(),
                        status: "drift".to_string(),
                        expected: "installed dependencies".to_string(),
                        actual: "node_modules/ absent".to_string(),
                        evidence: vec![
                            DriftEvidence {
                                source: component_file(component, "package.json"),
                                kind: "declared".to_string(),
                            },
                            DriftEvidence {
                                source: "node_modules/".to_string(),
                                kind: "installed".to_string(),
                            },
                        ],
                    }),
                    Some(_) => {
                        for dependency in runtime {
                            let fact =
                                javascript_installed_fact(detection, component, &dependency.name);
                            match fact.status.as_str() {
                                "present" => {}
                                "absent" => push_drift_issue(
                                    &mut issues,
                                    DriftItem {
                                        subject: dependency_subject(component, &dependency.name),
                                        status: "drift".to_string(),
                                        expected: "declared dependency installed".to_string(),
                                        actual: format!(
                                            "{} missing from installed packages",
                                            dependency.name
                                        ),
                                        evidence: vec![
                                            DriftEvidence {
                                                source: dependency.source.clone(),
                                                kind: "declared".to_string(),
                                            },
                                            DriftEvidence {
                                                source: fact
                                                    .source
                                                    .clone()
                                                    .unwrap_or_else(|| "node_modules/".to_string()),
                                                kind: "installed".to_string(),
                                            },
                                        ],
                                    },
                                ),
                                _ => push_drift_issue(
                                    &mut issues,
                                    DriftItem {
                                        subject: dependency_subject(component, &dependency.name),
                                        status: "unknown".to_string(),
                                        expected: "installed dependencies".to_string(),
                                        actual: format!(
                                            "{} installed state not established",
                                            dependency.name
                                        ),
                                        evidence: vec![DriftEvidence {
                                            source: fact
                                                .source
                                                .clone()
                                                .unwrap_or_else(|| "node_modules/".to_string()),
                                            kind: "installed".to_string(),
                                        }],
                                    },
                                ),
                            }
                        }
                    }
                }
            }
        }

        if component.ecosystem == Ecosystem::Python
            && let PythonInstall::Established { packages, source } =
                python_install(detection, component)
        {
            for dependency in declared
                .iter()
                .filter(|dependency| dependency.kind == "dependency")
            {
                let name = normalize_python_name(&dependency.name);
                if !packages
                    .iter()
                    .any(|package| normalize_python_name(package) == name)
                {
                    push_drift_issue(
                        &mut issues,
                        DriftItem {
                            subject: dependency_subject(component, &dependency.name),
                            status: "drift".to_string(),
                            expected: "declared dependency installed".to_string(),
                            actual: format!(
                                "{} missing from {} site-packages",
                                dependency.name, source
                            ),
                            evidence: vec![
                                DriftEvidence {
                                    source: dependency.source.clone(),
                                    kind: "declared".to_string(),
                                },
                                DriftEvidence {
                                    source: source.clone(),
                                    kind: "installed".to_string(),
                                },
                            ],
                        },
                    );
                }
            }
        }
    }
    let runtime = live.then(|| inspect_runtime(detection));
    if let Some(runtime) = &runtime {
        for observation in &runtime.observations {
            if matches!(observation.status.as_str(), "present" | "absent") {
                continue;
            }
            let actual = observation
                .failure
                .clone()
                .or_else(|| observation.detail.clone())
                .unwrap_or_else(|| format!("runtime state {}", observation.status));
            push_drift_issue(
                &mut issues,
                DriftItem {
                    subject: observation.subject.clone(),
                    status: "unknown".to_string(),
                    expected: "established runtime state".to_string(),
                    actual,
                    evidence: observation
                        .evidence
                        .iter()
                        .map(|source| DriftEvidence {
                            source: source.clone(),
                            kind: "runtime".to_string(),
                        })
                        .collect(),
                },
            );
        }
    }
    issues.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.status.cmp(&b.status)));
    let unsupported = detection.components.is_empty();
    let partial = issues.iter().any(|issue| issue.status == "unknown");
    let has_drift = issues.iter().any(|issue| issue.status == "drift");
    let has_ambiguous = issues.iter().any(|issue| issue.status == "ambiguous");
    let has_unknown = issues.iter().any(|issue| issue.status == "unknown");
    DriftOutput {
        schema: contract::DRIFT_SCHEMA,
        schema_version: LEGACY_SCHEMA_VERSION,
        outcome: if unsupported {
            Outcome::Unsupported
        } else if partial {
            Outcome::Partial
        } else {
            Outcome::Ok
        },
        command: "drift",
        live,
        status: if unsupported {
            "unsupported".to_string()
        } else if has_drift {
            "drift".to_string()
        } else if has_ambiguous {
            "ambiguous".to_string()
        } else if has_unknown {
            "unknown".to_string()
        } else {
            "match".to_string()
        },
        issues,
        runtime,
        cargo: detection.cargo.clone(),
    }
}

fn is_compose_file_name(name: &str) -> bool {
    let name = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(name);
    matches!(
        name,
        "compose.yaml" | "compose.yml" | "docker-compose.yaml" | "docker-compose.yml"
    )
}

struct ComposeScope {
    component: String,
    directory: PathBuf,
    /// Compose file names relative to the component directory, usable as `-f` values.
    files: Vec<String>,
    /// Compose file paths relative to the project root, for output and evidence.
    display_files: Vec<String>,
    /// Every manifest the component was detected from, relative to the project root.
    manifests: Vec<String>,
    services: Vec<String>,
    read_failure: Option<String>,
}

fn collect_compose_scopes(detection: &RepositoryDetection) -> Vec<ComposeScope> {
    detection
        .components
        .iter()
        .filter(|component| component.ecosystem == Ecosystem::Container)
        .map(|component| {
            let directory = component_directory(detection, component);
            let manifests = component
                .manifests
                .iter()
                .map(|manifest| component_file(component, manifest))
                .collect::<Vec<_>>();
            let mut files = Vec::new();
            let mut display_files = Vec::new();
            let mut services = Vec::new();
            let mut read_failure = None;
            for manifest in &component.manifests {
                if !is_compose_file_name(manifest) {
                    continue;
                }
                files.push(manifest.clone());
                let display = component_file(component, manifest);
                display_files.push(display.clone());
                match fs::read_to_string(directory.join(manifest)) {
                    Ok(contents) => {
                        let (declared, _) = parse_compose_file(&contents);
                        for service in declared {
                            if !services.contains(&service) {
                                services.push(service);
                            }
                        }
                    }
                    Err(error) => {
                        if read_failure.is_none() {
                            read_failure = Some(format!("could not read {display}: {error}"));
                        }
                    }
                }
            }
            ComposeScope {
                component: component.path.clone(),
                directory,
                files,
                display_files,
                manifests,
                services,
                read_failure,
            }
        })
        .collect()
}

fn runtime_scope_label(scopes: &[ComposeScope]) -> String {
    let files = scopes
        .iter()
        .flat_map(|scope| scope.display_files.iter().cloned())
        .collect::<Vec<_>>();
    if files.is_empty() {
        if scopes.is_empty() {
            "no container components".to_string()
        } else {
            "container components without Compose files".to_string()
        }
    } else {
        format!("compose:{}", files.join(", "))
    }
}

fn scope_for_observation(scope: &ComposeScope) -> String {
    if scope.display_files.is_empty() {
        scope.component.clone()
    } else {
        format!("compose:{}", scope.display_files.join(", "))
    }
}

fn runtime_observation(
    subject: String,
    status: &str,
    source: &str,
    evidence: Vec<String>,
    detail: Option<String>,
    failure: Option<String>,
    scope: Option<String>,
) -> RealityObservation {
    RealityObservation {
        subject,
        status: status.to_string(),
        source: source.to_string(),
        evidence,
        format: None,
        package_count: None,
        detail,
        scope,
        failure,
    }
}

fn compose_ps_args(files: &[String]) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    for file in files {
        args.push("-f".to_string());
        args.push(file.clone());
    }
    args.push("ps".to_string());
    args.push("--format".to_string());
    args.push("json".to_string());
    args
}

fn compose_ps_command(files: &[String]) -> String {
    let mut command = "docker compose".to_string();
    for file in files {
        command.push_str(" -f ");
        command.push_str(file);
    }
    command.push_str(" ps --format json");
    command
}

#[derive(Clone, Debug)]
struct ComposeContainer {
    service: String,
    state: Option<String>,
}

impl ComposeContainer {
    fn running(&self) -> bool {
        self.state.as_deref().is_some_and(|state| {
            let state = state.trim().to_ascii_lowercase();
            state == "running" || state.starts_with("up ")
        })
    }

    fn state_label(&self) -> String {
        self.state
            .as_deref()
            .map(str::trim)
            .filter(|state| !state.is_empty())
            .unwrap_or("state not reported")
            .to_string()
    }
}

fn parse_compose_ps(contents: &str) -> Result<Vec<ComposeContainer>, String> {
    let trimmed = contents.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    if trimmed.starts_with('[') {
        let array = serde_json::from_str::<Vec<serde_json::Value>>(trimmed)
            .map_err(|error| format!("expected a JSON array: {error}"))?;
        values.extend(array);
    } else {
        for line in trimmed.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value = serde_json::from_str::<serde_json::Value>(line)
                .map_err(|error| format!("expected one JSON object per line: {error}"))?;
            match value {
                serde_json::Value::Array(items) => values.extend(items),
                value @ serde_json::Value::Object(_) => values.push(value),
                _ => return Err("expected one JSON object per line".to_string()),
            }
        }
    }
    let mut containers = Vec::new();
    for value in values {
        let Some(object) = value.as_object() else {
            continue;
        };
        let service = ["Service", "service"]
            .iter()
            .find_map(|name| object.get(*name).and_then(|value| value.as_str()));
        let Some(service) = service else {
            continue;
        };
        let state = ["State", "state", "Status", "status"]
            .iter()
            .find_map(|name| object.get(*name).and_then(|value| value.as_str()))
            .map(str::to_string);
        containers.push(ComposeContainer {
            service: service.to_string(),
            state,
        });
    }
    Ok(containers)
}

struct BoundedOutput {
    status: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}

fn run_bounded(
    program: &Path,
    args: &[String],
    directory: &Path,
    timeout: Duration,
) -> Result<BoundedOutput, String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout was not captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr was not captured".to_string())?;
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error.to_string());
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait().ok();
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(BoundedOutput {
        status: status.and_then(|status| status.code()),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
    })
}

fn describe_failure(output: &BoundedOutput) -> String {
    if output.timed_out {
        return "timed out".to_string();
    }
    let message = if output.stderr.trim().is_empty() {
        output.stdout.trim()
    } else {
        output.stderr.trim()
    };
    let first_line = message.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return match output.status {
            Some(code) => format!("exit code {code}"),
            None => "no exit status".to_string(),
        };
    }
    first_line.chars().take(300).collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let extensions = env::var_os("PATHEXT")
                .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into())
                .to_string_lossy()
                .to_string();
            for extension in extensions.split(';').filter(|value| !value.is_empty()) {
                let candidate = directory.join(format!("{name}{extension}"));
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn format_utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = remainder / 3_600;
    let minute = (remainder % 3_600) / 60;
    let second = remainder % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn utc_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_utc_timestamp(seconds)
}

const RUNTIME_TIMEOUT: Duration = Duration::from_secs(5);

/// Observes Docker/Compose runtime state for detected container components.
///
/// This is only called when `--live` explicitly authorizes runtime inspection.
/// The only commands run are bounded `docker info` and `docker compose ps`.
fn inspect_runtime(detection: &RepositoryDetection) -> RuntimeLayer {
    let scopes = collect_compose_scopes(detection);
    let mut layer = RuntimeLayer {
        scope: Some(runtime_scope_label(&scopes)),
        observed_at: Some(utc_timestamp()),
        permission: Some("authorized (--live)".to_string()),
        commands: Vec::new(),
        failure: None,
        observations: Vec::new(),
    };
    if scopes.is_empty() {
        return layer;
    }

    let Some(docker) = find_executable("docker") else {
        let failure = "docker CLI not found on PATH".to_string();
        layer.failure = Some(failure.clone());
        for scope in &scopes {
            layer.observations.push(runtime_observation(
                scope.component.clone(),
                "blocked",
                "environment",
                vec!["PATH".to_string()],
                Some(failure.clone()),
                Some(failure.clone()),
                Some(scope_for_observation(scope)),
            ));
        }
        return layer;
    };

    let info_args = vec![
        "info".to_string(),
        "--format".to_string(),
        "{{.ServerVersion}}".to_string(),
    ];
    layer
        .commands
        .push("docker info --format {{.ServerVersion}}".to_string());
    let info = run_bounded(&docker, &info_args, &detection.root, RUNTIME_TIMEOUT);
    let daemon_failure = match &info {
        Err(error) => Some(format!("could not start docker: {error}")),
        Ok(output) if output.timed_out => Some(format!(
            "docker info timed out after {}s",
            RUNTIME_TIMEOUT.as_secs()
        )),
        Ok(output) if output.status != Some(0) => Some(format!(
            "docker daemon inaccessible: {}",
            describe_failure(output)
        )),
        Ok(_) => None,
    };
    if let Some(failure) = daemon_failure {
        layer.failure = Some(failure.clone());
        for scope in &scopes {
            layer.observations.push(runtime_observation(
                scope.component.clone(),
                "blocked",
                "docker",
                vec!["docker info".to_string()],
                Some(failure.clone()),
                Some(failure.clone()),
                Some(scope_for_observation(scope)),
            ));
        }
        return layer;
    }

    for scope in &scopes {
        let observation_scope = scope_for_observation(scope);
        if scope.files.is_empty() {
            layer.observations.push(runtime_observation(
                scope.component.clone(),
                "unsupported",
                "docker",
                scope.manifests.clone(),
                Some(
                    "runtime observation is scoped to Compose projects; no Compose file was found"
                        .to_string(),
                ),
                None,
                Some(observation_scope),
            ));
            continue;
        }
        if let Some(failure) = &scope.read_failure {
            layer.failure.get_or_insert_with(|| failure.clone());
            layer.observations.push(runtime_observation(
                scope.component.clone(),
                "failed",
                "docker",
                scope.display_files.clone(),
                Some(failure.clone()),
                Some(failure.clone()),
                Some(observation_scope),
            ));
            continue;
        }
        if scope.services.is_empty() {
            layer.observations.push(runtime_observation(
                scope.component.clone(),
                "unknown",
                "docker",
                scope.display_files.clone(),
                Some("Compose file declares no services".to_string()),
                None,
                Some(observation_scope),
            ));
            continue;
        }

        let command = compose_ps_command(&scope.files);
        layer.commands.push(command);
        let args = compose_ps_args(&scope.files);
        match run_bounded(&docker, &args, &scope.directory, RUNTIME_TIMEOUT) {
            Err(error) => {
                let failure = format!("could not start docker compose ps: {error}");
                layer.failure.get_or_insert_with(|| failure.clone());
                layer.observations.push(runtime_observation(
                    scope.component.clone(),
                    "failed",
                    "docker",
                    scope.display_files.clone(),
                    Some(failure.clone()),
                    Some(failure),
                    Some(observation_scope),
                ));
            }
            Ok(output) if output.timed_out => {
                let failure = format!(
                    "docker compose ps timed out after {}s",
                    RUNTIME_TIMEOUT.as_secs()
                );
                layer.failure.get_or_insert_with(|| failure.clone());
                layer.observations.push(runtime_observation(
                    scope.component.clone(),
                    "failed",
                    "docker",
                    scope.display_files.clone(),
                    Some(failure.clone()),
                    Some(failure),
                    Some(observation_scope),
                ));
            }
            Ok(output) if output.status != Some(0) => {
                let failure = format!("docker compose ps failed: {}", describe_failure(&output));
                layer.failure.get_or_insert_with(|| failure.clone());
                layer.observations.push(runtime_observation(
                    scope.component.clone(),
                    "failed",
                    "docker",
                    scope.display_files.clone(),
                    Some(failure.clone()),
                    Some(failure),
                    Some(observation_scope),
                ));
            }
            Ok(output) => match parse_compose_ps(&output.stdout) {
                Err(error) => {
                    let failure = format!("could not parse docker compose ps output: {error}");
                    layer.failure.get_or_insert_with(|| failure.clone());
                    layer.observations.push(runtime_observation(
                        scope.component.clone(),
                        "failed",
                        "docker",
                        scope.display_files.clone(),
                        Some(failure.clone()),
                        Some(failure),
                        Some(observation_scope),
                    ));
                }
                Ok(containers) => {
                    for service in &scope.services {
                        let subject = dependency_subject(
                            detection
                                .components
                                .iter()
                                .find(|component| component.path == scope.component)
                                .expect("scope belongs to a detected component"),
                            service,
                        );
                        let container = containers
                            .iter()
                            .find(|container| container.service == *service);
                        let (status, detail) = match container {
                            Some(container) if container.running() => {
                                let label = container.state_label();
                                let detail = if label.eq_ignore_ascii_case("running") {
                                    "running".to_string()
                                } else {
                                    format!("running ({label})")
                                };
                                ("present", Some(detail))
                            }
                            Some(container) if container.state.is_none() => {
                                ("unknown", Some("Compose reported no state".to_string()))
                            }
                            Some(container) => (
                                "absent",
                                Some(format!("not running ({})", container.state_label())),
                            ),
                            None => ("absent", Some("not running".to_string())),
                        };
                        layer.observations.push(runtime_observation(
                            subject,
                            status,
                            "docker",
                            scope.display_files.clone(),
                            detail,
                            None,
                            Some(observation_scope.clone()),
                        ));
                    }
                }
            },
        }
    }
    layer
}

fn render_graph(output: &GraphOutput) -> String {
    let mut lines = vec!["PROJECT GRAPH".to_string()];
    lines.extend(
        output
            .nodes
            .iter()
            .filter(|node| node.kind == "component")
            .map(|node| {
                format!(
                    "{} ({})",
                    node.id,
                    serde_json::to_string(&node.ecosystem).unwrap()
                )
            }),
    );
    lines.join("\n")
}

fn render_runtime(layer: &RuntimeLayer, live: bool) -> Vec<String> {
    if !live {
        return vec![
            "RUNTIME".to_string(),
            "  not inspected (pass --live to authorize Docker/Compose observation)".to_string(),
        ];
    }
    let mut lines = vec!["RUNTIME".to_string()];
    let mut metadata = Vec::new();
    if let Some(scope) = &layer.scope {
        metadata.push(format!("scope {scope}"));
    }
    if let Some(observed_at) = &layer.observed_at {
        metadata.push(format!("observed {observed_at}"));
    }
    if let Some(permission) = &layer.permission {
        metadata.push(format!("permission {permission}"));
    }
    if !metadata.is_empty() {
        lines.push(format!("  {}", metadata.join(", ")));
    }
    if let Some(failure) = &layer.failure {
        lines.push(format!("  failure: {failure}"));
    }
    if !layer.commands.is_empty() {
        lines.push(format!("  commands: {}", layer.commands.join("; ")));
    }
    if layer.observations.is_empty() {
        lines.push("  no container components to inspect".to_string());
    }
    lines.extend(layer.observations.iter().map(|observation| {
        let detail = observation
            .detail
            .as_deref()
            .map(|detail| format!(" ({detail})"))
            .unwrap_or_default();
        format!(
            "  {}: {}{}",
            observation.subject, observation.status, detail
        )
    }));
    lines
}

fn render_reality(output: &RealityOutput) -> String {
    let count = |layer: &RealityLayer| layer.observations.len();
    let mut lines = vec![
        format!("PROJECT\n  {} components", count(&output.declared)),
        format!("DECLARED\n  {}", count(&output.declared)),
        format!("RESOLVED\n  {}", count(&output.resolved)),
        format!("INSTALLED\n  {}", count(&output.installed)),
    ];
    lines.extend(render_runtime(&output.runtime, output.live));
    lines.join("\n")
}

fn render_drift(output: &DriftOutput) -> String {
    let mut lines = Vec::new();
    if output.issues.is_empty() {
        lines.push("NO DRIFT".to_string());
    } else {
        match output.status.as_str() {
            "unknown" => lines.push("UNKNOWN".to_string()),
            "ambiguous" => lines.push("AMBIGUOUS".to_string()),
            _ => {}
        }
        lines.push(format!("{} issues", output.issues.len()));
        lines.extend(
            output
                .issues
                .iter()
                .map(|issue| format!("  {}\n    {}", issue.subject, issue.actual)),
        );
    }
    if let Some(runtime) = &output.runtime {
        lines.extend(render_runtime(runtime, true));
    }
    lines.join("\n")
}

struct ParsedCli {
    command: CommandName,
    json: bool,
    run_args: Vec<String>,
    tool: Option<String>,
    dir: Option<PathBuf>,
    dry_run: bool,
    live: bool,
    observe: observe::ObserveOptions,
}

fn parse_args<I, S>(args: I) -> Result<ParsedCli, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if !args.is_empty() {
        args.remove(0);
    }
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        return Err(CliError {
            message: usage(),
            exit_code: 0,
        });
    }
    if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-V") {
        return Err(CliError {
            message: format!("pax {}", env!("CARGO_PKG_VERSION")),
            exit_code: 0,
        });
    }
    if args.len() == 2
        && args[1] == "--help"
        && matches!(
            args[0].as_str(),
            "build"
                | "test"
                | "lint"
                | "typecheck"
                | "run"
                | "x"
                | "install"
                | "graph"
                | "reality"
                | "drift"
                | "observe"
        )
    {
        return Err(CliError {
            message: command_usage(&args[0]),
            exit_code: 0,
        });
    }

    let mut json = false;
    let mut dry_run = false;
    let mut live = false;
    let mut tool = None;
    let mut dir = None;
    let mut observe = observe::ObserveOptions::default();
    let mut filtered = Vec::with_capacity(args.len());
    let mut index = 0;
    let mut options = true;
    while index < args.len() {
        let arg = &args[index];
        if options && arg == "--" {
            options = false;
            index += 1;
            continue;
        }
        if options && arg == "--json" {
            json = true;
        } else if options && arg == "--dry-run" {
            dry_run = true;
        } else if options && arg == "--live" {
            live = true;
        } else if options && arg == "--tool" {
            index += 1;
            tool = Some(
                args.get(index)
                    .ok_or_else(|| CliError {
                        message: "pax --tool requires a tool".to_string(),
                        exit_code: 2,
                    })?
                    .clone(),
            );
        } else if options
            && matches!(
                arg.as_str(),
                "--scope" | "--max-files" | "--max-bytes" | "--max-facts"
            )
        {
            let flag = arg.clone();
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| CliError {
                    message: format!("pax {flag} requires a value"),
                    exit_code: 2,
                })?
                .clone();
            match flag.as_str() {
                "--scope" => observe.scope = Some(value),
                "--max-files" => observe.max_files = Some(value),
                "--max-bytes" => observe.max_bytes = Some(value),
                _ => observe.max_facts = Some(value),
            }
        } else if options && arg == "--dir" {
            index += 1;
            dir = Some(PathBuf::from(args.get(index).ok_or_else(|| CliError {
                message: "pax --dir requires a directory".to_string(),
                exit_code: 2,
            })?));
        } else {
            filtered.push(arg.clone());
        }
        index += 1;
    }
    args = filtered;

    let (command, run_args) = match args.as_slice() {
        [command, rest @ ..]
            if matches!(command.as_str(), "build" | "test" | "lint" | "typecheck") =>
        {
            let command = match command.as_str() {
                "build" => CommandName::Build,
                "test" => CommandName::Test,
                "lint" => CommandName::Lint,
                "typecheck" => CommandName::Typecheck,
                _ => unreachable!(),
            };
            (command, rest.to_vec())
        }
        [command, target, rest @ ..] if command == "run" || command == "x" => {
            if target.is_empty() {
                return Err(CliError {
                    message: format!("pax {command} requires a target\n\n{}", usage()),
                    exit_code: 2,
                });
            }
            (
                if command == "run" {
                    CommandName::Run
                } else {
                    CommandName::X
                },
                std::iter::once(target.clone())
                    .chain(rest.iter().cloned())
                    .collect(),
            )
        }
        [command, rest @ ..] if command == "deploy" => (CommandName::Deploy, rest.to_vec()),
        [command, rest @ ..] if command == "install" => (CommandName::Install, rest.to_vec()),
        [command, rest @ ..] if command == "add" => (CommandName::Add, rest.to_vec()),
        [command, rest @ ..] if command == "remove" => (CommandName::Remove, rest.to_vec()),
        [command, rest @ ..] if command == "exec" => (CommandName::Exec, rest.to_vec()),
        [command] if command == "info" => (CommandName::Info, Vec::new()),
        [command] if command == "doctor" => (CommandName::Doctor, Vec::new()),
        [command] if command == "deps" => (CommandName::Deps, Vec::new()),
        [command] if command == "scripts" => (CommandName::Scripts, Vec::new()),
        [command] if command == "workspaces" => (CommandName::Workspaces, Vec::new()),
        [command] if command == "lock" => (CommandName::Lock, Vec::new()),
        [command] if command == "graph" => (CommandName::Graph, Vec::new()),
        [command] if command == "reality" => (CommandName::Reality, Vec::new()),
        [command] if command == "drift" => (CommandName::Drift, Vec::new()),
        [command] if command == "observe" => (CommandName::Observe, Vec::new()),
        [command] if command == "run" => {
            return Err(CliError {
                message: format!("pax run requires a target\n\n{}", usage()),
                exit_code: 2,
            });
        }
        [command] if command == "x" => {
            return Err(CliError {
                message: format!("pax x requires a package\n\n{}", usage()),
                exit_code: 2,
            });
        }
        [command] if command == "deploy" => {
            return Err(CliError {
                message: format!("pax deploy requires a provider\n\n{}", usage()),
                exit_code: 2,
            });
        }
        [] => {
            return Err(CliError {
                message: usage(),
                exit_code: 2,
            });
        }
        _ => {
            let guidance = args.first().map_or(String::new(), |operation| {
                format!("\nTo run a project-defined operation, use: pax run {operation}")
            });
            return Err(CliError {
                message: format!(
                    "unknown arguments: {}{guidance}\n\n{}",
                    args.join(" "),
                    usage()
                ),
                exit_code: 2,
            });
        }
    };

    Ok(ParsedCli {
        command,
        json,
        run_args,
        tool,
        dir,
        dry_run,
        live,
        observe,
    })
}

fn usage() -> String {
    r#"PAX is a universal, read-only project-tooling boundary.

Usage: pax [OPTIONS] COMMAND [ARGS...]

Inspection:
  info, doctor, deps, scripts, workspaces, lock
  graph, reality, drift
  observe            bounded deterministic project-structure observation
Project operations (delegated to native tools):
  build, test, lint, typecheck
Execution (delegated to native tools):
  run, x, install, add, remove, exec, deploy

Options:
  --dir <path>       select the project root
  --tool <tool>      explicitly select the native tool
  --dry-run          preview a delegated command without executing it
  --json             emit machine-readable output for automation
  --live             allow runtime observations for reality/drift
  --scope <scope>    observe: project (default), crate:<pkg>, module:<id>, file:<path>, path:<prefix>
  --max-files <n>    observe: bound on files read and directory entries listed (default 200)
  --max-bytes <n>    observe: bound on source bytes read (default 4194304)
  --max-facts <n>    observe: bound on facts returned (default 5000)
  -h, --help         show this help
  -V, --version      show the package version

All operations resolve to the project's native tooling. Use `pax run <operation>`
for project-defined operations. See README.md for the JSON contract."#
        .to_string()
}

fn command_usage(command: &str) -> String {
    match command {
        "build" => "Usage: pax build [args...]\nBuild the project with its selected native tool.".to_string(),
        "test" => "Usage: pax test [args...]\nTest the project with its selected native tool.".to_string(),
        "lint" => "Usage: pax lint [args...]\nLint the project with its selected native tool.".to_string(),
        "typecheck" => "Usage: pax typecheck [args...]\nTypecheck the project with its selected native tool.".to_string(),
        "run" => "Usage: pax run <target> [args...]\nRun a project task with the selected native tool.".to_string(),
        "x" => "Usage: pax x <package> [args...]\nRun an ephemeral package or tool with the native runner.".to_string(),
        "install" => "Usage: pax install [package...]\nInstall declared project dependencies or named packages.".to_string(),
        "graph" => "Usage: pax graph [--json]\nInspect static component and dependency relationships.".to_string(),
        "reality" => "Usage: pax reality [--live] [--json]\nCompare declared, resolved, installed, and runtime observations.".to_string(),
        "observe" => "Usage: pax observe [--scope <scope>] [--max-files <n>] [--max-bytes <n>] [--max-facts <n>] [--json]\nReport bounded, deterministic project-structure facts with provenance. Rust source structure only.\nScopes: project, crate:<package>[/<kind>[/<name>]], module:<crate-id>::crate[::<module>...], file:<path>, path:<prefix>.\nExceeding a bound is an error, never a silent truncation.".to_string(),
        "drift" => "Usage: pax drift [--live] [--json]\nReport contradictions without repairing them.".to_string(),
        _ => unreachable!(),
    }
}

fn select_deploy_provider(
    detection: &RepositoryDetection,
    requested: Option<&str>,
) -> Result<DeploySelection, CliError> {
    let candidates = [
        (DeployProvider::Fly, "fly.toml"),
        (DeployProvider::Vercel, "vercel.json"),
        (DeployProvider::Vercel, ".vercel/project.json"),
        (DeployProvider::Netlify, "netlify.toml"),
    ];
    let mut detected = Vec::new();
    for (provider, evidence) in candidates {
        if detection.root.join(evidence).is_file()
            && !detected
                .iter()
                .any(|(found, _): &(DeployProvider, String)| *found == provider)
        {
            detected.push((provider, evidence.to_string()));
        }
    }
    if let Some(requested) = requested {
        let provider = DeployProvider::parse(requested).ok_or_else(|| CliError {
            message: format!("unsupported deployment provider: {requested}"),
            exit_code: 2,
        })?;
        let evidence = detected
            .iter()
            .filter(|(found, _)| *found == provider)
            .map(|(_, evidence)| evidence.clone())
            .collect();
        return Ok(DeploySelection { provider, evidence });
    }
    match detected.as_slice() {
        [(provider, evidence)] => Ok(DeploySelection {
            provider: *provider,
            evidence: vec![evidence.clone()],
        }),
        [] => Err(CliError {
            message: "could not detect a deployment provider (use --tool <provider>)".to_string(),
            exit_code: 1,
        }),
        _ => Err(CliError {
            message: format!(
                "ambiguous deployment providers: {}",
                detected
                    .iter()
                    .map(|(provider, evidence)| format!("{} ({evidence})", provider.name()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            exit_code: 2,
        }),
    }
}

fn build_x_command(
    detection: &RepositoryDetection,
    run_args: &[String],
    override_tool: Option<&str>,
) -> Result<RunCommand, CliError> {
    let package = run_args.first().ok_or_else(|| CliError {
        message: format!("pax x requires a package\n\n{}", usage()),
        exit_code: 2,
    })?;
    let mut args = run_args.to_vec();
    if package == "install" {
        return build_install_command(detection, &args[1..], override_tool);
    }
    let program = if let Some(tool) = override_tool {
        match tool {
            "npm" => "npx",
            "pnpm" => {
                args.insert(0, "dlx".to_string());
                "pnpm"
            }
            "bun" => "bunx",
            "yarn" => {
                args.insert(0, "dlx".to_string());
                "yarn"
            }
            "uv" => "uvx",
            "pip" => "pipx",
            _ => {
                return Err(CliError {
                    message: format!("unsupported package runner tool: {tool}"),
                    exit_code: 2,
                });
            }
        }
    } else if let Some(manager) = detection.manager.as_ref() {
        match manager.name {
            PackageManager::Npm => "npx",
            PackageManager::Pnpm => {
                args.insert(0, "dlx".to_string());
                "pnpm"
            }

            PackageManager::Bun => "bunx",
            PackageManager::Yarn => {
                args.insert(0, "dlx".to_string());
                "yarn"
            }
        }
    } else if let Some(component) = detection
        .components
        .iter()
        .find(|component| component.path == "." && component.ecosystem == Ecosystem::Python)
    {
        match component.tool.as_deref() {
            Some("uv") => "uvx",
            Some("poetry" | "pdm" | "pip") => "pipx",
            Some("pipenv") => {
                return Err(CliError {
                    message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                    exit_code: 2,
                });
            }
            _ => {
                return Err(CliError {
                    message: "could not detect an authoritative package runner".to_string(),
                    exit_code: 1,
                });
            }
        }
    } else {
        return Err(CliError {
            message: "could not detect an authoritative package runner".to_string(),
            exit_code: 1,
        });
    };
    let _ = package;
    Ok(RunCommand {
        program: program.to_string(),
        args,
        working_directory: detection.root.clone(),
    })
}

fn build_install_command(
    detection: &RepositoryDetection,
    install_args: &[String],
    override_tool: Option<&str>,
) -> Result<RunCommand, CliError> {
    if let Some(tool) = override_tool {
        if !matches!(tool, "npm" | "pnpm" | "bun" | "yarn") {
            return build_python_install_command(detection, install_args, tool);
        }

        let mut args = vec!["install".to_string()];
        args.extend(install_args.iter().cloned());
        return Ok(RunCommand {
            program: tool.to_string(),
            args,
            working_directory: detection.root.clone(),
        });
    }

    fn build_python_install_command(
        detection: &RepositoryDetection,
        install_args: &[String],
        tool: &str,
    ) -> Result<RunCommand, CliError> {
        let mut args = Vec::new();
        let program = match tool {
            "uv" => {
                args.push("pip".to_string());
                "uv"
            }
            "pip" => "pip",
            "poetry" => "poetry",
            "pdm" => "pdm",
            "pipenv" => {
                return Err(CliError {
                    message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                    exit_code: 2,
                });
            }
            _ => {
                return Err(CliError {
                    message: format!("unsupported package manager tool: {tool}"),
                    exit_code: 2,
                });
            }
        };
        args.push("install".to_string());
        args.extend(install_args.iter().cloned());
        Ok(RunCommand {
            program: program.to_string(),
            args,
            working_directory: detection.root.clone(),
        })
    }
    if let Some(manager) = detection.manager.as_ref() {
        let mut args = vec!["install".to_string()];
        args.extend(install_args.iter().cloned());
        return Ok(RunCommand {
            program: manager.name.display_name().to_string(),
            args,
            working_directory: detection.root.clone(),
        });
    }

    let component = detection
        .components
        .iter()
        .find(|component| component.path == "." && component.ecosystem == Ecosystem::Python)
        .ok_or_else(|| CliError {
            message: "could not detect an authoritative package manager".to_string(),
            exit_code: 1,
        })?;
    let mut args = Vec::new();
    let program = match component.tool.as_deref() {
        Some("uv") => {
            args.push("pip".to_string());
            "uv"
        }
        Some("poetry") => "poetry",
        Some("pdm") => "pdm",
        Some("pip") => "pip",
        Some("pipenv") => {
            return Err(CliError {
                message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                exit_code: 2,
            });
        }
        _ => {
            return Err(CliError {
                message: "could not detect an authoritative package manager".to_string(),
                exit_code: 1,
            });
        }
    };
    args.push("install".to_string());
    args.extend(install_args.iter().cloned());
    Ok(RunCommand {
        program: program.to_string(),
        args,
        working_directory: detection.root.clone(),
    })
}

fn build_package_mutation_command(
    detection: &RepositoryDetection,
    args: &[String],
    override_tool: Option<&str>,
    add: bool,
) -> Result<RunCommand, CliError> {
    let package = args.first().ok_or_else(|| CliError {
        message: format!(
            "pax {} requires a package\n\n{}",
            if add { "add" } else { "remove" },
            usage()
        ),
        exit_code: 2,
    })?;
    let inferred_tool = detection.components.iter().find_map(|component| {
        (component.path == "."
            && matches!(component.ecosystem, Ecosystem::Python | Ecosystem::Rust))
        .then_some(component.tool.as_deref())
        .flatten()
    });
    let tool = override_tool
        .or_else(|| {
            detection
                .manager
                .as_ref()
                .map(|manager| manager.name.display_name())
        })
        .or(inferred_tool)
        .ok_or_else(|| CliError {
            message: "could not determine an authoritative package tool; use --tool".to_string(),
            exit_code: 1,
        })?;
    let mut command_args = args.to_vec();
    let (program, operation) = match tool {
        "npm" => ("npm", if add { "install" } else { "uninstall" }),
        "pnpm" => ("pnpm", if add { "add" } else { "remove" }),
        "yarn" => ("yarn", if add { "add" } else { "remove" }),
        "bun" => ("bun", if add { "add" } else { "remove" }),
        "uv" => ("uv", if add { "add" } else { "remove" }),
        "cargo" => ("cargo", if add { "add" } else { "remove" }),
        "pipenv" => {
            return Err(CliError {
                message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                exit_code: 2,
            });
        }
        _ => {
            return Err(CliError {
                message: format!("unsupported canonical package operation for {tool}"),
                exit_code: 1,
            });
        }
    };
    command_args.insert(0, operation.to_string());
    let _ = package;
    Ok(RunCommand {
        program: program.to_string(),
        args: command_args,
        working_directory: detection.root.clone(),
    })
}

fn resolve_operation(
    detection: &RepositoryDetection,
    operation: Operation,
    args: &[String],
    override_tool: Option<&str>,
) -> Result<ResolvedOperation, CliError> {
    let mut ecosystem = None;
    let mut evidence = Vec::new();
    let mut operation_selection_reason = None;
    let command = match &operation {
        Operation::Build | Operation::Test | Operation::Lint | Operation::Typecheck => {
            resolve_standard_project_command(detection, &operation.name(), args, override_tool)?
        }
        Operation::Custom(name) => {
            let run_args = std::iter::once(name.clone())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>();
            build_run_command(detection, &run_args, override_tool)?
        }
        Operation::Install => {
            if args.is_empty() {
                resolve_root_install_command(detection, override_tool)?
            } else {
                build_install_command(detection, args, override_tool)?
            }
        }
        Operation::Add | Operation::Remove => build_package_mutation_command(
            detection,
            args,
            override_tool,
            matches!(operation, Operation::Add),
        )?,
        Operation::Deploy => {
            let selection = select_deploy_provider(detection, override_tool)?;
            let (program, canonical) = selection.provider.command();
            ecosystem = Some("deployment".to_string());
            evidence = selection.evidence;
            operation_selection_reason = Some(if override_tool.is_some() {
                format!(
                    "selected {} by explicit --tool override",
                    selection.provider.name()
                )
            } else {
                format!(
                    "selected {} from deployment evidence",
                    selection.provider.name()
                )
            });
            RunCommand {
                program: program.to_string(),
                args: std::iter::once(canonical.to_string())
                    .chain(args.iter().cloned())
                    .collect(),
                working_directory: detection.root.clone(),
            }
        }
    };
    let selection_reason = operation_selection_reason.unwrap_or_else(|| {
        if let Some(tool) = override_tool {
            format!("selected {tool} by explicit --tool override")
        } else if let Some(manager) = detection.manager.as_ref() {
            format!("selected {} by {}", manager.name, manager.selected_by)
        } else {
            let tool = detection
                .components
                .iter()
                .find(|component| component.path == ".")
                .and_then(|component| component.tool.as_deref())
                .unwrap_or(&command.program);
            format!("selected {tool} from root project evidence")
        }
    });
    Ok(ResolvedOperation {
        operation,
        command,
        selection_reason,
        ecosystem,
        evidence,
    })
}

fn resolve_standard_project_command(
    detection: &RepositoryDetection,
    operation: &str,
    extra_args: &[String],
    override_tool: Option<&str>,
) -> Result<RunCommand, CliError> {
    let selected_tool = override_tool.or_else(|| {
        detection
            .manager
            .as_ref()
            .map(|manager| manager.name.display_name())
    });
    if selected_tool.is_some_and(|tool| PackageManager::parse(tool).is_some()) {
        if !detection
            .package_json_data
            .as_ref()
            .is_some_and(|data| data.scripts.contains_key(operation))
        {
            return Err(CliError {
                message: format!(
                    "project operation '{operation}' is not supported: package.json has no '{operation}' script\nUse `pax run <operation>` for another project-defined operation."
                ),
                exit_code: 2,
            });
        }
        let run_args = std::iter::once(operation.to_string())
            .chain(extra_args.iter().cloned())
            .collect::<Vec<_>>();
        return build_run_command(detection, &run_args, override_tool);
    }

    let root_component = detection
        .components
        .iter()
        .find(|component| component.path == ".");
    let tool = override_tool
        .or_else(|| root_component.and_then(|component| component.tool.as_deref()))
        .ok_or_else(|| CliError {
            message: format!("project operation '{operation}' is not supported: no authoritative native tool was detected"),
            exit_code: 1,
        })?;
    if tool == "cargo" {
        let native_operation = match operation {
            "build" => "build",
            "test" => "test",
            "lint" => "clippy",
            "typecheck" => "check",
            _ => unreachable!(),
        };
        return Ok(RunCommand {
            program: "cargo".to_string(),
            args: std::iter::once(native_operation.to_string())
                .chain(extra_args.iter().cloned())
                .collect(),
            working_directory: detection.root.clone(),
        });
    }
    if tool == "docker" && operation == "build" {
        return Ok(RunCommand {
            program: "docker".to_string(),
            args: std::iter::once("compose".to_string())
                .chain(std::iter::once("build".to_string()))
                .chain(extra_args.iter().cloned())
                .collect(),
            working_directory: detection.root.clone(),
        });
    }
    let run_args = std::iter::once(operation.to_string())
        .chain(extra_args.iter().cloned())
        .collect::<Vec<_>>();
    build_run_command(detection, &run_args, override_tool)
}

fn resolve_root_install_command(
    detection: &RepositoryDetection,
    override_tool: Option<&str>,
) -> Result<RunCommand, CliError> {
    if detection.package_json
        || override_tool.is_some_and(|tool| PackageManager::parse(tool).is_some())
    {
        return build_install_command(detection, &[], override_tool);
    }
    let component = detection
        .components
        .iter()
        .find(|component| component.path == ".")
        .ok_or_else(|| CliError {
            message: "no installable root project detected".to_string(),
            exit_code: 1,
        })?;
    let tool = override_tool
        .or(component.tool.as_deref())
        .ok_or_else(|| CliError {
            message: "could not detect an authoritative package manager".to_string(),
            exit_code: 1,
        })?;
    let (program, args) = match tool {
        "pipenv" => {
            return Err(CliError {
                message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                exit_code: 2,
            });
        }
        "uv" => ("uv", vec!["sync".to_string()]),
        "poetry" | "pdm" => (tool, vec!["install".to_string()]),
        "pip" => {
            let requirements = component
                .manifests
                .iter()
                .find(|path| path.ends_with(".txt"));
            match requirements {
                Some(path) => (
                    "pip",
                    vec!["install".to_string(), "-r".to_string(), path.clone()],
                ),
                None => ("pip", vec!["install".to_string(), ".".to_string()]),
            }
        }
        "cargo" => ("cargo", vec!["fetch".to_string()]),
        _ => {
            return Err(CliError {
                message: format!("unsupported install operation for {tool}"),
                exit_code: 2,
            });
        }
    };
    Ok(RunCommand {
        program: program.to_string(),
        args,
        working_directory: detection.root.clone(),
    })
}

fn dispatch_resolved_operation(
    resolved: ResolvedOperation,
    detection: &RepositoryDetection,
    dry_run: bool,
    json: bool,
) -> Result<String, CliError> {
    if dry_run {
        let is_deploy = matches!(resolved.operation, Operation::Deploy);
        let mut plan = execution_plan(&resolved.command);
        plan.operation = resolved.operation.name();
        plan.project_root = detection.root.display().to_string();
        plan.workspace = detection.workspace_source.clone();
        plan.selection_reason = resolved.selection_reason;
        if let Some(ecosystem) = resolved.ecosystem {
            plan.ecosystem = ecosystem;
        }
        if !resolved.evidence.is_empty() {
            plan.evidence = resolved.evidence;
        }
        return if json {
            serde_json::to_string_pretty(&plan).map_err(|error| CliError {
                message: format!("failed to serialize execution plan: {error}"),
                exit_code: 1,
            })
        } else {
            if is_deploy {
                Ok(format!(
                    "ecosystem: {}\nprovider: {}\nevidence: {}\ncommand: {}\nworking directory: {}",
                    plan.ecosystem,
                    plan.tool,
                    if plan.evidence.is_empty() {
                        "none".to_string()
                    } else {
                        plan.evidence.join(", ")
                    },
                    plan.command.join(" "),
                    plan.working_directory
                ))
            } else {
                Ok(format!(
                    "Operation:   {}\nTool:        {}\nSelected by: {}\nCommand:     {}\nWorking dir: {}",
                    plan.operation,
                    plan.tool,
                    plan.selection_reason,
                    plan.command.join(" "),
                    plan.working_directory
                ))
            }
        };
    }
    dispatch_execution(resolved.command, false, json)
}

fn dispatch_execution(command: RunCommand, dry_run: bool, json: bool) -> Result<String, CliError> {
    if dry_run {
        let plan = execution_plan(&command);
        return if json {
            serde_json::to_string_pretty(&plan).map_err(|error| CliError {
                message: format!("failed to serialize execution plan: {error}"),
                exit_code: 1,
            })
        } else {
            Ok(format!(
                "Ecosystem:   {}\nTool:        {}\nOperation:   {}\nEvidence:    {}\nCommand:     {}\nWorking dir: {}",
                plan.ecosystem,
                plan.tool,
                plan.operation,
                if plan.evidence.is_empty() {
                    "none".to_string()
                } else {
                    plan.evidence.join(", ")
                },
                plan.command.join(" "),
                plan.working_directory
            ))
        };
    }
    let status = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.working_directory)
        .status()
        .map_err(|error| CliError {
            message: format!("failed to execute {}: {error}", command.program),
            exit_code: 1,
        })?;
    match status.code() {
        Some(0) => Ok(String::new()),
        Some(code) => Err(CliError {
            message: String::new(),
            exit_code: code.min(u8::MAX as i32) as u8,
        }),
        None => Err(CliError {
            message: String::new(),
            exit_code: 1,
        }),
    }
}

fn execution_plan(command: &RunCommand) -> ExecutionPlan {
    let tool = match command.program.as_str() {
        "npx" => "npm",
        "pnpm" => "pnpm",
        "bunx" => "bun",
        "yarn" => "yarn",
        "uvx" | "uv" => "uv",
        "pipx" | "pip" => "pip",
        "cargo" => "cargo",
        "docker" => "docker",
        other => other,
    };
    let operation = if matches!(command.args.first().map(String::as_str), Some("install")) {
        "install"
    } else if matches!(
        command.args.first().map(String::as_str),
        Some("add" | "remove" | "uninstall")
    ) {
        command.args.first().unwrap()
    } else if command.args.first().map(String::as_str) == Some("run") {
        "task"
    } else if command.program == "npx"
        || command.program == "bunx"
        || command.program == "uvx"
        || (command.program == "pnpm" && command.args.first().map(String::as_str) == Some("dlx"))
        || (command.program == "yarn" && command.args.first().map(String::as_str) == Some("dlx"))
    {
        "package-run"
    } else {
        "exec"
    };
    let mut evidence = Vec::new();
    if command.working_directory.join("package.json").is_file() {
        if let Ok(contents) = fs::read_to_string(command.working_directory.join("package.json"))
            && contents.contains("\"packageManager\"")
        {
            evidence.push("package.json#packageManager".to_string());
        }
        for lockfile in [
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
        ] {
            if command.working_directory.join(lockfile).is_file() {
                evidence.push(lockfile.to_string());
            }
        }
    }
    for source in [
        "requirements.txt",
        "requirements-dev.txt",
        "uv.lock",
        "poetry.lock",
        "pdm.lock",
        "pyproject.toml",
    ] {
        if command.working_directory.join(source).is_file() {
            evidence.push(source.to_string());
        }
    }
    ExecutionPlan {
        schema: contract::PLAN_SCHEMA,
        operation: operation.to_string(),
        ecosystem: if tool == "cargo" {
            "rust"
        } else if matches!(tool, "uv" | "pip") {
            "python"
        } else if tool == "docker" {
            "docker"
        } else {
            "javascript"
        }
        .to_string(),
        tool: tool.to_string(),
        runner: if command.program == "pnpm"
            && command.args.first().map(String::as_str) == Some("dlx")
        {
            "pnpm dlx".to_string()
        } else {
            command.program.clone()
        },
        command: std::iter::once(command.program.clone())
            .chain(command.args.iter().cloned())
            .collect(),
        evidence,
        working_directory: command.working_directory.display().to_string(),
        project_root: command.working_directory.display().to_string(),
        workspace: None,
        environment: BTreeMap::new(),
        supported: true,
        selection_reason: format!("selected {tool} from project evidence"),
    }
}

fn build_run_command(
    detection: &RepositoryDetection,
    run_args: &[String],
    override_tool: Option<&str>,
) -> Result<RunCommand, CliError> {
    let target = run_args.first().ok_or_else(|| CliError {
        message: format!("pax run requires a target\n\n{}", usage()),
        exit_code: 2,
    })?;
    let extra_args = &run_args[1..];

    if let Some(tool) = override_tool {
        if let Some(name) = PackageManager::parse(tool) {
            let mut args = vec!["run".to_string(), target.clone()];
            args.extend(extra_args.iter().cloned());
            return Ok(RunCommand {
                program: name.display_name().to_string(),
                args,
                working_directory: detection.root.clone(),
            });
        }
        let mut args = Vec::new();
        let program = match tool {
            "pipenv" => {
                return Err(CliError {
                    message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                    exit_code: 2,
                });
            }
            "uv" | "poetry" | "pdm" => {
                args.push("run".to_string());
                tool
            }
            "cargo" => {
                args.extend([
                    "run".to_string(),
                    "--bin".to_string(),
                    target.clone(),
                    "--".to_string(),
                ]);
                "cargo"
            }
            "docker" => {
                args.extend([
                    "compose".to_string(),
                    "run".to_string(),
                    "--rm".to_string(),
                    target.clone(),
                ]);
                "docker"
            }
            _ => {
                return Err(CliError {
                    message: format!("unsupported execution tool: {tool}"),
                    exit_code: 2,
                });
            }
        };
        if !matches!(tool, "cargo" | "docker") {
            args.push(target.clone());
        }
        args.extend(extra_args.iter().cloned());
        return Ok(RunCommand {
            program: program.to_string(),
            args,
            working_directory: detection.root.clone(),
        });
    }

    let manager = detection.manager.clone();
    if let Some(manager) = manager.as_ref() {
        let mut args = vec!["run".to_string(), target.clone()];
        args.extend(extra_args.iter().cloned());
        return Ok(RunCommand {
            program: manager.name.display_name().to_string(),
            args,
            working_directory: detection.root.clone(),
        });
    }

    let component = detection
        .components
        .iter()
        .filter(|component| component.path == ".")
        .find(|component| matches!(component.ecosystem, Ecosystem::Python))
        .or_else(|| {
            detection
                .components
                .iter()
                .filter(|component| component.path == ".")
                .find(|component| matches!(component.ecosystem, Ecosystem::Rust))
        })
        .or_else(|| {
            detection
                .components
                .iter()
                .filter(|component| component.path == ".")
                .find(|component| matches!(component.ecosystem, Ecosystem::Container))
        })
        .ok_or_else(|| CliError {
            message: "could not detect an authoritative execution tool".to_string(),
            exit_code: 1,
        })?;
    let working_directory = if component.path == "." {
        detection.root.clone()
    } else {
        detection.root.join(&component.path)
    };
    let mut args = Vec::new();
    let program = match (component.ecosystem, component.tool.as_deref()) {
        (Ecosystem::Python, Some("uv" | "poetry" | "pdm")) => {
            args.push("run".to_string());
            component.tool.clone().unwrap()
        }
        (Ecosystem::Python, Some("pip")) => "python".to_string(),
        (Ecosystem::Python, Some("pipenv")) => {
            return Err(CliError {
                message: "pipenv is not supported: PAX can discover Pipfile declarations but cannot run pipenv commands".to_string(),
                exit_code: 2,
            });
        }
        (Ecosystem::Rust, Some("cargo")) => {
            args.extend(["run".to_string(), "--bin".to_string(), target.clone()]);
            args.extend(["--".to_string()]);
            "cargo".to_string()
        }
        (Ecosystem::Container, Some("docker")) => {
            args.push("compose".to_string());
            if matches!(target.as_str(), "up" | "down" | "logs" | "ps") {
                args.push(target.clone());
            } else {
                args.extend(["run".to_string(), "--rm".to_string(), target.clone()]);
            }
            "docker".to_string()
        }
        _ => {
            return Err(CliError {
                message: "could not detect an authoritative execution tool".to_string(),
                exit_code: 1,
            });
        }
    };
    if !matches!(component.ecosystem, Ecosystem::Rust | Ecosystem::Container) {
        args.push(target.clone());
    }
    args.extend(extra_args.iter().cloned());
    Ok(RunCommand {
        program,
        args,
        working_directory,
    })
}

fn detect_repository(root: &Path) -> Result<RepositoryDetection, CliError> {
    let mut detection_diagnostics = Vec::new();
    let cargo = detect_cargo(root, &mut detection_diagnostics);
    let package_json_path = root.join("package.json");
    let package_json_data = if package_json_path.is_file() {
        Some(read_package_json(&package_json_path, "package.json")?)
    } else {
        None
    };

    let package_json = package_json_data.is_some();
    let workspace_files = ["pnpm-workspace.yaml"]
        .into_iter()
        .filter(|file| root.join(file).is_file())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let lockfiles = [
        "bun.lock",
        "bun.lockb",
        "pnpm-lock.yaml",
        "yarn.lock",
        "package-lock.json",
    ]
    .into_iter()
    .filter(|file| root.join(file).is_file())
    .map(str::to_string)
    .collect::<Vec<_>>();

    let workspace_source = if package_json_data
        .as_ref()
        .is_some_and(|data| data.has_workspaces)
    {
        Some("package.json#workspaces".to_string())
    } else if workspace_files
        .iter()
        .any(|file| file == "pnpm-workspace.yaml")
    {
        Some("pnpm-workspace.yaml".to_string())
    } else {
        None
    };

    let workspace = workspace_source.is_some();
    let workspace_packages = package_json_data
        .as_ref()
        .map(|data| data.workspace_packages.clone())
        .filter(|packages| !packages.is_empty())
        .unwrap_or_else(|| {
            workspace_files
                .iter()
                .filter(|file| *file == "pnpm-workspace.yaml")
                .flat_map(|_| {
                    fs::read_to_string(root.join("pnpm-workspace.yaml"))
                        .unwrap_or_default()
                        .lines()
                        .filter_map(|line| line.trim().strip_prefix("- "))
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect()
        });
    let (manager, mut selection_notes) = select_manager(package_json_data.as_ref(), &lockfiles);
    if cargo.is_some() && !package_json {
        selection_notes.clear();
        selection_notes.push("Cargo.toml selects Cargo".to_string());
    }
    let fallback_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-project")
        .to_string();
    let project_name = package_json_data
        .as_ref()
        .and_then(|data| data.name.clone())
        .or_else(|| {
            cargo.as_ref().and_then(|cargo| {
                cargo
                    .packages
                    .iter()
                    .find(|package| package.manifest_path == cargo.manifest)
                    .map(|package| package.name.clone())
            })
        })
        .unwrap_or(fallback_name);
    let (mut components, mut native_dependencies, container) =
        detect_components(root, &mut detection_diagnostics);
    if let Some(cargo) = &cargo {
        if let Some(component) = components
            .iter_mut()
            .find(|component| component.ecosystem == Ecosystem::Rust && component.path == ".")
        {
            if cargo.workspace {
                component.workspace_packages = cargo.workspace_members.clone();
            }
            component.lockfiles = cargo.lockfile.iter().cloned().collect();
            component.evidence.retain(|item| item.kind != "lockfile");
            component
                .evidence
                .extend(cargo.lockfile.iter().map(|path| EvidenceItem {
                    kind: "lockfile".to_string(),
                    path: path.clone(),
                }));
        }
        if cargo.source.starts_with("cargo metadata") {
            native_dependencies.retain(|dependency| dependency.ecosystem != Ecosystem::Rust);
            native_dependencies.extend(cargo.dependencies.iter().cloned());
        }
    }

    Ok(RepositoryDetection {
        root: root.to_path_buf(),
        project_name,
        package_json,
        package_json_data,
        workspace,
        workspace_source,
        manager,
        lockfiles,
        workspace_files,
        workspace_packages,
        selection_notes,
        components,
        native_dependencies,
        container,
        cargo,
        detection_diagnostics,
    })
}

fn display_path(path: &Path, root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(root) {
        return project_relative_path(&relative.to_string_lossy())
            .unwrap_or_else(|_| ".".to_string());
    }
    // A path outside the project root must never become a machine-specific
    // absolute identity. Keep an opaque, stable suffix instead.
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("external/{name}"))
        .unwrap_or_else(|| "external/unknown".to_string())
}

fn detect_cargo(root: &Path, diagnostics: &mut Vec<Diagnostic>) -> Option<CargoObservation> {
    let manifest_path = root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return None;
    }
    let manifest_contents = fs::read_to_string(&manifest_path).unwrap_or_default();
    let manifest = toml::from_str::<toml::Value>(&manifest_contents);
    if let Err(error) = &manifest {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            check: "Cargo.toml",
            message: format!("failed to parse Cargo.toml: {error}"),
        });
    }
    let declares_workspace = declares_workspace_section(&manifest_contents);
    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest_path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok());

    if let Some(metadata) = metadata {
        let workspace_root = metadata["workspace_root"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| root.to_path_buf());
        let member_ids = metadata["workspace_members"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut packages = metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|package| {
                let name = package["name"].as_str()?.to_string();
                let manifest = PathBuf::from(package["manifest_path"].as_str()?);
                let id = &package["id"];
                Some(CargoPackage {
                    name,
                    manifest_path: display_path(&manifest, root),
                    workspace_member: member_ids.iter().any(|member| member == id),
                })
            })
            .collect::<Vec<_>>();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        let mut workspace_members = packages
            .iter()
            .filter(|package| package.workspace_member)
            .map(|package| package.name.clone())
            .collect::<Vec<_>>();
        workspace_members.sort();
        let package_dependencies = metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|package| {
                let manifest =
                    display_path(&PathBuf::from(package["manifest_path"].as_str()?), root);
                let mut dependencies = package["dependencies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|dependency| {
                        let name = dependency["name"].as_str()?.to_string();
                        let kind = match dependency["kind"].as_str() {
                            Some("dev") => "development",
                            Some("build") => "build",
                            _ => "dependency",
                        };
                        Some(NativeDependency {
                            name,
                            specifier: dependency["req"].as_str().unwrap_or("*").to_string(),
                            kind: kind.to_string(),
                            ecosystem: Ecosystem::Rust,
                            native_kind: if dependency["optional"].as_bool() == Some(true) {
                                "optional".to_string()
                            } else {
                                kind.to_string()
                            },
                            source: manifest.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                sort_dependencies(&mut dependencies);
                Some((manifest, dependencies))
            })
            .collect::<BTreeMap<_, _>>();
        let mut dependencies = package_dependencies
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        sort_dependencies(&mut dependencies);
        let lockfile_path = workspace_root.join("Cargo.lock");
        return Some(CargoObservation {
            manifest: "Cargo.toml".to_string(),
            lockfile: lockfile_path
                .is_file()
                .then(|| display_path(&lockfile_path, root)),
            workspace: declares_workspace,
            workspace_root: display_path(&workspace_root, root),
            workspace_members,
            packages,
            dependencies,
            package_dependencies,
            source: "cargo metadata --no-deps --offline".to_string(),
        });
    }

    let package_name = manifest
        .as_ref()
        .ok()
        .and_then(|value| value.get("package"))
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            manifest_contents.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "name").then(|| value.trim().trim_matches(['"', '\'']).to_string())
            })
        });
    let members = manifest
        .as_ref()
        .ok()
        .and_then(|value| value.get("workspace"))
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| parse_toml_list(&manifest_contents, "[workspace]", "members"));
    Some(CargoObservation {
        manifest: "Cargo.toml".to_string(),
        lockfile: root
            .join("Cargo.lock")
            .is_file()
            .then(|| "Cargo.lock".to_string()),
        workspace: declares_workspace,
        workspace_root: ".".to_string(),
        workspace_members: package_name.iter().cloned().chain(members).collect(),
        packages: package_name
            .map(|name| CargoPackage {
                name,
                manifest_path: "Cargo.toml".to_string(),
                workspace_member: true,
            })
            .into_iter()
            .collect(),
        dependencies: Vec::new(),
        package_dependencies: BTreeMap::new(),
        source: "Cargo.toml fallback".to_string(),
    })
}

fn sort_dependencies(dependencies: &mut Vec<NativeDependency>) {
    dependencies.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.kind.cmp(&b.kind))
            .then(a.specifier.cmp(&b.specifier))
            .then(a.source.cmp(&b.source))
    });
    dependencies.dedup_by(|a, b| {
        a.name == b.name && a.kind == b.kind && a.specifier == b.specifier && a.source == b.source
    });
}

const JAVASCRIPT_LOCKFILES: [&str; 5] = [
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "yarn.lock",
    "package-lock.json",
];

/// Joins a component directory (`"."` for the project root) with a file name.
/// The result is a project-relative identity, never an absolute path.
fn join_project_path(relative: &str, file: &str) -> String {
    if relative == "." {
        file.to_string()
    } else {
        format!("{relative}/{file}")
    }
}

fn detect_components(
    root: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<Component>, Vec<NativeDependency>, Option<ContainerInfo>) {
    let mut roots = Vec::new();
    collect_project_dirs(root, &mut roots);
    let mut components = Vec::new();
    let mut dependencies = Vec::new();
    let mut container = None;
    for path in roots {
        let relative = path
            .strip_prefix(root)
            .ok()
            .and_then(|path| path.to_str())
            .filter(|path| !path.is_empty())
            .unwrap_or(".")
            .to_string();
        if let Some(component) =
            detect_python_component(root, &path, &relative, &mut dependencies, diagnostics)
        {
            components.push(component);
        }
        if let Some(component) =
            detect_rust_component(root, &path, &relative, &mut dependencies, diagnostics)
        {
            components.push(component);
        }
        if let Some(component) = detect_container_component(root, &path, &relative, &mut container)
        {
            components.push(component);
        }
        if let Some(component) =
            detect_javascript_component(root, &path, &relative, &mut dependencies, diagnostics)
        {
            components.push(component);
        }
    }
    components.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| format!("{:?}", a.ecosystem).cmp(&format!("{:?}", b.ecosystem)))
    });
    (components, dependencies, container)
}

fn detect_javascript_component(
    _root: &Path,
    path: &Path,
    relative: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Component> {
    let manifest = path.join("package.json");
    if !manifest.is_file() {
        return None;
    }
    let source = join_project_path(relative, "package.json");
    let lockfiles = JAVASCRIPT_LOCKFILES
        .iter()
        .filter(|name| path.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let tool = detect_javascript_manager(path);
    if relative != "." {
        match read_package_json(&manifest, &source) {
            Ok(data) => {
                for (group, kind, native_kind) in [
                    (data.dependencies, "dependency", "runtime-dependency"),
                    (
                        data.dev_dependencies,
                        "development",
                        "development-dependency",
                    ),
                    (
                        data.optional_dependencies,
                        "optional",
                        "optional-dependency",
                    ),
                    (data.peer_dependencies, "peer", "peer-dependency"),
                ] {
                    for (name, specifier) in group {
                        dependencies.push(NativeDependency {
                            name,
                            specifier,
                            kind: kind.to_string(),
                            ecosystem: Ecosystem::JavaScript,
                            native_kind: native_kind.to_string(),
                            source: source.clone(),
                        });
                    }
                }
            }
            Err(error) => diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                check: "package.json",
                message: error.message,
            }),
        }
    }
    let evidence = std::iter::once(EvidenceItem {
        kind: "manifest".to_string(),
        path: "package.json".to_string(),
    })
    .chain(lockfiles.iter().map(|path| EvidenceItem {
        kind: "lockfile".to_string(),
        path: path.clone(),
    }))
    .collect();
    Some(Component {
        path: relative.to_string(),
        ecosystem: Ecosystem::JavaScript,
        tool,
        manifests: vec!["package.json".to_string()],
        lockfiles: lockfiles.clone(),
        evidence,
        workspace_packages: Vec::new(),
        dependency_sources: lockfiles
            .iter()
            .map(|path| DependencySource {
                kind: "lockfile".to_string(),
                path: path.clone(),
            })
            .collect(),
    })
}

fn detect_javascript_manager(path: &Path) -> Option<String> {
    let package = fs::read_to_string(path.join("package.json")).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&package).ok()?;
    value
        .get("packageManager")
        .and_then(|value| value.as_str())
        .and_then(|value| {
            value
                .split('@')
                .next()
                .filter(|name| ["npm", "pnpm", "bun", "yarn"].contains(name))
                .map(str::to_string)
        })
        .or_else(|| {
            [
                ("bun.lock", "bun"),
                ("bun.lockb", "bun"),
                ("pnpm-lock.yaml", "pnpm"),
                ("yarn.lock", "yarn"),
                ("package-lock.json", "npm"),
            ]
            .iter()
            .find(|(file, _)| path.join(file).is_file())
            .map(|(_, name)| (*name).to_string())
        })
}

fn collect_project_dirs(path: &Path, roots: &mut Vec<PathBuf>) {
    roots.push(path.to_path_buf());
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !entry.file_name().to_string_lossy().starts_with('.')
                && entry.file_name() != "target"
                && entry.file_name() != "node_modules"
            {
                collect_project_dirs(&entry.path(), roots);
            }
        }
    }
}

fn detect_python_component(
    _root: &Path,
    path: &Path,
    relative: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Component> {
    let pyproject_present = path.join("pyproject.toml").is_file();
    let pipfile_present = path.join("Pipfile").is_file();
    let mut found = Vec::new();
    if pyproject_present {
        found.push("pyproject.toml".to_string());
    }
    if pipfile_present {
        found.push("Pipfile".to_string());
    }
    if let Ok(entries) = fs::read_dir(path) {
        let mut requirements = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                (name.starts_with("requirements")
                    && name.ends_with(".txt")
                    && entry.file_type().is_ok_and(|kind| kind.is_file()))
                .then_some(name)
            })
            .collect::<Vec<_>>();
        requirements.sort();
        found.extend(requirements);
    }
    if let Ok(entries) = fs::read_dir(path.join("requirements")) {
        let mut requirements = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                (name.ends_with(".txt") && entry.file_type().is_ok_and(|kind| kind.is_file()))
                    .then_some(name)
            })
            .collect::<Vec<_>>();
        requirements.sort();
        found.extend(
            requirements
                .into_iter()
                .map(|name| format!("requirements/{name}")),
        );
    }
    let lockfiles = ["uv.lock", "poetry.lock", "pdm.lock", "Pipfile.lock"]
        .iter()
        .filter(|name| path.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    if found.is_empty() && lockfiles.is_empty() {
        return None;
    }

    if pyproject_present && pipfile_present {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "Python manifests",
            message: format!(
                "{} declares both pyproject.toml and Pipfile; pyproject.toml is interpreted",
                project_label(relative)
            ),
        });
    }

    for manifest in &found {
        let source = join_project_path(relative, manifest);
        if manifest == "pyproject.toml" {
            if let Some(value) =
                read_python_manifest(&path.join(manifest), &source, "pyproject.toml", diagnostics)
            {
                parse_pyproject(&value, &source, dependencies, diagnostics);
            }
        } else if manifest == "Pipfile" {
            // pyproject.toml wins when both manifests exist; the warning above
            // records the ambiguity.
            if pyproject_present {
                continue;
            }
            if let Some(value) =
                read_python_manifest(&path.join(manifest), &source, "Pipfile", diagnostics)
            {
                parse_pipfile(&value, &source, dependencies);
            }
        } else {
            parse_requirements_file(&path.join(manifest), &source, dependencies, diagnostics);
        }
    }

    let tool = python_tool(path, pyproject_present, pipfile_present);
    let manifest_sources = found
        .iter()
        .map(|manifest| join_project_path(relative, manifest))
        .collect::<Vec<_>>();
    if manifest_sources.is_empty() {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "Python project",
            message: format!(
                "Python lockfile found without a manifest at {}",
                project_label(relative)
            ),
        });
    } else {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "Python project",
            message: format!(
                "{} found at {}",
                manifest_sources.join(", "),
                project_label(relative)
            ),
        });
    }
    diagnostics.push(Diagnostic {
        level: if lockfiles.is_empty() {
            DiagnosticLevel::Warn
        } else {
            DiagnosticLevel::Ok
        },
        message: if lockfiles.is_empty() {
            "No Python lockfile detected".to_string()
        } else {
            format!("Found {}", lockfiles.join(", "))
        },
        check: "Python lockfile",
    });

    let evidence = found
        .iter()
        .map(|path| EvidenceItem {
            kind: "manifest".to_string(),
            path: path.clone(),
        })
        .chain(lockfiles.iter().map(|path| EvidenceItem {
            kind: "lockfile".to_string(),
            path: path.clone(),
        }))
        .collect();
    Some(Component {
        path: relative.to_string(),
        ecosystem: Ecosystem::Python,
        tool: Some(tool.to_string()),
        manifests: found.clone(),
        lockfiles: lockfiles.clone(),
        evidence,
        workspace_packages: Vec::new(),
        dependency_sources: found
            .iter()
            .map(|path| DependencySource {
                kind: if path.ends_with(".txt") {
                    "requirements".to_string()
                } else {
                    "manifest".to_string()
                },
                path: path.clone(),
            })
            .chain(lockfiles.iter().map(|path| DependencySource {
                kind: "lockfile".to_string(),
                path: path.clone(),
            }))
            .collect(),
    })
}

fn project_label(relative: &str) -> String {
    if relative == "." {
        "the project root".to_string()
    } else {
        relative.to_string()
    }
}

fn read_python_manifest(
    path: &Path,
    source: &str,
    check: &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<toml::Value> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                check,
                message: format!("failed to read {source}: {error}"),
            });
            return None;
        }
    };
    match toml::from_str::<toml::Value>(&contents) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                check,
                message: format!("failed to parse {source}: {error}"),
            });
            None
        }
    }
}

fn python_tool(path: &Path, pyproject_present: bool, pipfile_present: bool) -> &'static str {
    if path.join("uv.lock").is_file() {
        "uv"
    } else if path.join("poetry.lock").is_file() {
        "poetry"
    } else if path.join("pdm.lock").is_file() {
        "pdm"
    } else if pipfile_present && !pyproject_present {
        // A Pipfile is interpreted only when no pyproject.toml competes with it.
        "pipenv"
    } else {
        "pip"
    }
}

fn parse_pyproject(
    value: &toml::Value,
    source: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(project) = value.get("project") {
        add_requirement_list(
            project.get("dependencies"),
            "dependency",
            "dependency",
            source,
            dependencies,
            diagnostics,
        );
        if let Some(groups) = project
            .get("optional-dependencies")
            .and_then(toml::Value::as_table)
        {
            for (group, requirements) in groups {
                add_requirement_list(
                    Some(requirements),
                    dependency_group_kind(group),
                    "optional-dependency",
                    source,
                    dependencies,
                    diagnostics,
                );
            }
        }
    }

    if let Some(groups) = value
        .get("dependency-groups")
        .and_then(toml::Value::as_table)
    {
        for (group, requirements) in groups {
            add_requirement_list(
                Some(requirements),
                dependency_group_kind(group),
                "dependency-group",
                source,
                dependencies,
                diagnostics,
            );
        }
    }

    if let Some(poetry) = value.get("tool").and_then(|tool| tool.get("poetry")) {
        add_poetry_dependencies(
            poetry.get("dependencies"),
            "dependency",
            source,
            dependencies,
        );
        add_poetry_dependencies(
            poetry.get("dev-dependencies"),
            "development",
            source,
            dependencies,
        );
        if let Some(groups) = poetry.get("group").and_then(toml::Value::as_table) {
            for (group, group_value) in groups {
                add_poetry_dependencies(
                    group_value.get("dependencies"),
                    dependency_group_kind(group),
                    source,
                    dependencies,
                );
            }
        }
    }

    if let Some(requires) = value
        .get("build-system")
        .and_then(|build_system| build_system.get("requires"))
    {
        add_requirement_list(
            Some(requires),
            "build",
            "build-requirement",
            source,
            dependencies,
            diagnostics,
        );
    }
}

fn add_requirement_list(
    value: Option<&toml::Value>,
    kind: &str,
    native_kind: &str,
    source: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(value) = value else {
        return;
    };
    let Some(entries) = value.as_array() else {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            check: "pyproject.toml",
            message: format!("{source}: expected an array of requirement strings"),
        });
        return;
    };
    for entry in entries {
        match entry {
            toml::Value::String(requirement) => match parse_requirement(requirement) {
                Some((name, specifier)) => dependencies.push(NativeDependency {
                    name,
                    specifier,
                    kind: kind.to_string(),
                    ecosystem: Ecosystem::Python,
                    native_kind: native_kind.to_string(),
                    source: source.to_string(),
                }),
                None => diagnostics.push(Diagnostic {
                    level: DiagnosticLevel::Warn,
                    check: "pyproject.toml",
                    message: format!("{source}: could not interpret requirement {requirement:?}"),
                }),
            },
            toml::Value::Table(table) if table.contains_key("include-group") => {
                diagnostics.push(Diagnostic {
                    level: DiagnosticLevel::Warn,
                    check: "pyproject.toml",
                    message: format!("{source}: dependency-group include entries are not expanded"),
                });
            }
            _ => diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Warn,
                check: "pyproject.toml",
                message: format!("{source}: unsupported dependency entry"),
            }),
        }
    }
}

fn add_poetry_dependencies(
    value: Option<&toml::Value>,
    kind: &str,
    source: &str,
    dependencies: &mut Vec<NativeDependency>,
) {
    let Some(table) = value.and_then(toml::Value::as_table) else {
        return;
    };
    for (name, value) in table {
        // `python` is the interpreter constraint, not a package.
        if name == "python" {
            continue;
        }
        dependencies.push(NativeDependency {
            name: name.clone(),
            specifier: poetry_specifier(value),
            kind: kind.to_string(),
            ecosystem: Ecosystem::Python,
            native_kind: "poetry".to_string(),
            source: source.to_string(),
        });
    }
}

fn poetry_specifier(value: &toml::Value) -> String {
    match value {
        toml::Value::String(specifier) => specifier.clone(),
        toml::Value::Table(table) => table
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or("*")
            .to_string(),
        _ => "*".to_string(),
    }
}

fn dependency_group_kind(name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    if ["dev", "test", "lint", "type", "doc"]
        .iter()
        .any(|marker| name.contains(marker))
    {
        "development"
    } else {
        "dependency"
    }
}

fn parse_pipfile(value: &toml::Value, source: &str, dependencies: &mut Vec<NativeDependency>) {
    for (section, kind) in [("packages", "dependency"), ("dev-packages", "development")] {
        let Some(table) = value.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, value) in table {
            let specifier = match value {
                toml::Value::String(specifier) => specifier.clone(),
                toml::Value::Table(table) => table
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("*")
                    .to_string(),
                _ => "*".to_string(),
            };
            dependencies.push(NativeDependency {
                name: name.clone(),
                specifier,
                kind: kind.to_string(),
                ecosystem: Ecosystem::Python,
                native_kind: "pipfile".to_string(),
                source: source.to_string(),
            });
        }
    }
}

fn parse_requirements_file(
    path: &Path,
    source: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                check: "requirements",
                message: format!("failed to read {source}: {error}"),
            });
            return;
        }
    };
    let kind = requirements_kind(source);
    let mut logical = String::new();
    let mut start_line = 0;
    for (index, raw) in contents.lines().enumerate() {
        if logical.is_empty() {
            start_line = index + 1;
        }
        let trimmed = raw.trim_end();
        if let Some(continued) = trimmed.strip_suffix('\\') {
            logical.push_str(continued.trim_end());
            logical.push(' ');
            continue;
        }
        logical.push_str(raw);
        let line = std::mem::take(&mut logical);
        parse_requirements_line(&line, start_line, source, kind, dependencies, diagnostics);
    }
    if !logical.is_empty() {
        parse_requirements_line(
            &logical,
            start_line,
            source,
            kind,
            dependencies,
            diagnostics,
        );
    }
}

fn parse_requirements_line(
    raw: &str,
    line_number: usize,
    source: &str,
    kind: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let line = strip_requirement_comment(raw).trim();
    if line.is_empty() || line.starts_with('-') {
        // Comments, blank lines, and pip options (`-r`, `-c`, `-e`, ...) are
        // not package declarations.
        return;
    }
    if line.contains("://")
        || ["git+", "hg+", "svn+", "bzr+", "file:"]
            .iter()
            .any(|prefix| line.starts_with(prefix))
    {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "requirements",
            message: format!("{source}:{line_number}: URL requirement is not interpreted: {line}"),
        });
        return;
    }
    match parse_requirement(line) {
        Some((name, specifier)) => dependencies.push(NativeDependency {
            name,
            specifier,
            kind: kind.to_string(),
            ecosystem: Ecosystem::Python,
            native_kind: "requirements".to_string(),
            source: source.to_string(),
        }),
        None => diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "requirements",
            message: format!("{source}:{line_number}: could not interpret requirement: {line}"),
        }),
    }
}

fn strip_requirement_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    let bytes = line.as_bytes();
    for index in 1..bytes.len() {
        if bytes[index] == b'#' && matches!(bytes[index - 1], b' ' | b'\t') {
            return &line[..index];
        }
    }
    line
}

fn requirements_kind(source: &str) -> &'static str {
    let file = source
        .rsplit('/')
        .next()
        .unwrap_or(source)
        .to_ascii_lowercase();
    if file.contains("dev") || file.contains("test") {
        "development"
    } else {
        "dependency"
    }
}

fn parse_requirement(input: &str) -> Option<(String, String)> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let name_length = input
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
        })
        .unwrap_or(input.len());
    let name = &input[..name_length];
    if name.is_empty() || !name.chars().next()?.is_ascii_alphanumeric() {
        return None;
    }
    let mut rest = input[name_length..].trim_start();
    if rest.starts_with('[') {
        let end = rest.find(']')?;
        rest = rest[end + 1..].trim_start();
    }
    let specifier = rest.split(';').next().unwrap_or("").trim();
    Some((
        name.to_string(),
        if specifier.is_empty() {
            "*".to_string()
        } else {
            specifier.to_string()
        },
    ))
}

fn declares_workspace_section(contents: &str) -> bool {
    contents.lines().any(|line| {
        let header = line.split('#').next().unwrap_or("").trim();
        header == "[workspace]"
    })
}

fn parse_toml_list(contents: &str, section_name: &str, key: &str) -> Vec<String> {
    let mut section = "";
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed.trim_matches(['[', ']'].as_ref());
        } else if section == section_name.trim_matches(['[', ']'].as_ref())
            && trimmed.starts_with(&format!("{key} ="))
        {
            return trimmed
                .split_once('=')
                .map(|(_, value)| value.split('#').next().unwrap_or("").trim())
                .unwrap_or_default()
                .trim_matches(['[', ']'].as_ref())
                .split(',')
                .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_string())
                .filter(|item| !item.is_empty())
                .collect();
        }
    }
    Vec::new()
}

fn detect_rust_component(
    _root: &Path,
    path: &Path,
    relative: &str,
    dependencies: &mut Vec<NativeDependency>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Component> {
    let manifest = path.join("Cargo.toml");
    if !manifest.is_file() {
        return None;
    }
    let lockfiles = ["Cargo.lock"]
        .iter()
        .filter(|name| path.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let source = join_project_path(relative, "Cargo.toml");
    let contents = fs::read_to_string(&manifest).unwrap_or_default();
    let parsed = toml::from_str::<toml::Value>(&contents).ok();
    if parsed.is_none() && relative != "." {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            check: "Cargo.toml",
            message: format!("failed to parse {source}"),
        });
    }
    let mut section = "other";
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let header = trimmed.split('#').next().unwrap_or("").trim();
            section = if header.contains("dev-dependencies") {
                "development"
            } else if header.contains("build-dependencies") {
                "build"
            } else if header == "[dependencies]" {
                "dependency"
            } else {
                "other"
            };
        } else if section != "other"
            && let Some((name, specifier)) = trimmed.split_once('=')
        {
            let name = name.trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && !name.starts_with('#')
            {
                dependencies.push(NativeDependency {
                    name: name.to_string(),
                    specifier: specifier.trim().to_string(),
                    kind: section.to_string(),
                    ecosystem: Ecosystem::Rust,
                    native_kind: section.to_string(),
                    source: source.clone(),
                });
            }
        }
    }
    let workspace_packages = parsed
        .as_ref()
        .and_then(|value| value.get("workspace"))
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| parse_toml_list(&contents, "[workspace]", "members"));
    let evidence = vec![EvidenceItem {
        kind: "manifest".to_string(),
        path: "Cargo.toml".to_string(),
    }]
    .into_iter()
    .chain(lockfiles.iter().map(|path| EvidenceItem {
        kind: "lockfile".to_string(),
        path: path.clone(),
    }))
    .collect();
    Some(Component {
        path: relative.to_string(),
        ecosystem: Ecosystem::Rust,
        tool: Some("cargo".to_string()),
        manifests: vec!["Cargo.toml".to_string()],
        lockfiles: lockfiles.clone(),
        evidence,
        workspace_packages,
        dependency_sources: lockfiles
            .iter()
            .map(|path| DependencySource {
                kind: "lockfile".to_string(),
                path: path.clone(),
            })
            .collect(),
    })
}

fn detect_container_component(
    _root: &Path,
    path: &Path,
    relative: &str,
    container: &mut Option<ContainerInfo>,
) -> Option<Component> {
    let dockerfiles = fs::read_dir(path)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (name == "Dockerfile" || name.starts_with("Dockerfile.")).then_some(name)
        })
        .collect::<Vec<_>>();
    let compose_files = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ]
    .iter()
    .filter(|name| path.join(name).is_file())
    .map(|name| name.to_string())
    .collect::<Vec<_>>();
    if dockerfiles.is_empty() && compose_files.is_empty() {
        return None;
    }
    let mut info = ContainerInfo {
        dockerfiles: dockerfiles.clone(),
        compose_files: compose_files.clone(),
        ..Default::default()
    };
    for file in &dockerfiles {
        let contents = fs::read_to_string(path.join(file)).unwrap_or_default();
        for line in contents.lines() {
            let mut parts = line.splitn(2, char::is_whitespace);
            let directive = parts.next().unwrap_or_default().to_uppercase();
            let value = parts.next().unwrap_or_default().trim();
            if [
                "FROM",
                "WORKDIR",
                "EXPOSE",
                "ENV",
                "ARG",
                "ENTRYPOINT",
                "CMD",
                "COPY",
                "ADD",
            ]
            .contains(&directive.as_str())
                && !value.is_empty()
            {
                info.directives
                    .entry(directive)
                    .or_default()
                    .push(value.to_string());
            }
        }
    }
    for file in &compose_files {
        let contents = fs::read_to_string(path.join(file)).unwrap_or_default();
        let (services, images) = parse_compose_file(&contents);
        for service in services {
            if !info.services.contains(&service) {
                info.services.push(service);
            }
        }
        info.images.extend(images);
    }
    *container = Some(info);
    let evidence = dockerfiles
        .iter()
        .map(|path| EvidenceItem {
            kind: "dockerfile".to_string(),
            path: path.clone(),
        })
        .chain(compose_files.iter().map(|path| EvidenceItem {
            kind: "compose".to_string(),
            path: path.clone(),
        }))
        .collect();
    Some(Component {
        path: relative.to_string(),
        ecosystem: Ecosystem::Container,
        tool: Some("docker".to_string()),
        manifests: dockerfiles.into_iter().chain(compose_files).collect(),
        lockfiles: Vec::new(),
        evidence,
        workspace_packages: Vec::new(),
        dependency_sources: Vec::new(),
    })
}

fn parse_compose_file(contents: &str) -> (Vec<String>, Vec<String>) {
    let mut services = Vec::new();
    let mut images = Vec::new();
    let mut in_services = false;
    let mut service_indent = None;
    for raw in contents.lines() {
        let line = strip_yaml_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t'))
            .count();
        if indent == 0 {
            in_services = trimmed == "services:";
            service_indent = None;
            continue;
        }
        if !in_services {
            continue;
        }
        if service_indent.is_none() {
            service_indent = Some(indent);
        }
        if let Some(image) = trimmed.strip_prefix("image:") {
            let image = image.trim().trim_matches(['"', '\'']);
            if !image.is_empty() {
                images.push(image.to_string());
            }
        }
        if Some(indent) == service_indent && trimmed.ends_with(':') && !trimmed.starts_with('-') {
            let name = trimmed
                .trim_end_matches(':')
                .trim()
                .trim_matches(['"', '\'']);
            if !name.is_empty() && !services.iter().any(|service| service == name) {
                services.push(name.to_string());
            }
        }
    }
    (services, images)
}

fn strip_yaml_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for index in 1..bytes.len() {
        if bytes[index] == b'#' && matches!(bytes[index - 1], b' ' | b'\t') {
            return &line[..index];
        }
    }
    line
}

fn read_package_json(path: &Path, label: &str) -> Result<PackageJsonData, CliError> {
    let contents = fs::read_to_string(path).map_err(|error| CliError {
        message: format!("failed to read {label}: {error}"),
        exit_code: 1,
    })?;

    let value = serde_json::from_str::<serde_json::Value>(&contents).map_err(|error| CliError {
        message: format!("failed to parse {label}: {error}"),
        exit_code: 1,
    })?;

    let name = value
        .get("name")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let package_manager_field = value
        .get("packageManager")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let has_workspaces = value.get("workspaces").is_some();
    let workspace_packages = value
        .get("workspaces")
        .and_then(|workspaces| {
            workspaces.as_array().or_else(|| {
                workspaces
                    .get("packages")
                    .and_then(serde_json::Value::as_array)
            })
        })
        .map(|packages| {
            packages
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let read_map = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_object)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|(name, version)| {
                        version
                            .as_str()
                            .map(|version| (name.clone(), version.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    Ok(PackageJsonData {
        name,
        package_manager_field,
        has_workspaces,
        dependencies: read_map("dependencies"),
        dev_dependencies: read_map("devDependencies"),
        optional_dependencies: read_map("optionalDependencies"),
        peer_dependencies: read_map("peerDependencies"),
        scripts: read_map("scripts"),
        workspace_packages,
    })
}

fn select_manager(
    package_json_data: Option<&PackageJsonData>,
    lockfiles: &[String],
) -> (Option<DetectedManager>, Vec<String>) {
    let package_manager_field = package_json_data
        .and_then(|data| data.package_manager_field.as_deref())
        .map(parse_package_manager_field);

    let lockfile_signals = lockfiles
        .iter()
        .filter_map(|lockfile| manager_from_lockfile(lockfile).map(|manager| (manager, lockfile)))
        .collect::<Vec<_>>();
    let mut notes = Vec::new();

    if let Some(Some((manager, version))) = package_manager_field {
        notes.push(format!(
            "Selected {manager} from package.json packageManager field."
        ));

        if lockfile_signals.len() > 1 {
            notes.push(format!(
                "Observed multiple lockfiles ({}); packageManager field takes precedence.",
                lockfiles.join(", ")
            ));
        }

        let matching_lockfile = manager
            .expected_lockfiles()
            .iter()
            .find_map(|expected| lockfiles.iter().find(|lockfile| lockfile == expected))
            .cloned();

        return (
            Some(DetectedManager {
                name: manager,
                version,
                lockfile: matching_lockfile,
                selected_by: "package.json packageManager".to_string(),
            }),
            notes,
        );
    }

    if matches!(package_manager_field, Some(None)) {
        notes.push(
            "Ignored unrecognized packageManager field and fell back to lockfile detection."
                .to_string(),
        );
    }

    for candidate in [
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("package-lock.json", PackageManager::Npm),
    ] {
        if lockfiles.iter().any(|lockfile| lockfile == candidate.0) {
            if lockfile_signals.len() > 1 {
                notes.push(format!(
                    "Selected {} using deterministic lockfile precedence over {}.",
                    candidate.1,
                    lockfiles.join(", ")
                ));
            } else {
                notes.push(format!("Selected {} from {}.", candidate.1, candidate.0));
            }

            return (
                Some(DetectedManager {
                    name: candidate.1,
                    version: None,
                    lockfile: Some(candidate.0.to_string()),
                    selected_by: "lockfile precedence".to_string(),
                }),
                notes,
            );
        }
    }

    notes.push("No supported package manager signals detected.".to_string());
    (None, notes)
}

fn parse_package_manager_field(value: &str) -> Option<(PackageManager, Option<String>)> {
    let (name, version) = match value.rsplit_once('@') {
        Some((name, version)) if !name.is_empty() => (name, Some(version.to_string())),
        _ => (value, None),
    };

    let manager = match name {
        "npm" => PackageManager::Npm,
        "pnpm" => PackageManager::Pnpm,
        "bun" => PackageManager::Bun,
        "yarn" => PackageManager::Yarn,
        _ => return None,
    };

    Some((manager, version))
}

fn manager_from_lockfile(lockfile: &str) -> Option<PackageManager> {
    match lockfile {
        "package-lock.json" => Some(PackageManager::Npm),
        "pnpm-lock.yaml" => Some(PackageManager::Pnpm),
        "bun.lock" | "bun.lockb" => Some(PackageManager::Bun),
        "yarn.lock" => Some(PackageManager::Yarn),
        _ => None,
    }
}

fn inspection_outcome(
    command: &str,
    detection: &RepositoryDetection,
    diagnostics: &[Diagnostic],
) -> Outcome {
    let has_any =
        !detection.components.is_empty() || detection.package_json || detection.cargo.is_some();
    if !has_any {
        return Outcome::Unsupported;
    }

    let any_javascript = detection
        .components
        .iter()
        .any(|component| component.ecosystem == Ecosystem::JavaScript);
    let any_javascript_or_rust = detection
        .components
        .iter()
        .any(|component| matches!(component.ecosystem, Ecosystem::JavaScript | Ecosystem::Rust));
    let any_python = detection
        .components
        .iter()
        .any(|component| component.ecosystem == Ecosystem::Python);
    let python_lockfile_count = detection
        .components
        .iter()
        .filter(|component| component.ecosystem == Ecosystem::Python)
        .flat_map(|component| component.lockfiles.iter())
        .count();
    let only_container = !detection.components.is_empty()
        && detection
            .components
            .iter()
            .all(|component| component.ecosystem == Ecosystem::Container);

    let command_outcome = match command {
        // Scripts are only observed for JavaScript manifests.
        "scripts" if !any_javascript => Some(Outcome::Unsupported),
        "scripts" if detection.components.len() > 1 => Some(Outcome::Partial),
        "deps" if only_container => Some(Outcome::Unsupported),
        // Python workspace membership is not observed.
        "workspaces" if !any_javascript_or_rust => Some(Outcome::Unsupported),
        "workspaces" if any_python => Some(Outcome::Partial),
        "lock" if !any_javascript_or_rust && !any_python => Some(Outcome::Unsupported),
        // Multiple Python lockfiles leave no single selected lockfile.
        "lock" if python_lockfile_count > 1 => Some(Outcome::Partial),
        _ => None,
    };
    if let Some(outcome) = command_outcome {
        return outcome;
    }

    if diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic.level, DiagnosticLevel::Error))
    {
        Outcome::Partial
    } else {
        Outcome::Ok
    }
}

fn build_output(
    command: &'static str,
    detection: RepositoryDetection,
    include_runtime: bool,
) -> CommandOutput {
    let diagnostics = build_diagnostics(&detection);
    let outcome = inspection_outcome(command, &detection, &diagnostics);
    let runtime = if include_runtime {
        detect_node_runtime()
    } else {
        None
    };
    let manager_name = detection.manager.as_ref().map(|manager| manager.name);
    let manager_version = detection
        .manager
        .as_ref()
        .and_then(|manager| manager.version.clone());
    let manager_lockfile = detection
        .manager
        .as_ref()
        .and_then(|manager| manager.lockfile.clone())
        .or_else(|| detection.lockfiles.first().cloned())
        .or_else(|| {
            detection
                .cargo
                .as_ref()
                .and_then(|cargo| cargo.lockfile.clone())
        });
    let python_lockfile = detection
        .components
        .iter()
        .find(|component| component.ecosystem == Ecosystem::Python)
        .and_then(|component| component.lockfiles.first().cloned());
    let lock_selected = manager_lockfile.clone().or(python_lockfile);
    let selected_by = detection
        .manager
        .as_ref()
        .map(|manager| manager.selected_by.clone());
    let summary = match manager_name {
        Some(manager) => format!(
            "Detected {} package reality for {}",
            manager, detection.project_name
        ),
        None if detection.cargo.is_some() => {
            format!(
                "Detected Cargo project reality for {}",
                detection.project_name
            )
        }
        None => format!(
            "Unable to determine a supported package manager for {}",
            detection.project_name
        ),
    };

    CommandOutput {
        schema: inspection_schema(command),
        schema_version: LEGACY_SCHEMA_VERSION,
        outcome,
        command,
        project: ProjectInfo {
            root: detection.root.display().to_string(),
            name: detection.project_name,
            package_json: detection.package_json,
            workspace: detection.workspace
                || detection
                    .cargo
                    .as_ref()
                    .is_some_and(|cargo| cargo.workspace),
            workspace_source: detection.workspace_source.clone().or_else(|| {
                detection
                    .cargo
                    .as_ref()
                    .filter(|cargo| cargo.workspace)
                    .map(|_| "Cargo.toml#[workspace]".to_string())
            }),
        },
        manager: ManagerInfo {
            name: manager_name
                .map(|manager| manager.to_string())
                .or_else(|| detection.cargo.as_ref().map(|_| "cargo".to_string())),
            version: manager_version,
            lockfile: manager_lockfile.clone(),
            selected_by,
        },
        result: CommandResult { summary, runtime },
        diagnostics,
        evidence: Evidence {
            package_manager_field: detection
                .package_json_data
                .as_ref()
                .and_then(|data| data.package_manager_field.clone()),
            lockfiles: detection
                .lockfiles
                .iter()
                .cloned()
                .chain(
                    detection
                        .cargo
                        .iter()
                        .filter_map(|cargo| cargo.lockfile.clone()),
                )
                .collect(),
            workspace_files: detection.workspace_files,
            selection_notes: detection.selection_notes,
        },
        dependencies: (command == "deps").then(|| {
            let data = detection
                .package_json_data
                .clone()
                .unwrap_or(PackageJsonData {
                    name: None,
                    package_manager_field: None,
                    has_workspaces: false,
                    dependencies: BTreeMap::new(),
                    dev_dependencies: BTreeMap::new(),
                    optional_dependencies: BTreeMap::new(),
                    peer_dependencies: BTreeMap::new(),
                    scripts: BTreeMap::new(),
                    workspace_packages: Vec::new(),
                });
            DependencyGroups {
                dependencies: data.dependencies,
                dev_dependencies: data.dev_dependencies,
                optional_dependencies: data.optional_dependencies,
                peer_dependencies: data.peer_dependencies,
                native_dependencies: detection.native_dependencies.clone(),
            }
        }),
        scripts: (command == "scripts").then(|| {
            detection
                .package_json_data
                .clone()
                .map(|data| data.scripts)
                .unwrap_or_default()
        }),
        workspaces: (command == "workspaces").then(|| WorkspaceOutput {
            enabled: detection.workspace
                || detection
                    .cargo
                    .as_ref()
                    .is_some_and(|cargo| cargo.workspace)
                || detection
                    .components
                    .iter()
                    .any(|component| !component.workspace_packages.is_empty()),
            source: detection
                .workspace_source
                .clone()
                .or_else(|| {
                    detection.cargo.as_ref().and_then(|cargo| {
                        cargo
                            .workspace
                            .then(|| "Cargo.toml#[workspace]".to_string())
                    })
                })
                .or_else(|| {
                    detection
                        .components
                        .iter()
                        .find(|component| !component.workspace_packages.is_empty())
                        .map(|_| "Cargo.toml#[workspace]".to_string())
                }),
            packages: detection
                .workspace_packages
                .iter()
                .cloned()
                .chain(
                    detection
                        .cargo
                        .iter()
                        .filter(|cargo| cargo.workspace)
                        .flat_map(|cargo| cargo.workspace_members.iter().cloned()),
                )
                .chain(
                    detection
                        .components
                        .iter()
                        .flat_map(|component| component.workspace_packages.iter().cloned()),
                )
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        }),
        lock: (command == "lock").then(|| LockOutput {
            found: lock_selected.is_some(),
            files: detection
                .lockfiles
                .iter()
                .cloned()
                .chain(
                    detection
                        .cargo
                        .iter()
                        .filter_map(|cargo| cargo.lockfile.clone()),
                )
                .chain(
                    detection
                        .components
                        .iter()
                        .filter(|component| component.ecosystem == Ecosystem::Python)
                        .flat_map(|component| component.lockfiles.iter().cloned()),
                )
                .collect(),
            selected: lock_selected,
            manager: manager_name,
        }),
        ecosystem: {
            let mut ecosystems = detection
                .components
                .iter()
                .map(|component| component.ecosystem);
            let first = ecosystems.next();
            first.filter(|ecosystem| ecosystems.all(|candidate| candidate == *ecosystem))
        },
        components: detection.components.clone(),
        native_dependencies: detection.native_dependencies.clone(),
        container: detection.container.clone(),
        cargo: detection.cargo.clone(),
    }
}

fn build_diagnostics(detection: &RepositoryDetection) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(cargo) = &detection.cargo {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "Cargo project",
            message: format!("{} found", cargo.manifest),
        });
        diagnostics.push(Diagnostic {
            level: if cargo.lockfile.is_some() {
                DiagnosticLevel::Ok
            } else {
                DiagnosticLevel::Warn
            },
            check: "Cargo lockfile",
            message: cargo
                .lockfile
                .as_ref()
                .map(|lockfile| format!("Found {lockfile}"))
                .unwrap_or_else(|| "No Cargo.lock detected".to_string()),
        });
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "Cargo workspace",
            message: if cargo.workspace {
                format!(
                    "Workspace detected at {} with {} member(s)",
                    cargo.workspace_root,
                    cargo.workspace_members.len()
                )
            } else {
                "Single-package Cargo project detected".to_string()
            },
        });
    }

    if detection.package_json {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "package.json",
            message: "package.json found".to_string(),
        });

        diagnostics.push(Diagnostic {
            level: if detection.lockfiles.is_empty() {
                DiagnosticLevel::Warn
            } else {
                DiagnosticLevel::Ok
            },
            check: "lockfile",
            message: if detection.lockfiles.is_empty() {
                "No supported lockfile detected".to_string()
            } else {
                format!("Found {}", detection.lockfiles.join(", "))
            },
        });
    }

    let nested_javascript = detection
        .components
        .iter()
        .filter(|component| component.ecosystem == Ecosystem::JavaScript && component.path != ".")
        .map(|component| join_project_path(&component.path, "package.json"))
        .collect::<Vec<_>>();
    if !nested_javascript.is_empty() {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "JavaScript projects",
            message: format!("package.json found at {}", nested_javascript.join(", ")),
        });
    }

    let package_manager_field = detection
        .package_json_data
        .as_ref()
        .and_then(|data| data.package_manager_field.as_deref())
        .and_then(parse_package_manager_field);
    let matching_lockfile = package_manager_field.as_ref().and_then(|(manager, _)| {
        manager
            .expected_lockfiles()
            .iter()
            .find_map(|expected| {
                detection
                    .lockfiles
                    .iter()
                    .find(|lockfile| lockfile == expected)
            })
            .cloned()
    });

    let package_lock_consistency = match (
        package_manager_field,
        detection.lockfiles.is_empty(),
        matching_lockfile,
    ) {
        (Some((manager, _)), _, Some(lockfile)) => Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "package/lock consistency",
            message: format!("{manager} agrees with {lockfile}"),
        },
        (Some((manager, _)), false, None) => Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "package/lock consistency",
            message: format!(
                "packageManager selects {manager}, but detected {}",
                detection.lockfiles.join(", ")
            ),
        },
        (Some((manager, _)), true, None) => Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "package/lock consistency",
            message: format!("packageManager selects {manager}, but no lockfile was found"),
        },
        (None, false, _) => Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "package/lock consistency",
            message: "Manager inferred from lockfile only".to_string(),
        },
        (None, true, _) => Diagnostic {
            level: DiagnosticLevel::Warn,
            check: "package/lock consistency",
            message: "Not enough evidence to verify package/lock consistency".to_string(),
        },
    };
    if detection.package_json {
        diagnostics.push(package_lock_consistency);
    }

    if detection.package_json {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            check: "workspace configuration",
            message: detection
                .workspace_source
                .as_ref()
                .map(|source| format!("Workspace detected via {source}"))
                .unwrap_or_else(|| "No workspace configuration detected".to_string()),
        });
    }

    for component in &detection.components {
        if component.ecosystem == Ecosystem::Python && component.lockfiles.len() > 1 {
            diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Warn,
                check: "python lockfiles",
                message: format!(
                    "Python project has multiple lockfiles: {}",
                    component.lockfiles.join(", ")
                ),
            });
        }
    }

    diagnostics.extend(detection.detection_diagnostics.iter().cloned());

    diagnostics
}

fn detect_node_runtime() -> Option<String> {
    None
}

fn render_human(output: &CommandOutput) -> String {
    let heading = match output.command {
        "info" => "PAX Info",
        "doctor" => "PAX Doctor",
        "deps" => "PAX Dependencies",
        "scripts" => "PAX Scripts",
        "workspaces" => "PAX Workspaces",
        "lock" => "PAX Lock",
        "graph" => "PAX Graph",
        "reality" => "PAX Reality",
        "drift" => "PAX Drift",
        _ => "PAX",
    };
    let manager = output
        .manager
        .name
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let manager = match &output.manager.version {
        Some(version) => format!("{manager} {version}"),
        None => manager,
    };
    let workspace = match &output.project.workspace_source {
        Some(source) => format!("yes ({source})"),
        None => "no".to_string(),
    };
    let lockfile = output.manager.lockfile.as_deref().unwrap_or("missing");

    let mut lines = vec![
        heading.to_string(),
        format!("Project       {}", output.project.name),
        format!("Root          {}", output.project.root),
        format!("Manager       {manager}"),
        format!("Workspace     {workspace}"),
        format!("Lockfile      {lockfile}"),
    ];
    if let Some(cargo) = &output.cargo {
        lines.push(format!("Cargo root    {}", cargo.workspace_root));
        lines.push(format!("Cargo members {}", cargo.workspace_members.len()));
    }
    if !output.components.is_empty() {
        let ecosystems = output
            .components
            .iter()
            .map(|component| format!("{:?}", component.ecosystem).to_lowercase())
            .collect::<Vec<_>>();
        lines.push(format!("Ecosystems    {}", ecosystems.join(", ")));
        lines.push(format!("Components    {}", output.components.len()));
    }

    if let Some(selected_by) = &output.manager.selected_by {
        lines.push(format!("Selected by   {selected_by}"));
    }

    if let Some(runtime) = &output.result.runtime {
        lines.push(format!("Runtime       Node {runtime}"));
    }

    lines.push(String::new());
    if let Some(dependencies) = &output.dependencies {
        lines.push(format!(
            "Dependencies {}",
            dependencies.dependencies.len()
                + dependencies.dev_dependencies.len()
                + dependencies.optional_dependencies.len()
                + dependencies.peer_dependencies.len()
                + dependencies.native_dependencies.len()
        ));
    }
    if let Some(scripts) = &output.scripts {
        lines.push(format!("Scripts      {}", scripts.len()));
    }
    if let Some(workspaces) = &output.workspaces {
        lines.push(format!("Packages     {}", workspaces.packages.len()));
        lines.extend(
            workspaces
                .packages
                .iter()
                .map(|package| format!("  - {package}")),
        );
    }
    if let Some(lock) = &output.lock {
        lines.push(format!("Lockfiles    {}", lock.files.join(", ")));
    }
    lines.extend(output.diagnostics.iter().map(|diagnostic| {
        format!(
            "{} {} {}",
            diagnostic.level.symbol(),
            diagnostic.check,
            diagnostic.message
        )
    }));

    if !output.evidence.selection_notes.is_empty() {
        lines.push(String::new());
        lines.push("Evidence".to_string());
        lines.extend(
            output
                .evidence
                .selection_notes
                .iter()
                .map(|note| format!("  - {note}")),
        );
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let unique = format!(
            "pax-tests-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
        );
        let dir = env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn package_manager_field_wins_over_conflicting_lockfile() {
        let root = temp_dir();
        write(
            &root.join("package.json"),
            r#"{"name":"demo","packageManager":"pnpm@10.15.0"}"#,
        );
        write(&root.join("package-lock.json"), "{}");

        let detection = detect_repository(&root).unwrap();
        let manager = detection.manager.unwrap();

        assert_eq!(manager.name, PackageManager::Pnpm);
        assert_eq!(manager.version.as_deref(), Some("10.15.0"));
        assert_eq!(manager.selected_by, "package.json packageManager");
    }

    #[test]
    fn workspace_detection_uses_pnpm_workspace_file() {
        let root = temp_dir();
        write(&root.join("package.json"), r#"{"name":"demo"}"#);
        write(
            &root.join("pnpm-workspace.yaml"),
            "packages:\n  - packages/*\n",
        );

        let detection = detect_repository(&root).unwrap();

        assert!(detection.workspace);
        assert_eq!(
            detection.workspace_source.as_deref(),
            Some("pnpm-workspace.yaml")
        );
    }

    #[test]
    fn lockfile_precedence_is_deterministic_without_package_manager_field() {
        let root = temp_dir();
        write(&root.join("package.json"), r#"{"name":"demo"}"#);
        write(&root.join("yarn.lock"), "");
        write(&root.join("package-lock.json"), "{}");

        let detection = detect_repository(&root).unwrap();
        let manager = detection.manager.unwrap();

        assert_eq!(manager.name, PackageManager::Yarn);
        assert_eq!(manager.lockfile.as_deref(), Some("yarn.lock"));
        assert_eq!(manager.selected_by, "lockfile precedence");
    }

    #[test]
    fn info_json_includes_schema_and_manager() {
        let root = temp_dir();
        write(
            &root.join("package.json"),
            r#"{"name":"demo","packageManager":"bun@1.2.0","workspaces":["packages/*"]}"#,
        );
        write(&root.join("bun.lock"), "");

        let output = build_output("info", detect_repository(&root).unwrap(), false);
        let value = serde_json::to_value(output).unwrap();

        assert_eq!(value["schemaVersion"], json!("1"));
        assert_eq!(value["manager"]["name"], json!("bun"));
        assert_eq!(value["project"]["workspace"], json!(true));
        assert_eq!(value["manager"]["lockfile"], json!("bun.lock"));
    }

    #[test]
    fn compose_services_are_direct_children_only() {
        let (services, images) = parse_compose_file(
            "services:\n  web: # web\n    image: nginx:1\n    ports:\n      - \"80:80\"\n    environment:\n      DEBUG: \"1\"\n  db:\n    image: postgres:16\nvolumes:\n  data:\n    driver: local\n",
        );
        assert_eq!(services, vec!["web", "db"]);
        assert_eq!(images, vec!["nginx:1", "postgres:16"]);
    }

    #[test]
    fn runtime_timestamps_are_rfc3339_utc() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc_timestamp(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn compose_ps_parses_arrays_and_json_lines() {
        let array = parse_compose_ps(
            r#"[{"Service":"web","State":"running"},{"Service":"db","State":"exited"}]"#,
        )
        .unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array[0].service, "web");
        assert!(array[0].running());
        assert_eq!(array[1].service, "db");
        assert!(!array[1].running());
        assert_eq!(array[1].state_label(), "exited");

        let lines = parse_compose_ps(
            "{\"Service\":\"web\",\"Status\":\"Up 2 minutes\"}\n{\"service\":\"db\",\"status\":\"exited\"}\n",
        )
        .unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].running());
        assert!(!lines[1].running());
        assert!(parse_compose_ps("[]").unwrap().is_empty());
        assert!(parse_compose_ps("").unwrap().is_empty());
        assert!(parse_compose_ps("not json").is_err());
    }

    #[test]
    fn requirements_keep_specifiers_and_drop_extras_and_markers() {
        assert_eq!(
            parse_requirement("requests[socks]>=2 ; python_version >= \"3.9\""),
            Some(("requests".to_string(), ">=2".to_string()))
        );
        assert_eq!(
            parse_requirement("flask"),
            Some(("flask".to_string(), "*".to_string()))
        );
        assert_eq!(
            strip_requirement_comment("flask>=3  # runtime").trim(),
            "flask>=3"
        );
        assert_eq!(strip_requirement_comment("# whole line"), "");
    }

    #[test]
    fn python_tool_prefers_pyproject_over_pipfile() {
        let root = temp_dir();
        write(&root.join("Pipfile"), "");
        write(&root.join("Pipfile.lock"), "{}");
        assert_eq!(python_tool(&root, false, true), "pipenv");
        // pyproject.toml wins the ambiguity, so pipenv is not selected.
        assert_eq!(python_tool(&root, true, true), "pip");
    }

    #[test]
    fn python_distribution_metadata_names_are_extracted() {
        assert_eq!(
            distribution_name("typing_extensions-4.12.0"),
            "typing_extensions"
        );
        assert_eq!(
            distribution_name("zope.interface-6.0-py3.12"),
            "zope.interface"
        );
        assert_eq!(distribution_name("__editable__.svc-0.1.0"), "svc");
        assert_eq!(distribution_name("localpackage"), "localpackage");
    }

    #[test]
    fn python_distribution_names_follow_pep_503() {
        assert_eq!(
            normalize_python_name("typing_extensions"),
            "typing-extensions"
        );
        assert_eq!(normalize_python_name("zope.interface"), "zope-interface");
        assert_eq!(normalize_python_name("Foo__Bar..baz"), "foo-bar-baz");
        assert_eq!(normalize_python_name("requests"), "requests");
    }

    #[test]
    fn cargo_workspace_headers_ignore_trailing_comments() {
        assert!(declares_workspace_section(
            "[workspace] # root workspace\nmembers = [\"crates/app\"]\n"
        ));
        assert!(!declares_workspace_section(
            "[package]\nname = \"single\"\n"
        ));
        assert_eq!(
            parse_toml_list(
                "[workspace]\nmembers = [\"crates/app\", \"crates/lib\"] # members\n",
                "[workspace]",
                "members"
            ),
            vec!["crates/app", "crates/lib"]
        );
    }
}
