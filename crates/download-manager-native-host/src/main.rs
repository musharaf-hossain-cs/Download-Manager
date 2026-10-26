use std::{io, process::ExitCode};

use download_manager_engine::ipc::ExtensionResponse;
use download_manager_native_host::run_native_messaging_session;

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
        Some("--help" | "-h") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        None => match run_native_messaging_session(io::stdin().lock(), io::stdout().lock(), |_| {
            ExtensionResponse::Rejected {
                code: "not_ready".to_owned(),
                message: "native request handling is not available yet".to_owned(),
            }
        }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("native messaging failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some(argument) => {
            eprintln!("unknown argument: {argument}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
