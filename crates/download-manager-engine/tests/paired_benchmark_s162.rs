use std::{collections::BTreeSet, fs, path::Path};

use download_manager_engine::benchmark_runner::{
    BenchmarkApplication, BenchmarkCommand, BenchmarkRole, PairedBenchmarkPlan,
    run_paired_benchmark,
};

#[test]
fn small_ab_fixture_captures_versions_randomizes_order_and_reports_valid_statistics() {
    let temporary = tempfile::tempdir().expect("temporary benchmark directory");
    let source = temporary.path().join("fixture.bin");
    fs::write(&source, vec![0x5a; 512 * 1024]).expect("fixture body");
    let fixture = write_fixture_command(temporary.path());
    let application = |name: &str, version: &str| BenchmarkApplication {
        name: name.to_owned(),
        version_command: fixture_command(&fixture, &["version", version]),
        download_command: fixture_command(&fixture, &["download", "{url}", "{output}"]),
    };
    let plan = PairedBenchmarkPlan {
        schema_version: 1,
        scenario: "small-a-b-fixture".to_owned(),
        protocol: "fixture-copy".to_owned(),
        seed: 0x51_62_ab,
        pairs: 6,
        source: source.to_string_lossy().into_owned(),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("IDM", "IDM fixture 6.42.41"),
        candidate: application("Download Manager", "Download Manager fixture 0.1.0"),
    };

    let report = run_paired_benchmark(&plan).expect("paired benchmark");
    assert_eq!(report.scenario, "small-a-b-fixture");
    assert_eq!(report.protocol, "fixture-copy");
    assert_eq!(report.baseline.version, "IDM fixture 6.42.41");
    assert_eq!(report.candidate.version, "Download Manager fixture 0.1.0");
    assert_eq!(report.samples.len(), 6);
    assert!(report.summary.all_output_hashes_match);
    assert!(report.samples.iter().all(|sample| {
        sample.output_hash_matches
            && sample.baseline.bytes == 512 * 1024
            && sample.baseline.sha256.len() == 64
            && sample.baseline.goodput_bytes_per_second.is_finite()
            && sample.candidate.goodput_bytes_per_second.is_finite()
    }));

    let first_roles = report
        .samples
        .iter()
        .map(|sample| sample.first)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        first_roles,
        BTreeSet::from([BenchmarkRole::Baseline, BenchmarkRole::Candidate])
    );
    assert_valid_distribution(report.summary.baseline_goodput_bytes_per_second.median);
    assert_valid_distribution(report.summary.candidate_goodput_bytes_per_second.median);
    assert!(
        report
            .summary
            .paired_goodput_change_percent
            .coefficient_of_variation
            .is_finite()
    );
    assert!(
        report
            .render_markdown()
            .contains("Protocol: `fixture-copy`")
    );
    assert!(
        report
            .render_markdown()
            .contains("All output hashes match: **true**")
    );
    assert_eq!(
        fs::read_dir(&plan.output_directory)
            .expect("output directory")
            .count(),
        0,
        "large benchmark outputs should be removed after hashing"
    );
}

fn assert_valid_distribution(median: f64) {
    assert!(median.is_finite());
    assert!(median > 0.0);
}

#[cfg(windows)]
fn write_fixture_command(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("benchmark-fixture.cmd");
    fs::write(
        &path,
        "@echo off\r\nif \"%~1\"==\"version\" (\r\n  echo %~2\r\n  exit /b 0\r\n)\r\ncopy /b \"%~2\" \"%~3\" >nul\r\n",
    )
    .expect("fixture command");
    path
}

#[cfg(windows)]
fn fixture_command(path: &Path, arguments: &[&str]) -> BenchmarkCommand {
    let mut command_arguments = vec![
        "/D".to_owned(),
        "/C".to_owned(),
        path.to_string_lossy().into_owned(),
    ];
    command_arguments.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    BenchmarkCommand {
        program: "cmd.exe".into(),
        arguments: command_arguments,
    }
}

#[cfg(not(windows))]
fn write_fixture_command(directory: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join("benchmark-fixture.sh");
    fs::write(
        &path,
        "#!/bin/sh\nif [ \"$1\" = version ]; then\n  printf '%s\\n' \"$2\"\n  exit 0\nfi\ncp \"$2\" \"$3\"\n",
    )
    .expect("fixture command");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("fixture permissions");
    path
}

#[cfg(not(windows))]
fn fixture_command(path: &Path, arguments: &[&str]) -> BenchmarkCommand {
    let mut command_arguments = vec![path.to_string_lossy().into_owned()];
    command_arguments.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    BenchmarkCommand {
        program: "/bin/sh".into(),
        arguments: command_arguments,
    }
}
