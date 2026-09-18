use std::process::ExitCode;

fn main() -> ExitCode {
    match pax::run(std::env::args()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.message);
            ExitCode::from(error.exit_code)
        }
    }
}
