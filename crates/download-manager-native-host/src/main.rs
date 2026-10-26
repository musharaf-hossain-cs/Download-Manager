use std::{io, path::Path, process::ExitCode};

use download_manager_engine::ipc::{ExtensionEnvelope, ExtensionRequest, ExtensionResponse};
use download_manager_native_host::{
    authorize_firefox_native_request, authorize_native_request, run_native_messaging_session,
};

const DEVELOPMENT_EXTENSION_ID: &str = "abcdefghijklmnopabcdefghijklmnop";
const FIREFOX_EXTENSION_ID: &str = "download-manager@musharaf-hossain-cs.github.io";
const CHROMIUM_EXTENSION_IDS: &[&str] = &[DEVELOPMENT_EXTENSION_ID];
const FIREFOX_EXTENSION_IDS: &[&str] = &[FIREFOX_EXTENSION_ID];

const HELP: &str =
    "Download Manager native-messaging host\n\nUsage: download-manager-native-host [--version]\n";

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [argument] if matches!(argument.as_str(), "--version" | "-V") => {
            println!(
                "download-manager-native-host {}",
                download_manager_engine::VERSION
            );
            ExitCode::SUCCESS
        }
        [argument] if matches!(argument.as_str(), "--help" | "-h") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        [launch_origin, ..] if launch_origin.starts_with("chrome-extension://") => {
            let current_user = current_user_identity();
            // Chromium launches the per-user registered host as its child and
            // connects only inherited stdio, so the browser peer has the same
            // logon identity as this process. The policy keeps both identities
            // explicit for deterministic rejection and future IPC adapters.
            run_authenticated_session(|request| {
                authorize_native_request(
                    launch_origin,
                    request,
                    CHROMIUM_EXTENSION_IDS,
                    &current_user,
                    &current_user,
                )
            })
        }
        [launch_manifest_path, launch_extension_id]
            if !launch_manifest_path.starts_with('-') && !launch_extension_id.starts_with('-') =>
        {
            let current_user = current_user_identity();
            let Ok(expected_executable) = std::env::current_exe() else {
                eprintln!("native messaging could not resolve the running executable");
                return ExitCode::FAILURE;
            };
            run_authenticated_session(|request| {
                authorize_firefox_native_request(
                    Path::new(launch_manifest_path),
                    launch_extension_id,
                    &expected_executable,
                    request,
                    FIREFOX_EXTENSION_IDS,
                    &current_user,
                    &current_user,
                )
            })
        }
        [] => {
            eprintln!("native messaging requires a browser launch origin");
            ExitCode::FAILURE
        }
        [argument, ..] => {
            eprintln!("unknown argument: {argument}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn run_authenticated_session(
    mut authorize: impl FnMut(&ExtensionEnvelope) -> io::Result<()>,
) -> ExitCode {
    match run_native_messaging_session(io::stdin().lock(), io::stdout().lock(), |request| {
        if let Err(error) = authorize(request) {
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

fn current_user_identity() -> String {
    let username = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    if domain.is_empty() {
        username
    } else {
        format!("{domain}\\{username}")
    }
}
