use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    schema_version: &'static str,
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
    schema_version: &'static str,
    command: &'static str,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    evidence: Vec<GraphEvidence>,
    cargo: Option<CargoObservation>,
}

#[derive(Clone, Debug, Serialize)]
struct RealityObservation {
    subject: String,
    status: String,
    source: String,
    evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct RealityLayer {
    observations: Vec<RealityObservation>,
}

#[derive(Clone, Debug, Serialize)]
struct RealityOutput {
    schema_version: &'static str,
    command: &'static str,
    live: bool,
    declared: RealityLayer,
    resolved: RealityLayer,
    installed: RealityLayer,
    runtime: RealityLayer,
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
    schema_version: &'static str,
    command: &'static str,
    live: bool,
    status: String,
    issues: Vec<DriftItem>,
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
    let cwd = env::current_dir().map_err(|error| CliError {
        message: format!("failed to determine current directory: {error}"),
        exit_code: 1,
    })?;
    let working_directory = cli
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
        .unwrap_or(cwd);

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
    if cli.tool.is_none()
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
        )
        && detection.package_json
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
            > 1
        || cli.tool.is_none()
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
            )
            && detection.components.iter().any(|component| {
                component.ecosystem == Ecosystem::Python && component.lockfiles.len() > 1
            })
    {
        return Err(CliError {
            message: if detection.components.iter().any(|component| {
                component.ecosystem == Ecosystem::Python && component.lockfiles.len() > 1
            }) {
                "ambiguous Python toolchain: multiple lockfiles detected; use --tool uv, --tool poetry, --tool pdm, or --tool pip".to_string()
            } else {
                format!(
                    "multiple JavaScript package managers detected: {}\nuse --tool npm, --tool pnpm, --tool yarn, or --tool bun",
                    detection.lockfiles.join(", ")
                )
            },
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
        CommandName::Graph | CommandName::Reality | CommandName::Drift => unreachable!(),
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
        for dependency in detection
            .native_dependencies
            .iter()
            .filter(|dependency| dependency.ecosystem == component.ecosystem)
        {
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
                evidence: component
                    .manifests
                    .first()
                    .map(|manifest| {
                        if component.path == "." {
                            manifest.clone()
                        } else {
                            format!("{}/{}", component.path, manifest)
                        }
                    })
                    .into_iter()
                    .collect(),
            });
        }
    }
    if let Some(cargo) = &detection.cargo {
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
    edges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    edges.dedup_by(|a, b| a.from == b.from && a.to == b.to && a.kind == b.kind);
    evidence.sort_by(|a, b| a.edge.cmp(&b.edge));
    GraphOutput {
        schema_version: "1",
        command: "graph",
        nodes,
        edges,
        evidence,
        cargo: detection.cargo.clone(),
    }
}

fn build_reality(detection: &RepositoryDetection, live: bool) -> RealityOutput {
    let declared = detection
        .components
        .iter()
        .map(|component| RealityObservation {
            subject: component.path.clone(),
            status: "present".to_string(),
            source: "filesystem".to_string(),
            evidence: component
                .manifests
                .iter()
                .map(|path| {
                    if component.path == "." {
                        path.clone()
                    } else {
                        format!("{}/{}", component.path, path)
                    }
                })
                .collect(),
        })
        .collect();
    let resolved = detection
        .components
        .iter()
        .flat_map(|component| {
            component.lockfiles.iter().map(|path| RealityObservation {
                subject: component.path.clone(),
                status: "present".to_string(),
                source: "filesystem".to_string(),
                evidence: vec![if component.path == "." {
                    path.clone()
                } else {
                    format!("{}/{}", component.path, path)
                }],
            })
        })
        .collect();
    let installed = detection
        .components
        .iter()
        .map(|component| {
            let path = detection.root.join(&component.path);
            let (status, evidence) = match component.ecosystem {
                Ecosystem::JavaScript => {
                    let installed = path.join("node_modules").is_dir();
                    (
                        if installed { "present" } else { "absent" },
                        vec!["node_modules/".to_string()],
                    )
                }
                Ecosystem::Rust => {
                    if path.join("target").is_dir() {
                        ("present", vec!["target/".to_string()])
                    } else {
                        ("unknown", Vec::new())
                    }
                }
                Ecosystem::Python => {
                    let environment = [".venv", "venv", ".env"]
                        .iter()
                        .find(|name| path.join(name).is_dir());
                    match environment {
                        Some(name) => ("present", vec![format!("{name}/")]),
                        None => ("unknown", Vec::new()),
                    }
                }
                Ecosystem::Container => ("unknown", Vec::new()),
            };
            RealityObservation {
                subject: component.path.clone(),
                status: status.to_string(),
                source: "filesystem".to_string(),
                evidence,
            }
        })
        .collect();
    let runtime = if live {
        detection
            .components
            .iter()
            .filter(|component| component.ecosystem == Ecosystem::Container)
            .map(|component| RealityObservation {
                subject: component.path.clone(),
                status: "unknown".to_string(),
                source: "docker".to_string(),
                evidence: vec!["docker compose ps".to_string()],
            })
            .collect()
    } else {
        Vec::new()
    };
    RealityOutput {
        schema_version: "1",
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
        runtime: RealityLayer {
            observations: runtime,
        },
        cargo: detection.cargo.clone(),
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
    if let Some(manager) = package_manager {
        if js_locks.iter().any(|(found, _)| *found != manager) {
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
    } else if js_locks
        .iter()
        .map(|(manager, _)| *manager)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        > 1
    {
        issues.push(DriftItem {
            subject: ".".to_string(),
            status: "ambiguous".to_string(),
            expected: "one JavaScript package-manager authority".to_string(),
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
                .collect(),
        });
    }
    if let (Some(data), Some(lockfile)) = (
        detection.package_json_data.as_ref(),
        detection
            .manager
            .as_ref()
            .and_then(|manager| manager.lockfile.as_ref()),
    ) {
        let contents = fs::read_to_string(detection.root.join(lockfile)).unwrap_or_default();
        for name in data.dependencies.keys() {
            if !contents.contains(name) {
                issues.push(DriftItem {
                    subject: format!("./{name}"),
                    status: "drift".to_string(),
                    expected: "declared dependency in resolved lockfile".to_string(),
                    actual: format!("{name} missing from {lockfile}"),
                    evidence: vec![
                        DriftEvidence {
                            source: "package.json".to_string(),
                            kind: "declared".to_string(),
                        },
                        DriftEvidence {
                            source: lockfile.clone(),
                            kind: "resolved".to_string(),
                        },
                    ],
                });
            }
        }
    }
    for component in &detection.components {
        if component.ecosystem == Ecosystem::Python && component.lockfiles.len() > 1 {
            issues.push(DriftItem {
                subject: component.path.clone(),
                status: "ambiguous".to_string(),
                expected: "one Python resolution authority".to_string(),
                actual: component.lockfiles.join(", "),
                evidence: component
                    .lockfiles
                    .iter()
                    .map(|source| DriftEvidence {
                        source: format!("{}/{}", component.path, source),
                        kind: "resolved".to_string(),
                    })
                    .collect(),
            });
        }
        if component.ecosystem == Ecosystem::JavaScript
            && !detection
                .root
                .join(&component.path)
                .join("node_modules")
                .is_dir()
        {
            issues.push(DriftItem {
                subject: component.path.clone(),
                status: "drift".to_string(),
                expected: "installed dependencies".to_string(),
                actual: "node_modules/ absent".to_string(),
                evidence: vec![
                    DriftEvidence {
                        source: "package.json".to_string(),
                        kind: "declared".to_string(),
                    },
                    DriftEvidence {
                        source: "node_modules/".to_string(),
                        kind: "installed".to_string(),
                    },
                ],
            });
        }
    }
    if live {
        for component in &detection.components {
            if component.ecosystem == Ecosystem::Container {
                issues.push(DriftItem {
                    subject: component.path.clone(),
                    status: "unknown".to_string(),
                    expected: "declared runtime services".to_string(),
                    actual: "runtime not established".to_string(),
                    evidence: vec![DriftEvidence {
                        source: "docker compose ps".to_string(),
                        kind: "runtime".to_string(),
                    }],
                });
            }
        }
    }
    issues.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.status.cmp(&b.status)));
    DriftOutput {
        schema_version: "1",
        command: "drift",
        live,
        status: if issues.iter().any(|issue| issue.status == "drift") {
            "drift".to_string()
        } else if issues.is_empty() {
            "match".to_string()
        } else {
            "ambiguous".to_string()
        },
        issues,
        cargo: detection.cargo.clone(),
    }
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

fn render_reality(output: &RealityOutput) -> String {
    let count = |layer: &RealityLayer| layer.observations.len();
    format!(
        "PROJECT\n  {} components\nDECLARED\n  {}\nRESOLVED\n  {}\nINSTALLED\n  {}\nRUNTIME\n  {}",
        count(&output.declared),
        count(&output.declared),
        count(&output.resolved),
        count(&output.installed),
        if output.live {
            "inspected"
        } else {
            "not inspected"
        }
    )
}

fn render_drift(output: &DriftOutput) -> String {
    if output.issues.is_empty() {
        return "NO DRIFT".to_string();
    }
    let mut lines = vec![format!("{} issues", output.issues.len())];
    lines.extend(
        output
            .issues
            .iter()
            .map(|issue| format!("  {}\n    {}", issue.subject, issue.actual)),
    );
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
    })
}

fn usage() -> String {
    r#"PAX is a universal, read-only project-tooling boundary.

Usage: pax [OPTIONS] COMMAND [ARGS...]

Inspection:
  info, doctor, deps, scripts, workspaces, lock
  graph, reality, drift
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
    let cargo = detect_cargo(root);
    let package_json_path = root.join("package.json");
    let package_json_data = if package_json_path.is_file() {
        Some(read_package_json(&package_json_path)?)
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
    let (mut components, mut native_dependencies, container) = detect_components(root);
    if let Some(cargo) = &cargo {
        if let Some(component) = components
            .iter_mut()
            .find(|component| component.ecosystem == Ecosystem::Rust && component.path == ".")
        {
            component.workspace_packages = cargo.workspace_members.clone();
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
    })
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .and_then(|path| path.to_str())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| path.to_str().unwrap_or("."))
        .to_string()
}

fn detect_cargo(root: &Path) -> Option<CargoObservation> {
    let manifest_path = root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return None;
    }
    let manifest_contents = fs::read_to_string(&manifest_path).unwrap_or_default();
    let declares_workspace = manifest_contents
        .lines()
        .any(|line| line.trim() == "[workspace]");
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
        let mut dependencies = metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|package| package["dependencies"].as_array().into_iter().flatten())
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
                })
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then(a.kind.cmp(&b.kind))
                .then(a.specifier.cmp(&b.specifier))
        });
        dependencies
            .dedup_by(|a, b| a.name == b.name && a.kind == b.kind && a.specifier == b.specifier);
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
            source: "cargo metadata --no-deps --offline".to_string(),
        });
    }

    let package_name = manifest_contents.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "name").then(|| value.trim().trim_matches(['\"', '\'']).to_string())
    });
    let members = parse_toml_list(&manifest_contents, "[workspace]", "members");
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
        source: "Cargo.toml fallback".to_string(),
    })
}

fn detect_components(
    root: &Path,
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
        if let Some(component) = detect_python_component(root, &path, &relative, &mut dependencies)
        {
            components.push(component);
        }
        if let Some(component) = detect_rust_component(root, &path, &relative, &mut dependencies) {
            components.push(component);
        }
        if let Some(component) = detect_container_component(root, &path, &relative, &mut container)
        {
            components.push(component);
        }
    }
    if root.join("package.json").is_file() {
        let manager = detect_repository_manager_name(root);
        let lockfiles = [
            "bun.lock",
            "bun.lockb",
            "pnpm-lock.yaml",
            "yarn.lock",
            "package-lock.json",
        ]
        .iter()
        .filter(|name| root.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
        let component = Component {
            path: ".".to_string(),
            ecosystem: Ecosystem::JavaScript,
            tool: manager,
            manifests: vec!["package.json".to_string()],
            lockfiles: lockfiles.clone(),
            evidence: std::iter::once(EvidenceItem {
                kind: "manifest".to_string(),
                path: "package.json".to_string(),
            })
            .chain(lockfiles.iter().map(|path| EvidenceItem {
                kind: "lockfile".to_string(),
                path: path.clone(),
            }))
            .collect(),
            workspace_packages: Vec::new(),
            dependency_sources: lockfiles
                .iter()
                .map(|path| DependencySource {
                    kind: "lockfile".to_string(),
                    path: path.clone(),
                })
                .collect(),
        };
        components.insert(0, component);
    }

    fn detect_repository_manager_name(root: &Path) -> Option<String> {
        let package = fs::read_to_string(root.join("package.json")).ok()?;
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
                .find(|(file, _)| root.join(file).is_file())
                .map(|(_, name)| (*name).to_string())
            })
    }
    components.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| format!("{:?}", a.ecosystem).cmp(&format!("{:?}", b.ecosystem)))
    });
    (components, dependencies, container)
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
    root: &Path,
    path: &Path,
    relative: &str,
    dependencies: &mut Vec<NativeDependency>,
) -> Option<Component> {
    let manifests = ["pyproject.toml", "requirements.txt", "Pipfile"];
    let mut found = manifests
        .iter()
        .filter(|name| path.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    if let Ok(entries) = fs::read_dir(path.join("requirements")) {
        found.extend(entries.flatten().filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (name.ends_with(".txt")).then_some(format!("requirements/{name}"))
        }));
    }
    let lockfiles = ["uv.lock", "poetry.lock", "pdm.lock", "Pipfile.lock"]
        .iter()
        .filter(|name| path.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    if found.is_empty() && lockfiles.is_empty() && !path.join("requirements").is_dir() {
        return None;
    }
    let tool = if path.join("uv.lock").is_file() {
        "uv"
    } else if path.join("poetry.lock").is_file() {
        "poetry"
    } else if path.join("pdm.lock").is_file() {
        "pdm"
    } else {
        "pip"
    };
    if let Ok(contents) = fs::read_to_string(path.join("pyproject.toml")) {
        let mut kind = "dependency";
        for line in contents.lines() {
            let trimmed = line.trim();
            if let Some((name, specifier)) = trimmed.split_once('=') {
                let name = name.trim().trim_matches('"').trim_matches('\'');
                if name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && !name.is_empty()
                    && (specifier.contains('"') || specifier.contains('\''))
                {
                    dependencies.push(NativeDependency {
                        name: name.to_string(),
                        specifier: specifier
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string(),
                        kind: kind.to_string(),
                        ecosystem: Ecosystem::Python,
                        native_kind: if trimmed.contains("optional") {
                            "optional-dependency".to_string()
                        } else {
                            "dependency-group".to_string()
                        },
                    });
                }
            }
            for manifest in &found {
                if manifest.ends_with(".txt") {
                    let contents = fs::read_to_string(path.join(manifest)).unwrap_or_default();
                    for line in contents
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    {
                        let name = line
                            .split(['=', '<', '>', '!', ';'])
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !name.is_empty() {
                            dependencies.push(NativeDependency {
                                name: name.to_string(),
                                specifier: line[name.len()..].trim().to_string(),
                                kind: "dependency".to_string(),
                                ecosystem: Ecosystem::Python,
                                native_kind: "requirements".to_string(),
                            });
                        }
                    }
                }
            }
            if trimmed.starts_with('[') && trimmed.contains("dev") {
                kind = "development";
            }
        }
    }
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
    let _ = root;
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
                kind: if path == "pyproject.toml" {
                    "manifest".to_string()
                } else {
                    "requirements".to_string()
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
                .map(|(_, value)| value.trim().trim_matches(['[', ']'].as_ref()))
                .unwrap_or_default()
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
) -> Option<Component> {
    if !path.join("Cargo.toml").is_file() {
        return None;
    }
    let lockfiles = ["Cargo.lock"]
        .iter()
        .filter(|name| path.join(name).is_file())
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let contents = fs::read_to_string(path.join("Cargo.toml")).unwrap_or_default();
    let mut section = "other";
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = if trimmed.contains("dev-dependencies") {
                "development"
            } else if trimmed.contains("build-dependencies") {
                "build"
            } else if trimmed == "[dependencies]" {
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
                });
            }
        }
    }
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
        workspace_packages: parse_toml_list(&contents, "[workspace]", "members"),
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
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.ends_with(':')
                && !trimmed.starts_with('-')
                && !trimmed.starts_with('#')
                && !trimmed.contains(' ')
                && trimmed != "services:"
                && !trimmed.starts_with("version")
            {
                info.services
                    .push(trimmed.trim_end_matches(':').to_string());
            }
            if let Some(image) = trimmed.strip_prefix("image:") {
                info.images.push(image.trim().to_string());
            }
        }
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

fn read_package_json(path: &Path) -> Result<PackageJsonData, CliError> {
    let contents = fs::read_to_string(path).map_err(|error| CliError {
        message: format!("failed to read {}: {error}", path.display()),
        exit_code: 1,
    })?;

    let value = serde_json::from_str::<serde_json::Value>(&contents).map_err(|error| CliError {
        message: format!("failed to parse {}: {error}", path.display()),
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

fn build_output(
    command: &'static str,
    detection: RepositoryDetection,
    include_runtime: bool,
) -> CommandOutput {
    let diagnostics = build_diagnostics(&detection);
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
        schema_version: "1",
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
            found: manager_lockfile.is_some(),
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
                .collect(),
            selected: manager_lockfile,
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

    if detection.package_json || detection.cargo.is_none() {
        diagnostics.push(Diagnostic {
            level: if detection.package_json {
                DiagnosticLevel::Ok
            } else {
                DiagnosticLevel::Error
            },
            check: "package.json",
            message: if detection.package_json {
                "package.json found".to_string()
            } else {
                "package.json is required to inspect a JavaScript package".to_string()
            },
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
    if detection.package_json || detection.cargo.is_none() {
        diagnostics.push(package_lock_consistency);
    }

    if detection.package_json || detection.cargo.is_none() {
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
}
