use std::process::ExitCode;

use download_manager_cli::{DownloadBackend, LocalIpcServer, ServerCommand};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("download-manager-backend: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut backend = DownloadBackend::for_current_user()?;
    let mut server = LocalIpcServer::prepare_current_user()?;
    server.publish_ready()?;

    loop {
        if server.serve_once(&mut backend)? == ServerCommand::Shutdown {
            return Ok(());
        }
    }
}
