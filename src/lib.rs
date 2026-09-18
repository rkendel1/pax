use serde::Serialize;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct CliError {
    pub message: String,
    pub exit_code: u8,
}

#[derive(Clone, Copy, Debug)]
enum CommandName {
    Info,
    Doctor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum PackageManager {
    Npm,
    Pnpm,
    Bun,
    Yarn,
}

impl PackageManager {
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
    name: Option<PackageManager>,
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
    selection_notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct DetectedManager {
    name: PackageManager,
    version: Option<String>,
    lockfile: Option<String>,
    selected_by: String,
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

    let detection = detect_repository(&cwd)?;
    let output = match cli.command {
        CommandName::Info => build_output("info", detection, false),
        CommandName::Doctor => build_output("doctor", detection, true),
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

struct ParsedCli {
    command: CommandName,
    json: bool,
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

    let json = args.iter().any(|arg| arg == "--json");
    args.retain(|arg| arg != "--json");

    let command = match args.as_slice() {
        [command] if command == "info" => CommandName::Info,
        [command] if command == "doctor" => CommandName::Doctor,
        [] => {
            return Err(CliError {
                message: usage(),
                exit_code: 2,
            });
        }
        _ => {
            return Err(CliError {
                message: format!("unknown arguments: {}\n\n{}", args.join(" "), usage()),
                exit_code: 2,
            });
        }
    };

    Ok(ParsedCli { command, json })
}

fn usage() -> String {
    "usage: pax [--json] <info|doctor>".to_string()
}

fn detect_repository(root: &Path) -> Result<RepositoryDetection, CliError> {
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
    let (manager, selection_notes) = select_manager(package_json_data.as_ref(), &lockfiles);
    let fallback_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-project")
        .to_string();
    let project_name = package_json_data
        .as_ref()
        .and_then(|data| data.name.clone())
        .unwrap_or(fallback_name);

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
        selection_notes,
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

    Ok(PackageJsonData {
        name,
        package_manager_field,
        has_workspaces,
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
        .or_else(|| detection.lockfiles.first().cloned());
    let selected_by = detection
        .manager
        .as_ref()
        .map(|manager| manager.selected_by.clone());
    let summary = match manager_name {
        Some(manager) => format!(
            "Detected {} package reality for {}",
            manager, detection.project_name
        ),
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
            workspace: detection.workspace,
            workspace_source: detection.workspace_source,
        },
        manager: ManagerInfo {
            name: manager_name,
            version: manager_version,
            lockfile: manager_lockfile,
            selected_by,
        },
        result: CommandResult { summary, runtime },
        diagnostics,
        evidence: Evidence {
            package_manager_field: detection
                .package_json_data
                .and_then(|data| data.package_manager_field),
            lockfiles: detection.lockfiles,
            workspace_files: detection.workspace_files,
            selection_notes: detection.selection_notes,
        },
    }
}

fn build_diagnostics(detection: &RepositoryDetection) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

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
    diagnostics.push(package_lock_consistency);

    diagnostics.push(Diagnostic {
        level: DiagnosticLevel::Ok,
        check: "workspace configuration",
        message: detection
            .workspace_source
            .as_ref()
            .map(|source| format!("Workspace detected via {source}"))
            .unwrap_or_else(|| "No workspace configuration detected".to_string()),
    });

    diagnostics
}

fn detect_node_runtime() -> Option<String> {
    let output = Command::new("node").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8(output.stdout).ok()?;
    let trimmed = version.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn render_human(output: &CommandOutput) -> String {
    let heading = match output.command {
        "info" => "PAX Info",
        "doctor" => "PAX Doctor",
        _ => "PAX",
    };
    let manager = output
        .manager
        .name
        .map(|manager| manager.to_string())
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

    if let Some(selected_by) = &output.manager.selected_by {
        lines.push(format!("Selected by   {selected_by}"));
    }

    if let Some(runtime) = &output.result.runtime {
        lines.push(format!("Runtime       Node {runtime}"));
    }

    lines.push(String::new());
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = format!(
            "pax-tests-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = env::temp_dir().join(unique);
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
