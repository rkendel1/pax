use std::process::ExitCode;

/// Returns the command word, skipping global option values. This prevents a
/// script name or directory argument that happens to be `drift` from selecting
/// drift exit-code semantics.
fn command_word(args: &[String]) -> Option<&str> {
    let mut skip_value = false;
    let mut options = true;
    for arg in args.iter().skip(1) {
        if skip_value {
            skip_value = false;
            continue;
        }
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options && arg.starts_with("--") {
            if matches!(
                arg.as_str(),
                "--tool" | "--dir" | "--scope" | "--max-files" | "--max-bytes" | "--max-facts"
            ) {
                skip_value = true;
            }
            continue;
        }
        return Some(arg);
    }
    None
}

fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    let drift = command_word(&args) == Some("drift");
    let json = args.iter().any(|arg| arg == "--json");
    match pax::run(args) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            if drift {
                let status = if json {
                    serde_json::from_str::<serde_json::Value>(&output)
                        .ok()
                        .and_then(|value| value["status"].as_str().map(str::to_string))
                } else if output == "NO DRIFT" {
                    Some("match".to_string())
                } else {
                    Some("drift".to_string())
                };
                match status.as_deref() {
                    Some("drift") => ExitCode::from(1),
                    Some("ambiguous") => ExitCode::from(2),
                    _ => ExitCode::SUCCESS,
                }
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            if !error.message.is_empty() {
                if error.exit_code == 0 {
                    println!("{}", error.message);
                } else {
                    eprintln!("{}", error.message);
                }
            }
            ExitCode::from(error.exit_code)
        }
    }
}
