//! Versioned execution-result contract (`pax.execution-result.v1`).
//!
//! A plan (`--dry-run --json`) says what PAX will execute. A result
//! (`--json`, no `--dry-run`) says what happened and what PAX can establish
//! about the delegated operation. It never evaluates a consumer's wider goal.

use super::{
    CliError, CommandName, Operation, ParsedCli, RunCommand, detect_repository, execution_plan,
    resolve_operation, resolve_working_directory, selection_ambiguity,
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

fn execute_test(operation: &Operation, command: &RunCommand) -> ExecutionResult {
    let tool = Some(execution_plan(command).tool);
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
    let result = execute_test(&resolved.operation, &resolved.command);
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
