//! Reproducible paired benchmark orchestration for external download applications.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkCommand {
    pub program: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkApplication {
    pub name: String,
    pub version_command: BenchmarkCommand,
    pub download_command: BenchmarkCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedBenchmarkPlan {
    pub schema_version: u32,
    pub scenario: String,
    pub protocol: String,
    pub seed: u64,
    pub pairs: u32,
    pub source: String,
    pub output_directory: PathBuf,
    #[serde(default)]
    pub retain_outputs: bool,
    pub baseline: BenchmarkApplication,
    pub candidate: BenchmarkApplication,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkRole {
    Baseline,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkSample {
    pub role: BenchmarkRole,
    pub elapsed_micros: u64,
    pub bytes: u64,
    pub goodput_bytes_per_second: f64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedBenchmarkSample {
    pub pair: u32,
    pub first: BenchmarkRole,
    pub source: String,
    pub baseline: BenchmarkSample,
    pub candidate: BenchmarkSample,
    pub goodput_change_percent: f64,
    pub output_hash_matches: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleDistribution {
    pub samples: usize,
    pub median: f64,
    pub p10: f64,
    pub p90: f64,
    pub coefficient_of_variation: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedBenchmarkSummary {
    pub baseline_goodput_bytes_per_second: SampleDistribution,
    pub candidate_goodput_bytes_per_second: SampleDistribution,
    pub paired_goodput_change_percent: SampleDistribution,
    pub all_output_hashes_match: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedBenchmarkReport {
    pub schema_version: u32,
    pub scenario: String,
    pub protocol: String,
    pub seed: u64,
    pub baseline: ApplicationIdentity,
    pub candidate: ApplicationIdentity,
    pub samples: Vec<PairedBenchmarkSample>,
    pub summary: PairedBenchmarkSummary,
}

#[derive(Debug, Error)]
pub enum PairedBenchmarkError {
    #[error("unsupported paired benchmark schema version {0}")]
    UnsupportedSchema(u32),
    #[error("benchmark scenario, protocol, and application names must not be blank")]
    BlankName,
    #[error("paired benchmark requires at least two pairs")]
    TooFewPairs,
    #[error("benchmark source must not be blank")]
    BlankSource,
    #[error("benchmark command failed for {application}: {detail}")]
    CommandFailed { application: String, detail: String },
    #[error("benchmark command for {application} did not create {path}")]
    MissingOutput { application: String, path: PathBuf },
    #[error("benchmark output for {application} was empty")]
    EmptyOutput { application: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl PairedBenchmarkReport {
    /// Renders a stable human-readable report alongside the machine-readable JSON artifact.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let summary = &self.summary;
        format!(
            "# Paired benchmark: {}\n\nProtocol: `{}`  \nBaseline: **{}** (`{}`)  \nCandidate: **{}** (`{}`)  \nSeed: `{}`; pairs: `{}`\n\n| Metric | Baseline | Candidate |\n|---|---:|---:|\n| Median goodput (B/s) | {:.2} | {:.2} |\n| p10 goodput (B/s) | {:.2} | {:.2} |\n| p90 goodput (B/s) | {:.2} | {:.2} |\n| Coefficient of variation | {:.6} | {:.6} |\n\nMedian paired goodput change: {:+.2}%  \nPaired change p10/p90: {:+.2}% / {:+.2}%  \nAll output hashes match: **{}**\n",
            self.scenario,
            self.protocol,
            self.baseline.name,
            self.baseline.version,
            self.candidate.name,
            self.candidate.version,
            self.seed,
            self.samples.len(),
            summary.baseline_goodput_bytes_per_second.median,
            summary.candidate_goodput_bytes_per_second.median,
            summary.baseline_goodput_bytes_per_second.p10,
            summary.candidate_goodput_bytes_per_second.p10,
            summary.baseline_goodput_bytes_per_second.p90,
            summary.candidate_goodput_bytes_per_second.p90,
            summary
                .baseline_goodput_bytes_per_second
                .coefficient_of_variation,
            summary
                .candidate_goodput_bytes_per_second
                .coefficient_of_variation,
            summary.paired_goodput_change_percent.median,
            summary.paired_goodput_change_percent.p10,
            summary.paired_goodput_change_percent.p90,
            summary.all_output_hashes_match,
        )
    }
}

/// Runs a seeded, randomized sequence of paired external benchmark commands.
///
/// Download command arguments may contain `{url}`, `{output}`, `{pair}`, and `{seed}`.
/// Both applications receive the same expanded URL for a pair. Outputs are removed after
/// hashing unless `retain_outputs` is enabled.
///
/// # Errors
/// Returns an error for an invalid plan, failed version/download command, or invalid output.
pub fn run_paired_benchmark(
    plan: &PairedBenchmarkPlan,
) -> Result<PairedBenchmarkReport, PairedBenchmarkError> {
    validate_plan(plan)?;
    fs::create_dir_all(&plan.output_directory)?;

    let baseline = capture_identity(&plan.baseline)?;
    let candidate = capture_identity(&plan.candidate)?;
    let mut samples = Vec::with_capacity(plan.pairs as usize);
    let orders = randomized_orders(plan.pairs, plan.seed);

    for (pair, first) in (1..=plan.pairs).zip(orders) {
        samples.push(run_pair(plan, pair, first)?);
    }

    let baseline_goodput = samples
        .iter()
        .map(|sample| sample.baseline.goodput_bytes_per_second)
        .collect::<Vec<_>>();
    let candidate_goodput = samples
        .iter()
        .map(|sample| sample.candidate.goodput_bytes_per_second)
        .collect::<Vec<_>>();
    let paired_changes = samples
        .iter()
        .map(|sample| sample.goodput_change_percent)
        .collect::<Vec<_>>();
    let all_output_hashes_match = samples.iter().all(|sample| sample.output_hash_matches);

    Ok(PairedBenchmarkReport {
        schema_version: CONFIG_SCHEMA_VERSION,
        scenario: plan.scenario.clone(),
        protocol: plan.protocol.clone(),
        seed: plan.seed,
        baseline,
        candidate,
        summary: PairedBenchmarkSummary {
            baseline_goodput_bytes_per_second: distribution(&baseline_goodput),
            candidate_goodput_bytes_per_second: distribution(&candidate_goodput),
            paired_goodput_change_percent: distribution(&paired_changes),
            all_output_hashes_match,
        },
        samples,
    })
}

fn run_pair(
    plan: &PairedBenchmarkPlan,
    pair: u32,
    first: BenchmarkRole,
) -> Result<PairedBenchmarkSample, PairedBenchmarkError> {
    let source = expand_source(&plan.source, pair, plan.seed);
    let baseline_path = plan
        .output_directory
        .join(format!("pair-{pair:03}-baseline.bin"));
    let candidate_path = plan
        .output_directory
        .join(format!("pair-{pair:03}-candidate.bin"));
    let run_baseline = || {
        run_sample(
            BenchmarkRole::Baseline,
            &plan.baseline,
            &source,
            &baseline_path,
            pair,
            plan.seed,
        )
    };
    let run_candidate = || {
        run_sample(
            BenchmarkRole::Candidate,
            &plan.candidate,
            &source,
            &candidate_path,
            pair,
            plan.seed,
        )
    };
    let (baseline, candidate) = match first {
        BenchmarkRole::Baseline => (run_baseline()?, run_candidate()?),
        BenchmarkRole::Candidate => {
            let candidate = run_candidate()?;
            (run_baseline()?, candidate)
        }
    };
    let output_hash_matches =
        baseline.sha256 == candidate.sha256 && baseline.bytes == candidate.bytes;
    let goodput_change_percent =
        ((candidate.goodput_bytes_per_second / baseline.goodput_bytes_per_second) - 1.0) * 100.0;

    if !plan.retain_outputs {
        remove_output(&baseline_path)?;
        remove_output(&candidate_path)?;
    }
    Ok(PairedBenchmarkSample {
        pair,
        first,
        source,
        baseline,
        candidate,
        goodput_change_percent,
        output_hash_matches,
    })
}

fn validate_plan(plan: &PairedBenchmarkPlan) -> Result<(), PairedBenchmarkError> {
    if plan.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(PairedBenchmarkError::UnsupportedSchema(plan.schema_version));
    }
    if plan.scenario.trim().is_empty()
        || plan.protocol.trim().is_empty()
        || plan.baseline.name.trim().is_empty()
        || plan.candidate.name.trim().is_empty()
    {
        return Err(PairedBenchmarkError::BlankName);
    }
    if plan.pairs < 2 {
        return Err(PairedBenchmarkError::TooFewPairs);
    }
    if plan.source.trim().is_empty() {
        return Err(PairedBenchmarkError::BlankSource);
    }
    Ok(())
}

fn capture_identity(
    application: &BenchmarkApplication,
) -> Result<ApplicationIdentity, PairedBenchmarkError> {
    let output = Command::new(&application.version_command.program)
        .args(&application.version_command.arguments)
        .output()
        .map_err(|error| command_error(application, error.to_string()))?;
    if !output.status.success() {
        return Err(command_error(
            application,
            format!("version command exited with {}", output.status),
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        return Err(command_error(
            application,
            "version command returned blank stdout".to_owned(),
        ));
    }
    Ok(ApplicationIdentity {
        name: application.name.clone(),
        version,
    })
}

fn run_sample(
    role: BenchmarkRole,
    application: &BenchmarkApplication,
    source: &str,
    output_path: &Path,
    pair: u32,
    seed: u64,
) -> Result<BenchmarkSample, PairedBenchmarkError> {
    if output_path.exists() {
        fs::remove_file(output_path)?;
    }
    let output = output_path.to_string_lossy();
    let pair_text = pair.to_string();
    let seed_text = seed.to_string();
    let arguments = application
        .download_command
        .arguments
        .iter()
        .map(|argument| {
            argument
                .replace("{url}", source)
                .replace("{output}", &output)
                .replace("{pair}", &pair_text)
                .replace("{seed}", &seed_text)
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    let status = Command::new(&application.download_command.program)
        .args(arguments)
        .status()
        .map_err(|error| command_error(application, error.to_string()))?;
    let elapsed_micros = u64::try_from(started.elapsed().as_micros())
        .unwrap_or(u64::MAX)
        .max(1);
    if !status.success() {
        return Err(command_error(
            application,
            format!("download command exited with {status}"),
        ));
    }
    if !output_path.is_file() {
        return Err(PairedBenchmarkError::MissingOutput {
            application: application.name.clone(),
            path: output_path.to_path_buf(),
        });
    }
    let bytes = output_path.metadata()?.len();
    if bytes == 0 {
        return Err(PairedBenchmarkError::EmptyOutput {
            application: application.name.clone(),
        });
    }
    let sha256 = sha256_file(output_path)?;
    let goodput_bytes_per_second = (u64_to_f64(bytes) * 1_000_000.0) / u64_to_f64(elapsed_micros);
    Ok(BenchmarkSample {
        role,
        elapsed_micros,
        bytes,
        goodput_bytes_per_second,
        sha256,
    })
}

fn command_error(application: &BenchmarkApplication, detail: String) -> PairedBenchmarkError {
    PairedBenchmarkError::CommandFailed {
        application: application.name.clone(),
        detail,
    }
}

fn expand_source(source: &str, pair: u32, seed: u64) -> String {
    source
        .replace("{pair}", &pair.to_string())
        .replace("{seed}", &seed.to_string())
}

fn sha256_file(path: &Path) -> Result<String, io::Error> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn remove_output(path: &Path) -> Result<(), io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn distribution(values: &[f64]) -> SampleDistribution {
    debug_assert_ne!(values.len(), 0);
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let sample_count = u32::try_from(ordered.len()).expect("benchmark pair count fits u32");
    let mean = ordered.iter().sum::<f64>() / f64::from(sample_count);
    let variance = ordered
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / f64::from(sample_count);
    SampleDistribution {
        samples: ordered.len(),
        median: percentile(&ordered, 1, 2),
        p10: percentile(&ordered, 1, 10),
        p90: percentile(&ordered, 9, 10),
        coefficient_of_variation: if mean == 0.0 {
            0.0
        } else {
            variance.sqrt() / mean.abs()
        },
    }
}

fn percentile(ordered: &[f64], numerator: usize, denominator: usize) -> f64 {
    let scaled = (ordered.len() - 1) * numerator;
    let lower = scaled / denominator;
    let remainder = scaled % denominator;
    if remainder == 0 {
        ordered[lower]
    } else {
        let fraction = f64::from(u32::try_from(remainder).expect("percentile remainder fits u32"))
            / f64::from(u32::try_from(denominator).expect("percentile denominator fits u32"));
        ordered[lower] + (ordered[lower + 1] - ordered[lower]) * fraction
    }
}

pub(crate) fn u64_to_f64(value: u64) -> f64 {
    let high = u32::try_from(value >> 32).expect("upper 32 bits fit u32");
    let low = u32::try_from(value & u64::from(u32::MAX)).expect("lower 32 bits fit u32");
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}

struct SeededRandom(u64);

impl SeededRandom {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn randomized_orders(pairs: u32, seed: u64) -> Vec<BenchmarkRole> {
    let mut orders = (0..pairs)
        .map(|index| {
            if index % 2 == 0 {
                BenchmarkRole::Baseline
            } else {
                BenchmarkRole::Candidate
            }
        })
        .collect::<Vec<_>>();
    let mut random = SeededRandom::new(seed);
    for upper in (1..orders.len()).rev() {
        let bound = u64::try_from(upper + 1).expect("benchmark pair count fits u64");
        let selected =
            usize::try_from(random.next_u64() % bound).expect("randomized order index fits usize");
        orders.swap(upper, selected);
    }
    orders
}

#[cfg(test)]
mod tests {
    use super::{BenchmarkRole, distribution, expand_source, randomized_orders};

    #[test]
    fn distribution_reports_interpolated_percentiles_and_population_cv() {
        let result = distribution(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(result.samples, 4);
        assert_eq!(result.median, 2.5);
        assert!((result.p10 - 1.3).abs() < f64::EPSILON);
        assert!((result.p90 - 3.7).abs() < f64::EPSILON);
        assert!((result.coefficient_of_variation - 0.447_213_595_499_958).abs() < 1e-12);
    }

    #[test]
    fn source_expansion_is_identical_for_both_sides_of_a_pair() {
        assert_eq!(
            expand_source("http://fixture/file?pair={pair}&seed={seed}", 7, 42),
            "http://fixture/file?pair=7&seed=42"
        );
    }

    #[test]
    fn randomized_order_is_seeded_and_balanced() {
        let orders = randomized_orders(10, 42);
        assert_eq!(orders, randomized_orders(10, 42));
        assert_eq!(
            orders
                .iter()
                .filter(|role| **role == BenchmarkRole::Baseline)
                .count(),
            5
        );
        assert_ne!(orders, randomized_orders(10, 43));
    }
}
