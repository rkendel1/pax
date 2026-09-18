use std::process::ExitCode;

fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    let drift = args.iter().any(|arg| arg == "drift");
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
                eprintln!("{}", error.message);
            }
            ExitCode::from(error.exit_code)
        }
    }
}
