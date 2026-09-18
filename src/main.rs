use std::process::ExitCode;

fn main() -> ExitCode {
    match pax::run(std::env::args()) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            if !error.message.is_empty() {
                eprintln!("{}", error.message);
            }
            ExitCode::from(error.exit_code)
        }
    }
}
