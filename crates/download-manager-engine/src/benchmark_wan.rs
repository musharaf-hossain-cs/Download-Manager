//! Acceptance evidence for paired, controlled WAN-shaped benchmark runs.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::benchmark_runner::{
    PairedBenchmarkReport, SampleDistribution, distribution, u64_to_f64,
};

pub const SHAPED_WAN_MINIMUM_PAIRS: usize = 10;
pub const S167_REQUIRED_RTT_MS: [u16; 2] = [20, 80];

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WanShape {
    pub rtt_ms: u16,
    pub loss_basis_points: u16,
    pub bandwidth_mbps: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferCounters {
    pub retry_count: u32,
    pub wire_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTransferCounters {
    pub pair: u32,
    pub baseline: TransferCounters,
    pub candidate: TransferCounters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedMetricSummary {
    pub baseline: SampleDistribution,
    pub candidate: SampleDistribution,
    pub candidate_change: SampleDistribution,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WanProfileReport {
    pub shape: WanShape,
    pub paired_report: PairedBenchmarkReport,
    pub counters: Vec<PairedTransferCounters>,
    pub retry_count: PairedMetricSummary,
    pub wire_bytes: PairedMetricSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wan20And80MsReport {
    pub schema_version: u32,
    pub profiles: Vec<WanProfileReport>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ShapedWanReportError {
    #[error("20/80 ms WAN evidence requires exactly one profile at each RTT; found {actual:?}")]
    WrongRttProfiles { actual: Vec<u16> },
    #[error(
        "WAN shape bandwidth must be greater than zero and loss must not exceed 10000 basis points"
    )]
    InvalidShape,
    #[error("shaped WAN report requires at least 10 paired runs at {rtt_ms} ms; found {actual}")]
    TooFewPairs { rtt_ms: u16, actual: usize },
    #[error("shaped WAN pair {pair} at {rtt_ms} ms did not produce equal non-empty output")]
    OutputMismatch { rtt_ms: u16, pair: u32 },
    #[error("shaped WAN counters at {rtt_ms} ms do not match the paired run IDs")]
    CounterPairMismatch { rtt_ms: u16 },
    #[error("shaped WAN pair {pair} at {rtt_ms} ms reports fewer wire bytes than useful bytes")]
    WireBytesBelowUseful { rtt_ms: u16, pair: u32 },
}

impl Wan20And80MsReport {
    /// Renders throughput, retry, and wire-byte medians for both required latency profiles.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let mut markdown = String::from(
            "# Controlled shaped-WAN benchmark: 20/80 ms\n\n| RTT | Loss | Bandwidth | Baseline goodput | Candidate goodput | Paired goodput | Baseline retries | Candidate retries | Paired retries | Baseline wire bytes | Candidate wire bytes | Paired wire bytes |\n|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
        );
        for profile in &self.profiles {
            let goodput = &profile.paired_report.summary;
            writeln!(
                markdown,
                "| {} ms | {:.2}% | {} Mbps | {:.2} B/s | {:.2} B/s | {:+.2}% | {:.2} | {:.2} | {:+.2} | {:.2} | {:.2} | {:+.2}% |",
                profile.shape.rtt_ms,
                f64::from(profile.shape.loss_basis_points) / 100.0,
                profile.shape.bandwidth_mbps,
                goodput.baseline_goodput_bytes_per_second.median,
                goodput.candidate_goodput_bytes_per_second.median,
                goodput.paired_goodput_change_percent.median,
                profile.retry_count.baseline.median,
                profile.retry_count.candidate.median,
                profile.retry_count.candidate_change.median,
                profile.wire_bytes.baseline.median,
                profile.wire_bytes.candidate.median,
                profile.wire_bytes.candidate_change.median,
            )
            .expect("writing to a String cannot fail");
        }
        markdown.push_str("\nAll output hashes match: **true**\n");
        markdown
    }
}

/// Builds one machine-readable report containing the required 20 ms and 80 ms paired evidence.
///
/// `wire_bytes` includes useful and repeated transfer bytes. Callers obtain the values from each
/// application's engine metrics or release-lab wrapper; they must never be smaller than the final
/// output length. Retry counts and wire bytes remain paired with the runner's cache-busted run ID.
///
/// # Errors
/// Returns an error for missing/duplicate latency profiles, invalid shaping parameters, incomplete
/// or corrupt paired runs, or counters that do not align with those runs.
pub fn build_20_80ms_wan_report(
    profiles: Vec<(WanShape, PairedBenchmarkReport, Vec<PairedTransferCounters>)>,
) -> Result<Wan20And80MsReport, ShapedWanReportError> {
    let actual_rtts = profiles
        .iter()
        .map(|profile| profile.0.rtt_ms)
        .collect::<Vec<_>>();
    let actual_set = actual_rtts.iter().copied().collect::<BTreeSet<_>>();
    if profiles.len() != S167_REQUIRED_RTT_MS.len()
        || actual_set != S167_REQUIRED_RTT_MS.into_iter().collect()
    {
        return Err(ShapedWanReportError::WrongRttProfiles {
            actual: actual_rtts,
        });
    }

    let mut reports = profiles
        .into_iter()
        .map(|(shape, report, counters)| build_profile(shape, report, counters))
        .collect::<Result<Vec<_>, _>>()?;
    reports.sort_by_key(|profile| profile.shape.rtt_ms);
    Ok(Wan20And80MsReport {
        schema_version: REPORT_SCHEMA_VERSION,
        profiles: reports,
    })
}

fn build_profile(
    shape: WanShape,
    report: PairedBenchmarkReport,
    counters: Vec<PairedTransferCounters>,
) -> Result<WanProfileReport, ShapedWanReportError> {
    if shape.bandwidth_mbps == 0 || shape.loss_basis_points > 10_000 {
        return Err(ShapedWanReportError::InvalidShape);
    }
    if report.samples.len() < SHAPED_WAN_MINIMUM_PAIRS {
        return Err(ShapedWanReportError::TooFewPairs {
            rtt_ms: shape.rtt_ms,
            actual: report.samples.len(),
        });
    }
    for sample in &report.samples {
        if !sample.output_hash_matches
            || sample.baseline.bytes == 0
            || sample.baseline.bytes != sample.candidate.bytes
            || sample.baseline.sha256 != sample.candidate.sha256
        {
            return Err(ShapedWanReportError::OutputMismatch {
                rtt_ms: shape.rtt_ms,
                pair: sample.pair,
            });
        }
    }
    if !report.summary.all_output_hashes_match {
        return Err(ShapedWanReportError::OutputMismatch {
            rtt_ms: shape.rtt_ms,
            pair: 0,
        });
    }

    let samples = report
        .samples
        .iter()
        .map(|sample| (sample.pair, sample))
        .collect::<BTreeMap<_, _>>();
    let counter_pairs = counters
        .iter()
        .map(|item| item.pair)
        .collect::<BTreeSet<_>>();
    if counters.len() != samples.len()
        || counter_pairs.len() != counters.len()
        || counter_pairs != samples.keys().copied().collect()
    {
        return Err(ShapedWanReportError::CounterPairMismatch {
            rtt_ms: shape.rtt_ms,
        });
    }
    for counter in &counters {
        let sample = samples[&counter.pair];
        if counter.baseline.wire_bytes < sample.baseline.bytes
            || counter.candidate.wire_bytes < sample.candidate.bytes
        {
            return Err(ShapedWanReportError::WireBytesBelowUseful {
                rtt_ms: shape.rtt_ms,
                pair: counter.pair,
            });
        }
    }

    let retry_count = summarize(
        counters
            .iter()
            .map(|item| f64::from(item.baseline.retry_count)),
        counters
            .iter()
            .map(|item| f64::from(item.candidate.retry_count)),
        false,
    );
    let wire_bytes = summarize(
        counters
            .iter()
            .map(|item| u64_to_f64(item.baseline.wire_bytes)),
        counters
            .iter()
            .map(|item| u64_to_f64(item.candidate.wire_bytes)),
        true,
    );
    Ok(WanProfileReport {
        shape,
        paired_report: report,
        counters,
        retry_count,
        wire_bytes,
    })
}

fn summarize(
    baseline: impl Iterator<Item = f64>,
    candidate: impl Iterator<Item = f64>,
    percentage_change: bool,
) -> PairedMetricSummary {
    let baseline = baseline.collect::<Vec<_>>();
    let candidate = candidate.collect::<Vec<_>>();
    let changes = baseline
        .iter()
        .zip(&candidate)
        .map(|(baseline, candidate)| {
            if percentage_change {
                ((candidate / baseline) - 1.0) * 100.0
            } else {
                candidate - baseline
            }
        })
        .collect::<Vec<_>>();
    PairedMetricSummary {
        baseline: distribution(&baseline),
        candidate: distribution(&candidate),
        candidate_change: distribution(&changes),
    }
}
