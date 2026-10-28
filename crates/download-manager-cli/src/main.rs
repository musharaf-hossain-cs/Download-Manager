use std::process::ExitCode;

fn main() -> ExitCode {
    let mut backend = download_manager_cli::UnavailableBackend;
    let exit = download_manager_cli::run(
        std::env::args_os().skip(1),
        &mut backend,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(exit.code())
}
