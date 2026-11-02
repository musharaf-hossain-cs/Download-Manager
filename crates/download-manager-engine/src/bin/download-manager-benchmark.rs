use std::{env, fs, path::PathBuf, process::ExitCode};

use download_manager_engine::{
    benchmark_lan::{NON_RANGE_SINGLE_STREAM_SCENARIO, build_non_range_single_stream_report},
    benchmark_runner::{PairedBenchmarkPlan, run_paired_benchmark},
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("download-manager-benchmark: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let config_path = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: download-manager-benchmark <config.json> <report-prefix>")?,
    );
    let report_prefix = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: download-manager-benchmark <config.json> <report-prefix>")?,
    );
    if arguments.next().is_some() {
        return Err("usage: download-manager-benchmark <config.json> <report-prefix>".into());
    }

    let plan: PairedBenchmarkPlan = serde_json::from_slice(&fs::read(config_path)?)?;
    let report = run_paired_benchmark(&plan)?;
    let json_path = report_prefix.with_extension("json");
    let markdown_path = report_prefix.with_extension("md");
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if report.scenario == NON_RANGE_SINGLE_STREAM_SCENARIO {
        let evidence = build_non_range_single_stream_report(&report)?;
        fs::write(json_path, serde_json::to_vec_pretty(&evidence)?)?;
        fs::write(markdown_path, evidence.render_markdown())?;
    } else {
        fs::write(json_path, serde_json::to_vec_pretty(&report)?)?;
        fs::write(markdown_path, report.render_markdown())?;
    }
    Ok(())
}
