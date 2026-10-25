use std::process::ExitCode;

const HELP: &str =
    "Download Manager command-line interface\n\nUsage: download-manager-cli [--version]\n";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("download-manager-cli {}", download_manager_engine::VERSION);
            ExitCode::SUCCESS
        }
        None | Some("--help" | "-h") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(argument) => {
            eprintln!("unknown argument: {argument}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
