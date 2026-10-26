use std::{io, process::ExitCode};

use download_manager_engine::ipc::{ExtensionRequest, ExtensionResponse};
use download_manager_native_host::{authorize_native_request, run_native_messaging_session};

const DEVELOPMENT_EXTENSION_ID: &str = "abcdefghijklmnopabcdefghijklmnop";
const PACKAGED_EXTENSION_IDS: &[&str] = &[DEVELOPMENT_EXTENSION_ID];

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
        Some(launch_origin) if !launch_origin.starts_with('-') => {
            let current_user = current_user_identity();
            // Chromium launches the per-user registered host as its child and
            // connects only inherited stdio, so the browser peer has the same
            // logon identity as this process. The policy keeps both identities
            // explicit for deterministic rejection and future IPC adapters.
            match run_native_messaging_session(io::stdin().lock(), io::stdout().lock(), |request| {
                if let Err(error) = authorize_native_request(
                    launch_origin,
                    request,
                    PACKAGED_EXTENSION_IDS,
                    &current_user,
                    &current_user,
                ) {
                    return ExtensionResponse::Rejected {
                        code: "access_denied".to_owned(),
                        message: error.to_string(),
                    };
                }
                if matches!(&request.payload, ExtensionRequest::Health) {
                    ExtensionResponse::Healthy {
                        host_version: download_manager_engine::VERSION.to_owned(),
                    }
                } else {
                    ExtensionResponse::Rejected {
                        code: "not_ready".to_owned(),
                        message: "native request handling is not available yet".to_owned(),
                    }
                }
            }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("native messaging failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        None => {
            eprintln!("native messaging requires a browser launch origin");
            ExitCode::FAILURE
        }
        Some(argument) => {
            eprintln!("unknown argument: {argument}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn current_user_identity() -> String {
    let username = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    if domain.is_empty() {
        username
    } else {
        format!("{domain}\\{username}")
    }
}
