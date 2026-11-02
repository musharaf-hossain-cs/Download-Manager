use std::{env, fs::File, process::ExitCode};

use reqwest::{StatusCode, Version, blocking::Client, header};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("download-manager-wan-benchmark-client: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let Some(command) = arguments.next() else {
        return Err("usage: download-manager-wan-benchmark-client --version | download <role> <url> <output>".into());
    };
    if command == "--version" {
        if arguments.next().is_some() {
            return Err("--version does not accept arguments".into());
        }
        println!(
            "download-manager-wan-benchmark-client {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if command != "download" {
        return Err("unknown command".into());
    }
    let role = arguments.next().ok_or("download requires a role")?;
    let url = arguments.next().ok_or("download requires a URL")?;
    let output = arguments.next().ok_or("download requires an output path")?;
    if arguments.next().is_some() {
        return Err("download accepts exactly a role, URL, and output path".into());
    }

    let client = Client::builder().http1_only().build()?;
    for attempt in 0..=1 {
        let mut response = client
            .get(url.to_string_lossy().as_ref())
            .header(header::RANGE, "bytes=0-")
            .header("x-benchmark-role", role.to_string_lossy().as_ref())
            .send()?;
        if response.version() != Version::HTTP_11 {
            return Err(format!(
                "fixture negotiated {:?}, expected HTTP/1.1",
                response.version()
            )
            .into());
        }
        if response.status() == StatusCode::SERVICE_UNAVAILABLE && attempt == 0 {
            continue;
        }
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(format!("fixture returned {}", response.status()).into());
        }
        let mut output = File::create(output)?;
        response.copy_to(&mut output)?;
        output.sync_all()?;
        return Ok(());
    }
    Err("fixture retry budget exhausted".into())
}
