use std::process::ExitCode;

const HELP: &str =
    "Download Manager native-messaging host\n\nUsage: download-manager-native-host [--version]\n";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!(
                "download-manager-native-host {}",
                download_manager_engine::VERSION
            );
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
