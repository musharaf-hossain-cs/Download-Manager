//! Acceptance checks for controlled LAN benchmark reports.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::benchmark_runner::{PairedBenchmarkReport, SampleDistribution};

pub const CONTROLLED_LAN_MINIMUM_PAIRS: usize = 10;
pub const HTTP_1_1_PROTOCOL: &str = "HTTP/1.1";
pub const HTTP_2_PROTOCOL: &str = "HTTP/2";
pub const NON_RANGE_SINGLE_STREAM_SCENARIO: &str = "non-range-single-stream";
pub const MAXIMUM_SINGLE_STREAM_OVERHEAD_PERCENT: f64 = 3.0;

const REPORT_SCHEMA_VERSION: u32 = 1;
const PERCENTAGE_COMPARISON_EPSILON: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NonRangeSingleStreamReport {
    pub schema_version: u32,
    pub paired_report: PairedBenchmarkReport,
    pub completion_time_overhead_percent: SampleDistribution,
    pub maximum_allowed_overhead_percent: f64,
    pub overhead_within_limit: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ControlledLanReportError {
    #[error("controlled LAN report used scenario {actual:?}, expected {expected}")]
    WrongScenario {
        expected: &'static str,
        actual: String,
    },
    #[error("controlled LAN report used protocol {actual:?}, expected {expected}")]
    WrongProtocol {
        expected: &'static str,
        actual: String,
    },
    #[error("controlled LAN report requires at least 10 paired runs; found {actual}")]
    TooFewPairs { actual: usize },
    #[error("controlled LAN pair {pair} did not produce equal non-empty output hashes and lengths")]
    OutputMismatch { pair: u32 },
    #[error("controlled LAN report summary did not confirm all output hashes")]
    SummaryHashMismatch,
}

impl NonRangeSingleStreamReport {
    /// Renders explicit non-range overhead evidence alongside the paired JSON report.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let overhead = &self.completion_time_overhead_percent;
        let verdict = if self.overhead_within_limit {
            "PASS"
        } else {
            "FAIL"
        };
        format!(
            "# Non-range single-stream parity\n\nProtocol: `{}`  \nBaseline: **{}** (`{}`)  \nCandidate: **{}** (`{}`)  \nSeed: `{}`; pairs: `{}`\n\n| Completion-time overhead | Percent |\n|---|---:|\n| Median | {:+.2}% |\n| p10 | {:+.2}% |\n| p90 | {:+.2}% |\n| Coefficient of variation | {:.6} |\n\nMaximum allowed median overhead: **{:.2}%**  \nAll output hashes match: **{}**  \nOverhead gate: **{}**\n",
            self.paired_report.protocol,
            self.paired_report.baseline.name,
            self.paired_report.baseline.version,
            self.paired_report.candidate.name,
            self.paired_report.candidate.version,
            self.paired_report.seed,
            self.paired_report.samples.len(),
            overhead.median,
            overhead.p10,
            overhead.p90,
            overhead.coefficient_of_variation,
            self.maximum_allowed_overhead_percent,
            self.paired_report.summary.all_output_hashes_match,
            verdict,
        )
    }
}

/// Verifies the release evidence required from a controlled HTTP/1.1 LAN run.
///
/// # Errors
/// Returns an error unless the report identifies HTTP/1.1, contains at least ten pairs, and every
/// pair produced equal non-empty output bytes and SHA-256 values.
pub fn validate_http1_lan_report(
    report: &PairedBenchmarkReport,
) -> Result<(), ControlledLanReportError> {
    validate_lan_report(report, HTTP_1_1_PROTOCOL)
}

/// Verifies the release evidence required from a controlled HTTP/2 LAN run.
///
/// # Errors
/// Returns an error unless the report identifies HTTP/2, contains at least ten pairs, and every
/// pair produced equal non-empty output bytes and SHA-256 values.
pub fn validate_http2_lan_report(
    report: &PairedBenchmarkReport,
) -> Result<(), ControlledLanReportError> {
    validate_lan_report(report, HTTP_2_PROTOCOL)
}

/// Builds the release evidence for a non-range HTTP/1.1 single-stream comparison.
///
/// The report remains valid evidence when it fails the three-percent overhead gate, allowing the
/// measured regression to be preserved for the next profile-and-fix slice.
///
/// # Errors
/// Returns an error unless the scenario is explicitly non-range single-stream, uses HTTP/1.1,
/// contains at least ten pairs, and every pair produced equal non-empty output bytes and hashes.
pub fn build_non_range_single_stream_report(
    report: &PairedBenchmarkReport,
) -> Result<NonRangeSingleStreamReport, ControlledLanReportError> {
    if report.scenario != NON_RANGE_SINGLE_STREAM_SCENARIO {
        return Err(ControlledLanReportError::WrongScenario {
            expected: NON_RANGE_SINGLE_STREAM_SCENARIO,
            actual: report.scenario.clone(),
        });
    }
    validate_lan_report(report, HTTP_1_1_PROTOCOL)?;
    let overhead_values = report
        .samples
        .iter()
        .map(|sample| {
            let baseline = Duration::from_micros(sample.baseline.elapsed_micros).as_secs_f64();
            let candidate = Duration::from_micros(sample.candidate.elapsed_micros).as_secs_f64();
            ((candidate / baseline) - 1.0) * 100.0
        })
        .collect::<Vec<_>>();
    let completion_time_overhead_percent = distribution(&overhead_values);
    let overhead_within_limit = completion_time_overhead_percent.median
        <= MAXIMUM_SINGLE_STREAM_OVERHEAD_PERCENT + PERCENTAGE_COMPARISON_EPSILON;
    Ok(NonRangeSingleStreamReport {
        schema_version: REPORT_SCHEMA_VERSION,
        paired_report: report.clone(),
        completion_time_overhead_percent,
        maximum_allowed_overhead_percent: MAXIMUM_SINGLE_STREAM_OVERHEAD_PERCENT,
        overhead_within_limit,
    })
}

fn validate_lan_report(
    report: &PairedBenchmarkReport,
    expected_protocol: &'static str,
) -> Result<(), ControlledLanReportError> {
    if report.protocol != expected_protocol {
        return Err(ControlledLanReportError::WrongProtocol {
            expected: expected_protocol,
            actual: report.protocol.clone(),
        });
    }
    if report.samples.len() < CONTROLLED_LAN_MINIMUM_PAIRS {
        return Err(ControlledLanReportError::TooFewPairs {
            actual: report.samples.len(),
        });
    }
    for sample in &report.samples {
        if !sample.output_hash_matches
            || sample.baseline.bytes == 0
            || sample.baseline.bytes != sample.candidate.bytes
            || sample.baseline.sha256 != sample.candidate.sha256
        {
            return Err(ControlledLanReportError::OutputMismatch { pair: sample.pair });
        }
    }
    if !report.summary.all_output_hashes_match {
        return Err(ControlledLanReportError::SummaryHashMismatch);
    }
    Ok(())
}

fn distribution(values: &[f64]) -> SampleDistribution {
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let sample_count =
        u32::try_from(ordered.len()).expect("validated benchmark pair count fits u32");
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
