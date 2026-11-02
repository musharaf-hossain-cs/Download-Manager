use std::{
    env,
    fs::File,
    io::{Read, Write},
    process::ExitCode,
};

use reqwest::{StatusCode, Version, blocking::Client, header};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("download-manager-http1-benchmark-client: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let Some(command) = arguments.next() else {
        return Err(
            "usage: download-manager-http1-benchmark-client --version | download|download-single-stream|profile-before|profile-after <url> <output>"
                .into(),
        );
    };
    if command == "--version" {
        if arguments.next().is_some() {
            return Err("--version does not accept arguments".into());
        }
        println!(
            "download-manager-http1-benchmark-client {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let (send_range, sync_policy) = match command.to_string_lossy().as_ref() {
        "download" => (true, SyncPolicy::OnCompletion),
        "download-single-stream" | "profile-after" => (false, SyncPolicy::OnCompletion),
        "profile-before" => (false, SyncPolicy::EveryChunk),
        _ => return Err("unknown command".into()),
    };
    let url = arguments.next().ok_or("download requires a URL")?;
    let output = arguments.next().ok_or("download requires an output path")?;
    if arguments.next().is_some() {
        return Err("download accepts exactly a URL and output path".into());
    }

    let client = Client::builder().http1_only().build()?;
    let mut request = client.get(url.to_string_lossy().as_ref());
    if send_range {
        request = request.header(header::RANGE, "bytes=0-");
    }
    let mut response = request.send()?;
    if response.version() != Version::HTTP_11 {
        return Err(format!(
            "fixture negotiated {:?}, expected HTTP/1.1",
            response.version()
        )
        .into());
    }
    if !send_range && response.status() != StatusCode::OK {
        return Err(format!(
            "single-stream fixture returned {}, expected 200 OK",
            response.status()
        )
        .into());
    }
    if send_range && !response.status().is_success() {
        return Err(format!("fixture returned {}", response.status()).into());
    }
    let mut output = File::create(output)?;
    copy_with_sync_policy(&mut response, &mut output, sync_policy)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum SyncPolicy {
    EveryChunk,
    OnCompletion,
}

fn copy_with_sync_policy(
    source: &mut impl Read,
    output: &mut File,
    policy: SyncPolicy,
) -> Result<(), std::io::Error> {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let mut count = 0;
        while count < buffer.len() {
            let read = source.read(&mut buffer[count..])?;
            if read == 0 {
                break;
            }
            count += read;
        }
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        if matches!(policy, SyncPolicy::EveryChunk) {
            output.sync_all()?;
        }
    }
    output.flush()?;
    if matches!(policy, SyncPolicy::OnCompletion) {
        output.sync_all()?;
    }
    Ok(())
}
