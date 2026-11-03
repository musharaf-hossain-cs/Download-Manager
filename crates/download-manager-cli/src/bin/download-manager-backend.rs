use std::process::ExitCode;

use download_manager_cli::{LocalIpcServer, ServerCommand, UnavailableBackend};

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
    // PU02 intentionally exposes lifecycle only. PU03 replaces this bounded authority with the
    // real durable download backend without changing the authenticated process boundary.
    let mut backend = UnavailableBackend;
    let mut server = LocalIpcServer::prepare_current_user()?;
    server.publish_ready()?;

    loop {
        if server.serve_once(&mut backend)? == ServerCommand::Shutdown {
            return Ok(());
        }
    }
}
