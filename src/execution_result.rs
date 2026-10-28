//! Versioned execution-result contract (`pax.execution-result.v1`).
//!
//! A plan (`--dry-run --json`) says what PAX will execute. A result
//! (`--json`, no `--dry-run`) says what happened and what PAX can establish
//! about the delegated operation. It never evaluates a consumer's wider goal.

use super::{
    CliError, CommandName, Operation, PackageManager, ParsedCli, RunCommand, detect_repository,
    execution_plan, resolve_operation, resolve_working_directory, selection_ambiguity,
};
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

pub(crate) const SCHEMA: &str = "pax.execution-result.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Passed,
    Failed,
    Error,
    Unsupported,
    Ambiguous,
    NotRun,
}

/// Counts taken from libtest summary lines, summed over every test target.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct TestCounts {
    pub passed: u64,
    pub failed: u64,
    pub ignored: u64,
    pub measured: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ExecutionResult {
    pub schema: &'static str,
    pub operation: String,
    pub status: Status,
    pub reason: &'static str,
    pub tool: Option<String>,
    /// Native process exit status, exactly as reported; `null` when no native
    /// process ran to a normal exit.
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestCounts>,
    /// The test runner PAX read the result of, when it read one from a script:
    /// `vitest`, `jest` or `node-test`. Absent for Cargo and for anything PAX
    /// has no semantics for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<&'static str>,
}

impl ExecutionResult {
    fn new(
        operation: &Operation,
        status: Status,
        reason: &'static str,
        tool: Option<String>,
        exit_code: Option<i32>,
    ) -> Self {
        Self {
            schema: SCHEMA,
            operation: operation.name(),
            status,
            reason,
            tool,
            exit_code,
            tests: None,
            runner: None,
        }
    }
}

/// What PAX observed on cargo's machine-readable and libtest streams.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CargoTestEvidence {
    /// `Some(success)` from cargo's `build-finished` message, if one was seen.
    pub build_success: Option<bool>,
    /// Test executables cargo reported building (`profile.test` artifacts).
    pub test_executables: usize,
    pub summaries: usize,
    /// A `test result:` line was seen that PAX could not parse strictly.
    pub malformed_summary: bool,
    pub counts: TestCounts,
}

impl CargoTestEvidence {
    pub(crate) fn observe_cargo_message(&mut self, value: &serde_json::Value) {
        match value["reason"].as_str() {
            Some("build-finished") => self.build_success = value["success"].as_bool(),
            Some("compiler-artifact")
                if value["profile"]["test"].as_bool() == Some(true)
                    && value["executable"].is_string() =>
            {
                self.test_executables += 1;
            }
            _ => {}
        }
    }

    pub(crate) fn observe_summary_line(&mut self, line: &str) {
        if !line.starts_with("test result: ") {
            return;
        }
        match parse_summary(line) {
            Some(counts) => {
                self.summaries += 1;
                self.counts.passed += counts.passed;
                self.counts.failed += counts.failed;
                self.counts.ignored += counts.ignored;
                self.counts.measured += counts.measured;
            }
            None => self.malformed_summary = true,
        }
    }
}

/// Parses `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; ...`.
fn parse_summary(line: &str) -> Option<TestCounts> {
    let rest = line.strip_prefix("test result: ")?;
    let (verdict, rest) = rest.split_once(". ")?;
    if verdict != "ok" && verdict != "FAILED" {
        return None;
    }
    let mut fields = rest.split("; ");
    let mut next = |label: &str| -> Option<u64> {
        fields
            .next()?
            .strip_suffix(label)?
            .strip_suffix(' ')?
            .parse()
            .ok()
    };
    let counts = TestCounts {
        passed: next("passed")?,
        failed: next("failed")?,
        ignored: next("ignored")?,
        measured: next("measured")?,
    };
    next("filtered out")?;
    Some(counts)
}

/// Interprets one finished `cargo test`. `exit_code` is `None` when the process
/// ended without an exit status (killed by a signal).
pub(crate) fn interpret_cargo_test(
    exit_code: Option<i32>,
    evidence: &CargoTestEvidence,
) -> (Status, &'static str, Option<TestCounts>) {
    let counts = (evidence.summaries > 0 && !evidence.malformed_summary).then_some(evidence.counts);
    let failing = evidence.counts.failed > 0;
    if evidence.build_success == Some(false) {
        return (Status::Failed, "compilation-failed", counts);
    }
    if failing {
        return (Status::Failed, "tests-failed", counts);
    }
    match exit_code {
        None => (Status::Failed, "terminated-by-signal", counts),
        Some(code) if code != 0 => (Status::Failed, "native-exit-nonzero", counts),
        Some(_) => {
            let evidence_complete = evidence.summaries > 0
                && !evidence.malformed_summary
                && evidence.summaries >= evidence.test_executables;
            if !evidence_complete {
                return (Status::Unsupported, "no-libtest-evidence", counts);
            }
            let executed = evidence.counts.passed + evidence.counts.measured;
            if executed == 0 {
                (Status::NotRun, "no-tests-executed", counts)
            } else {
                (Status::Passed, "tests-passed", counts)
            }
        }
    }
}

/// Runs the native command with its stdout redirected to PAX's stderr so that
/// stdout carries only the result document. `on_line` may consume a line;
/// unconsumed lines are forwarded. Native stderr is inherited.
fn run_native(
    command: &RunCommand,
    args: &[String],
    mut on_line: impl FnMut(&str) -> bool,
) -> Result<Option<i32>, std::io::Error> {
    let mut child = Command::new(&command.program)
        .args(args)
        .current_dir(&command.working_directory)
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buffer);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !on_line(trimmed) {
            eprintln!("{trimmed}");
        }
    }
    Ok(child.wait()?.code())
}

fn execute_test(
    operation: &Operation,
    command: &RunCommand,
    script: Option<&str>,
) -> ExecutionResult {
    let tool = Some(execution_plan(command).tool);
    if let Some(script) = script
        && let Some(result) = execute_js_test(operation, command, script, tool.clone())
    {
        return result;
    }
    let is_cargo_test =
        command.program == "cargo" && command.args.first().map(String::as_str) == Some("test");
    if is_cargo_test {
        let mut args = command.args.clone();
        if !args.iter().any(|arg| arg.starts_with("--message-format")) {
            args.insert(1, "--message-format=json-render-diagnostics".to_string());
        }
        let mut evidence = CargoTestEvidence::default();
        let outcome = run_native(command, &args, |line| {
            if line.starts_with('{')
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                && value["reason"].is_string()
            {
                evidence.observe_cargo_message(&value);
                return true;
            }
            evidence.observe_summary_line(line);
            false
        });
        return match outcome {
            Err(error) => native_error(operation, tool, &error),
            Ok(exit_code) => {
                let (status, reason, tests) = interpret_cargo_test(exit_code, &evidence);
                let mut result = ExecutionResult::new(operation, status, reason, tool, exit_code);
                result.tests = tests;
                result
            }
        };
    }
    // No structured test semantics are established for this tool: run it, keep
    // the native exit status, and do not claim a semantic outcome.
    match run_native(command, &command.args, |_| false) {
        Err(error) => native_error(operation, tool, &error),
        Ok(exit_code) => ExecutionResult::new(
            operation,
            Status::Unsupported,
            "interpretation-unsupported",
            tool,
            exit_code,
        ),
    }
}

// ---- JavaScript test runners ---------------------------------------------------------------------
//
// A package manager runs the project's `test` script; PAX cannot see inside it, so it reads the script
// text, and only when that text is unambiguous. The runner must be the *last* command of the script (the
// reporter options PAX adds are appended to the whole script, so they reach the last command), must be the
// only runner named in it, and the script must not already choose a reporter or an output file. Anything
// else is `interpretation-unsupported`, as before: PAX says nothing rather than guess.
//
// What is read, and where it comes from:
//   * Vitest: `--reporter=json --outputFile=FILE` (Jest-compatible document).
//   * Jest:   `--json --outputFile=FILE`.
//   * `node --test`: `--test-reporter=tap`, whose closing `# tests/pass/fail/...` lines are read from stdout.
// A result is only as good as its evidence: no document, a document PAX cannot parse strictly, or counts
// that do not add up, is `no-runner-evidence`, never a pass.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsRunner {
    Vitest,
    Jest,
    NodeTest,
}

impl JsRunner {
    fn name(self) -> &'static str {
        match self {
            Self::Vitest => "vitest",
            Self::Jest => "jest",
            Self::NodeTest => "node-test",
        }
    }
}

const WRAPPERS: [&str; 9] = [
    "npx",
    "pnpx",
    "bunx",
    "pnpm",
    "yarn",
    "npm",
    "exec",
    "dlx",
    "cross-env",
];

/// The runner of the script's last command, or why there is none PAX will vouch for.
pub(crate) fn classify_script(script: &str) -> Option<JsRunner> {
    let mut segments: Vec<&str> = Vec::new();
    let mut rest = script;
    while let Some(at) = rest.find(['&', '|', ';']) {
        segments.push(&rest[..at]);
        let step = if rest[at..].starts_with("&&") || rest[at..].starts_with("||") {
            2
        } else {
            1
        };
        rest = &rest[at + step..];
    }
    segments.push(rest);
    let runner_of = |segment: &str| -> Option<JsRunner> {
        let mut tokens = segment.split_whitespace().peekable();
        let mut found = None;
        while let Some(token) = tokens.next() {
            if token.contains('=') && !token.starts_with('-') {
                continue; // an environment assignment
            }
            let base = token.rsplit('/').next().unwrap_or(token);
            if WRAPPERS.contains(&base) {
                continue;
            }
            found = match base {
                "vitest" => Some(JsRunner::Vitest),
                "jest" => Some(JsRunner::Jest),
                "node" if segment.split_whitespace().any(|t| t == "--test") => {
                    Some(JsRunner::NodeTest)
                }
                _ => None,
            };
            break;
        }
        found
    };
    let last = segments.last()?;
    let runner = runner_of(last)?;
    // Another runner anywhere before it makes the script ambiguous.
    if segments[..segments.len() - 1]
        .iter()
        .any(|segment| runner_of(segment).is_some())
    {
        return None;
    }
    // The script must not already choose what PAX is about to choose.
    let conflicts = [
        "--reporter",
        "--json",
        "--outputFile",
        "--output-file",
        "--test-reporter",
    ];
    if last.split_whitespace().any(|t| {
        conflicts
            .iter()
            .any(|c| t == *c || t.starts_with(&format!("{c}=")))
    }) {
        return None;
    }
    Some(runner)
}

/// What a JavaScript runner reported, before PAX decides what it means.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct JsEvidence {
    pub passed: u64,
    pub failed: u64,
    /// Skipped, pending and todo tests: known not to have run.
    pub ignored: u64,
    /// Suites or files that failed to load or run, which may contain no tests at all.
    pub suite_failures: u64,
    /// The runner's own overall verdict, where it states one.
    pub success: Option<bool>,
}

fn count(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}

/// A Jest-compatible JSON document (Jest `--json`, Vitest `--reporter=json`), read strictly: every count
/// must be present and the test counts must add up.
pub(crate) fn parse_jest_json(text: &str) -> Option<JsEvidence> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let total = count(&value, "numTotalTests")?;
    let passed = count(&value, "numPassedTests")?;
    let failed = count(&value, "numFailedTests")?;
    let pending = count(&value, "numPendingTests")?;
    let todo = count(&value, "numTodoTests").unwrap_or(0);
    if passed + failed + pending + todo != total {
        return None;
    }
    let suite_failures = count(&value, "numFailedTestSuites")?
        + count(&value, "numRuntimeErrorTestSuites").unwrap_or(0);
    Some(JsEvidence {
        passed,
        failed,
        ignored: pending + todo,
        suite_failures,
        success: Some(value.get("success")?.as_bool()?),
    })
}

/// The closing summary of Node's TAP reporter: `# tests N`, `# pass N`, `# fail N`, `# cancelled N`,
/// `# skipped N`, `# todo N`. Every one must be seen once and they must add up.
#[derive(Debug, Default)]
pub(crate) struct TapSummary {
    seen: [Option<u64>; 6],
    malformed: bool,
}

impl TapSummary {
    const LABELS: [&'static str; 6] = ["tests", "pass", "fail", "cancelled", "skipped", "todo"];

    pub(crate) fn observe_line(&mut self, line: &str) {
        let Some(rest) = line.strip_prefix("# ") else {
            return;
        };
        let Some((label, number)) = rest.split_once(' ') else {
            return;
        };
        let Some(slot) = Self::LABELS.iter().position(|l| *l == label) else {
            return;
        };
        match number.trim().parse::<u64>() {
            Ok(n) if self.seen[slot].is_none() => self.seen[slot] = Some(n),
            _ => self.malformed = true,
        }
    }

    pub(crate) fn evidence(&self) -> Option<JsEvidence> {
        if self.malformed {
            return None;
        }
        let [total, pass, fail, cancelled, skipped, todo] = self.seen;
        let (total, pass, fail, cancelled, skipped, todo) =
            (total?, pass?, fail?, cancelled?, skipped?, todo?);
        if pass + fail + cancelled + skipped + todo != total {
            return None;
        }
        Some(JsEvidence {
            passed: pass,
            failed: fail + cancelled,
            ignored: skipped + todo,
            suite_failures: 0,
            success: None,
        })
    }
}

pub(crate) fn interpret_js_test(
    exit_code: Option<i32>,
    evidence: Option<JsEvidence>,
) -> (Status, &'static str, Option<TestCounts>) {
    let counts = evidence.map(|e| TestCounts {
        passed: e.passed,
        failed: e.failed,
        ignored: e.ignored,
        measured: 0,
    });
    let Some(e) = evidence else {
        return match exit_code {
            None => (Status::Failed, "terminated-by-signal", None),
            Some(code) if code != 0 => (Status::Failed, "native-exit-nonzero", None),
            Some(_) => (Status::Unsupported, "no-runner-evidence", None),
        };
    };
    if e.failed > 0 {
        return (Status::Failed, "tests-failed", counts);
    }
    if e.suite_failures > 0 || e.success == Some(false) {
        return (Status::Failed, "test-suite-failed", counts);
    }
    match exit_code {
        None => (Status::Failed, "terminated-by-signal", counts),
        Some(code) if code != 0 => (Status::Failed, "native-exit-nonzero", counts),
        Some(_) if e.passed == 0 => (Status::NotRun, "no-tests-executed", counts),
        Some(_) => (Status::Passed, "tests-passed", counts),
    }
}

/// The package manager's own way of passing arguments to a script: npm needs `--` first.
fn with_script_args(command: &RunCommand, extra: &[String]) -> Vec<String> {
    let mut args = command.args.clone();
    if command.program == "npm" && !args.iter().any(|a| a == "--") {
        args.push("--".to_string());
    }
    args.extend(extra.iter().cloned());
    args
}

fn report_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("pax-js-{}-{nanos}.json", std::process::id()))
}

/// The most of a runner's JSON document PAX will read.
const MAX_REPORT_BYTES: u64 = 64 * 1024 * 1024;

/// Runs the project's `test` script with a machine-readable reporter, when its runner is one PAX can read.
/// `None` means PAX has no semantics for this script and the caller runs it as it always has.
fn execute_js_test(
    operation: &Operation,
    command: &RunCommand,
    script: &str,
    tool: Option<String>,
) -> Option<ExecutionResult> {
    let runner = classify_script(script)?;
    let output = report_path();
    let reporter: Vec<String> = match runner {
        JsRunner::Vitest => vec![
            "--reporter=json".into(),
            format!("--outputFile={}", output.display()),
        ],
        JsRunner::Jest => vec![
            "--json".into(),
            format!("--outputFile={}", output.display()),
        ],
        JsRunner::NodeTest => vec!["--test-reporter=tap".into()],
    };
    let args = with_script_args(command, &reporter);
    let mut tap = TapSummary::default();
    let outcome = run_native(command, &args, |line| {
        if runner == JsRunner::NodeTest {
            tap.observe_line(line);
        }
        false
    });
    let evidence = match runner {
        JsRunner::NodeTest => tap.evidence(),
        _ => std::fs::metadata(&output)
            .ok()
            .filter(|meta| meta.is_file() && meta.len() <= MAX_REPORT_BYTES)
            .and_then(|_| std::fs::read_to_string(&output).ok())
            .and_then(|text| parse_jest_json(&text)),
    };
    let _ = std::fs::remove_file(&output);
    Some(match outcome {
        Err(error) => native_error(operation, tool, &error),
        Ok(exit_code) => {
            let (status, reason, tests) = interpret_js_test(exit_code, evidence);
            let mut result = ExecutionResult::new(operation, status, reason, tool, exit_code);
            result.tests = tests;
            result.runner = Some(runner.name());
            result
        }
    })
}

fn native_error(
    operation: &Operation,
    tool: Option<String>,
    error: &std::io::Error,
) -> ExecutionResult {
    eprintln!("pax: native tool failed: {error}");
    ExecutionResult::new(operation, Status::Error, "launch-failed", tool, None)
}

/// Process exit status for a result: native status when a process ran,
/// otherwise PAX's existing fail-closed codes.
fn process_exit_code(result: &ExecutionResult, fallback: u8) -> u8 {
    match (result.status, result.exit_code) {
        (Status::Passed | Status::NotRun, _) => 0,
        (Status::Failed, Some(code)) if code != 0 => code.clamp(0, u8::MAX as i32) as u8,
        (Status::Failed, _) => 1,
        (Status::Unsupported, Some(code)) => code.clamp(0, u8::MAX as i32) as u8,
        _ => fallback,
    }
}

fn emit(result: &ExecutionResult, fallback: u8) -> Result<String, CliError> {
    let json = serde_json::to_string_pretty(result).map_err(|error| CliError {
        message: format!("failed to serialize execution result: {error}"),
        exit_code: 1,
    })?;
    println!("{json}");
    match process_exit_code(result, fallback) {
        0 => Ok(String::new()),
        exit_code => Err(CliError {
            message: String::new(),
            exit_code,
        }),
    }
}

/// Resolves and executes `operation`, emitting a `pax.execution-result.v1`
/// document on stdout. Diagnostics go to stderr. Selection semantics are the
/// same as the non-JSON path; there is no fallback tool selection.
pub(crate) fn execute_with_result(
    cli: &ParsedCli,
    operation: Operation,
) -> Result<String, CliError> {
    debug_assert!(matches!(cli.command, CommandName::Test));
    let early = |status, reason, error: Option<&CliError>, fallback| {
        if let Some(error) = error {
            eprintln!("{}", error.message);
        }
        let result = ExecutionResult::new(&operation, status, reason, None, None);
        emit(&result, fallback)
    };
    let working_directory = match resolve_working_directory(cli) {
        Ok(path) => path,
        Err(error) => return early(Status::Error, "invalid-project-directory", Some(&error), 1),
    };
    let detection = match detect_repository(&working_directory) {
        Ok(detection) => detection,
        Err(error) => {
            let code = error.exit_code.max(1);
            return early(Status::Error, "detection-failed", Some(&error), code);
        }
    };
    if let Some(message) = selection_ambiguity(cli, &detection) {
        eprintln!("{message}");
        return early(Status::Ambiguous, "ambiguous-selection", None, 2);
    }
    let resolved = match resolve_operation(
        &detection,
        operation.clone(),
        &cli.run_args,
        cli.tool.as_deref(),
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            let code = error.exit_code.max(1);
            return early(
                Status::Unsupported,
                "operation-unsupported",
                Some(&error),
                code,
            );
        }
    };
    // The text of the package.json `test` script, when the resolved command runs it
    // through a package manager: the only thing PAX can read a JavaScript runner from.
    let script = (matches!(resolved.operation, Operation::Test)
        && PackageManager::parse(&resolved.command.program).is_some()
        && resolved.command.args.first().map(String::as_str) == Some("run")
        && resolved.command.args.get(1).map(String::as_str) == Some("test"))
    .then(|| {
        detection
            .package_json_data
            .as_ref()
            .and_then(|data| data.scripts.get("test").cloned())
    })
    .flatten();
    let result = execute_test(&resolved.operation, &resolved.command, script.as_deref());
    emit(&result, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(build: Option<bool>, exes: usize, summaries: &[&str]) -> CargoTestEvidence {
        let mut evidence = CargoTestEvidence {
            build_success: build,
            test_executables: exes,
            ..Default::default()
        };
        for line in summaries {
            evidence.observe_summary_line(line);
        }
        evidence
    }

    const OK_1: &str = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s";
    const OK_0: &str = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s";
    const IGNORED: &str = "test result: ok. 0 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.00s";
    const FAILED: &str = "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s";

    fn status(exit: Option<i32>, evidence: &CargoTestEvidence) -> (Status, &'static str) {
        let (status, reason, _) = interpret_cargo_test(exit, evidence);
        (status, reason)
    }

    #[test]
    fn exit_zero_with_executed_tests_is_passed() {
        let e = evidence(Some(true), 1, &[OK_1, OK_0]);
        assert_eq!(status(Some(0), &e), (Status::Passed, "tests-passed"));
        let (_, _, counts) = interpret_cargo_test(Some(0), &e);
        assert_eq!(counts.unwrap().passed, 1);
    }

    #[test]
    fn exit_zero_is_not_universally_passed() {
        // zero tests
        let e = evidence(Some(true), 1, &[OK_0, OK_0]);
        assert_eq!(status(Some(0), &e), (Status::NotRun, "no-tests-executed"));
        // ignored only
        let e = evidence(Some(true), 1, &[IGNORED]);
        assert_eq!(status(Some(0), &e), (Status::NotRun, "no-tests-executed"));
        // no summary evidence at all (e.g. --no-run, --list, custom harness)
        let e = evidence(Some(true), 1, &[]);
        assert_eq!(
            status(Some(0), &e),
            (Status::Unsupported, "no-libtest-evidence")
        );
        // more test executables than summaries: some target is unobserved
        let e = evidence(Some(true), 3, &[OK_0]);
        assert_eq!(
            status(Some(0), &e),
            (Status::Unsupported, "no-libtest-evidence")
        );
        // unparseable summary line
        let e = evidence(Some(true), 1, &["test result: ok. many passed"]);
        assert_eq!(
            status(Some(0), &e),
            (Status::Unsupported, "no-libtest-evidence")
        );
    }

    #[test]
    fn native_failure_is_failed_and_never_error() {
        let e = evidence(Some(true), 1, &[FAILED]);
        assert_eq!(status(Some(101), &e), (Status::Failed, "tests-failed"));
        let e = evidence(Some(false), 0, &[]);
        assert_eq!(
            status(Some(101), &e),
            (Status::Failed, "compilation-failed")
        );
        let e = evidence(None, 0, &[]);
        assert_eq!(
            status(Some(101), &e),
            (Status::Failed, "native-exit-nonzero")
        );
        assert_eq!(status(None, &e), (Status::Failed, "terminated-by-signal"));
        // failing summary wins even if the exit status were 0
        let e = evidence(Some(true), 1, &[FAILED]);
        assert_eq!(status(Some(0), &e).0, Status::Failed);
    }

    #[test]
    fn summary_parser_is_strict() {
        assert_eq!(parse_summary(FAILED).unwrap().failed, 1);
        assert!(parse_summary("test result: ok. 1 passed; 0 failed").is_none());
        assert!(
            parse_summary(
                "test result: maybe. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
            )
            .is_none()
        );
        assert!(
            parse_summary(
                "test result: ok. -1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
            )
            .is_none()
        );
    }

    #[test]
    fn cargo_messages_feed_evidence() {
        let mut e = CargoTestEvidence::default();
        e.observe_cargo_message(&serde_json::json!({"reason":"compiler-artifact","profile":{"test":true},"executable":"/x"}));
        e.observe_cargo_message(&serde_json::json!({"reason":"compiler-artifact","profile":{"test":false},"executable":null}));
        e.observe_cargo_message(&serde_json::json!({"reason":"build-finished","success":false}));
        assert_eq!(e.test_executables, 1);
        assert_eq!(e.build_success, Some(false));
    }

    // ---- JavaScript runners -------------------------------------------------------------------

    const VITEST_JSON: &str = r#"{"numTotalTestSuites":2,"numPassedTestSuites":2,"numFailedTestSuites":0,"numPendingTestSuites":0,"numTotalTests":4,"numPassedTests":3,"numFailedTests":0,"numPendingTests":1,"numTodoTests":0,"startTime":1,"success":true,"testResults":[]}"#;

    #[test]
    fn the_last_command_of_the_script_names_the_runner_and_ambiguity_is_refused() {
        for (script, want) in [
            ("vitest run", Some(JsRunner::Vitest)),
            ("vitest", Some(JsRunner::Vitest)),
            ("npx vitest run --coverage", Some(JsRunner::Vitest)),
            ("pnpm exec vitest run", Some(JsRunner::Vitest)),
            ("CI=1 jest --ci", Some(JsRunner::Jest)),
            ("cross-env NODE_ENV=test jest", Some(JsRunner::Jest)),
            ("tsc --noEmit && vitest run", Some(JsRunner::Vitest)),
            ("node --test", Some(JsRunner::NodeTest)),
            ("node --test test/", Some(JsRunner::NodeTest)),
            // the runner is not last: the reporter options would reach something else
            ("vitest run && eslint .", None),
            // two runners
            ("jest && vitest run", None),
            // not a runner, or no runner
            ("mocha", None),
            ("echo vitest", None),
            ("node build.js", None),
            ("true", None),
            // the script already chooses a reporter or an output file
            ("vitest run --reporter=verbose", None),
            ("jest --json", None),
            ("jest --outputFile=out.json", None),
            ("node --test --test-reporter=spec", None),
        ] {
            assert_eq!(classify_script(script), want, "{script}");
        }
    }

    #[test]
    fn a_jest_compatible_document_is_read_strictly() {
        let e = parse_jest_json(VITEST_JSON).unwrap();
        assert_eq!(
            (e.passed, e.failed, e.ignored, e.suite_failures),
            (3, 0, 1, 0)
        );
        assert_eq!(e.success, Some(true));
        // counts that do not add up, a missing count, or no document at all
        for bad in [
            VITEST_JSON.replace("\"numTotalTests\":4", "\"numTotalTests\":5"),
            VITEST_JSON.replace("\"numPassedTests\":3,", ""),
            VITEST_JSON.replace("\"success\":true", "\"success\":\"yes\""),
            "not json".to_string(),
            "{}".to_string(),
        ] {
            assert!(parse_jest_json(&bad).is_none(), "{bad}");
        }
        // todo tests count as not run, and a runtime-error suite is a suite failure
        let with_todo = VITEST_JSON
            .replace("\"numTodoTests\":0", "\"numTodoTests\":2")
            .replace("\"numTotalTests\":4", "\"numTotalTests\":6");
        assert_eq!(parse_jest_json(&with_todo).unwrap().ignored, 3);
        let runtime = VITEST_JSON.replace(
            "\"numFailedTestSuites\":0",
            "\"numFailedTestSuites\":0,\"numRuntimeErrorTestSuites\":1",
        );
        assert_eq!(parse_jest_json(&runtime).unwrap().suite_failures, 1);
    }

    #[test]
    fn node_tap_summary_needs_every_line_once_and_they_must_add_up() {
        let lines = [
            "# tests 3",
            "# suites 0",
            "# pass 1",
            "# fail 1",
            "# cancelled 0",
            "# skipped 1",
            "# todo 0",
            "# duration_ms 38.5",
        ];
        let mut tap = TapSummary::default();
        lines.iter().for_each(|l| tap.observe_line(l));
        let e = tap.evidence().unwrap();
        assert_eq!((e.passed, e.failed, e.ignored), (1, 1, 1));
        // a missing line, a repeated line, a mismatch, and a cancelled test counting as a failure
        let mut missing = TapSummary::default();
        lines[..5].iter().for_each(|l| missing.observe_line(l));
        assert!(missing.evidence().is_none());
        let mut repeated = TapSummary::default();
        lines
            .iter()
            .chain(["# pass 1"].iter())
            .for_each(|l| repeated.observe_line(l));
        assert!(repeated.evidence().is_none());
        let mut off = TapSummary::default();
        lines
            .iter()
            .map(|l| l.replace("# tests 3", "# tests 4"))
            .for_each(|l| off.observe_line(&l));
        assert!(off.evidence().is_none());
        let mut cancelled = TapSummary::default();
        [
            "# tests 2",
            "# pass 1",
            "# fail 0",
            "# cancelled 1",
            "# skipped 0",
            "# todo 0",
        ]
        .iter()
        .for_each(|l| cancelled.observe_line(l));
        assert_eq!(cancelled.evidence().unwrap().failed, 1);
    }

    fn ev(
        passed: u64,
        failed: u64,
        ignored: u64,
        suites: u64,
        success: Option<bool>,
    ) -> Option<JsEvidence> {
        Some(JsEvidence {
            passed,
            failed,
            ignored,
            suite_failures: suites,
            success,
        })
    }

    #[test]
    fn js_exit_zero_is_not_a_pass_without_evidence_that_tests_ran() {
        let status = |exit, e| {
            let (s, r, _) = interpret_js_test(exit, e);
            (s, r)
        };
        assert_eq!(
            status(Some(0), ev(3, 0, 1, 0, Some(true))),
            (Status::Passed, "tests-passed")
        );
        // no document, or one PAX could not read
        assert_eq!(
            status(Some(0), None),
            (Status::Unsupported, "no-runner-evidence")
        );
        // nothing executed
        assert_eq!(
            status(Some(0), ev(0, 0, 2, 0, Some(true))),
            (Status::NotRun, "no-tests-executed")
        );
        assert_eq!(
            status(Some(0), ev(0, 0, 0, 0, Some(true))),
            (Status::NotRun, "no-tests-executed")
        );
        // a failing test wins over exit 0; so does a suite that failed to load (it holds no tests)
        assert_eq!(
            status(Some(0), ev(3, 1, 0, 0, None)),
            (Status::Failed, "tests-failed")
        );
        assert_eq!(
            status(Some(0), ev(3, 0, 0, 1, Some(true))),
            (Status::Failed, "test-suite-failed")
        );
        assert_eq!(
            status(Some(0), ev(3, 0, 0, 0, Some(false))),
            (Status::Failed, "test-suite-failed")
        );
        // a nonzero exit is a failure even when every counted test passed
        assert_eq!(
            status(Some(1), ev(3, 0, 0, 0, Some(true))),
            (Status::Failed, "native-exit-nonzero")
        );
        assert_eq!(
            status(Some(2), None),
            (Status::Failed, "native-exit-nonzero")
        );
        assert_eq!(
            status(None, ev(3, 0, 0, 0, None)),
            (Status::Failed, "terminated-by-signal")
        );
    }

    #[test]
    fn serialized_result_has_versioned_schema_and_native_exit_code() {
        let mut result = ExecutionResult::new(
            &Operation::Test,
            Status::Failed,
            "tests-failed",
            Some("cargo".into()),
            Some(101),
        );
        result.tests = Some(TestCounts::default());
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["schema"], "pax.execution-result.v1");
        assert_eq!(value["exit_code"], 101);
        assert_eq!(value["status"], "failed");
        assert_eq!(value["operation"], "test");
    }

    #[test]
    fn launch_error_is_distinct_from_failure() {
        let error = native_error(
            &Operation::Test,
            Some("cargo".into()),
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(error.status, Status::Error);
        assert_eq!(error.exit_code, None);
        assert_eq!(process_exit_code(&error, 1), 1);
    }

    #[test]
    fn process_exit_codes_preserve_native_status() {
        let mut result = ExecutionResult::new(
            &Operation::Test,
            Status::Failed,
            "tests-failed",
            None,
            Some(101),
        );
        assert_eq!(process_exit_code(&result, 1), 101);
        result.status = Status::Passed;
        assert_eq!(process_exit_code(&result, 1), 0);
    }
}
