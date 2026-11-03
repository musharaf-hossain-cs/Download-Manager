use std::process::ExitCode;

fn main() -> ExitCode {
    let mut authenticated =
        download_manager_cli::AuthenticatedIpcBackend::discover_current_user().ok();
    let mut unavailable = download_manager_cli::UnavailableBackend;
    let backend: &mut dyn download_manager_cli::CliBackend = match authenticated.as_mut() {
        Some(backend) => backend,
        None => &mut unavailable,
    };
    let exit = download_manager_cli::run(
        std::env::args_os().skip(1),
        backend,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(exit.code())
}
