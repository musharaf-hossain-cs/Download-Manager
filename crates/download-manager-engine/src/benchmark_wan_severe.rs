//! Acceptance evidence for the severe shaped-WAN adaptive-controller scenario.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::benchmark_runner::{PairedBenchmarkReport, u64_to_f64};

pub const SEVERE_WAN_SCENARIO: &str = "wan-180ms-loss-throttle";
pub const SEVERE_WAN_RTT_MILLIS: u16 = 180;
pub const SEVERE_WAN_LOSS_BASIS_POINTS: u16 = 100;
pub const SEVERE_WAN_MINIMUM_PAIRS: usize = 10;

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WanThrottle {
    PerConnection,
    Total,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SevereWanProfile {
    pub rtt_millis: u16,
    pub loss_basis_points: u16,
    pub bandwidth_mbps: u16,
    pub throttle: WanThrottle,
}

impl Default for SevereWanProfile {
    fn default() -> Self {
        Self {
            rtt_millis: SEVERE_WAN_RTT_MILLIS,
            loss_basis_points: SEVERE_WAN_LOSS_BASIS_POINTS,
            bandwidth_mbps: 25,
            throttle: WanThrottle::PerConnection,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveControllerStep {
    pub interval: u32,
    pub target_before: u8,
    pub target_after: u8,
    pub useful_bytes: u64,
    pub wire_bytes: u64,
    pub retries: u32,
    pub throttled: bool,
    pub network_loss: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePairTrace {
    pub pair: u32,
    pub steps: Vec<AdaptiveControllerStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveControllerSummary {
    pub pressure_events: u64,
    pub loss_events: u64,
    pub throttle_events: u64,
    pub retries: u64,
    pub useful_bytes: u64,
    pub wire_bytes: u64,
    pub wire_overhead_percent: f64,
    pub maximum_target_connections: u8,
    pub pairs_backed_off: usize,
    pub pairs_recovered: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SevereWanBenchmarkReport {
    pub schema_version: u32,
    pub profile: SevereWanProfile,
    pub paired_report: PairedBenchmarkReport,
    pub controller_traces: Vec<AdaptivePairTrace>,
    pub controller_summary: AdaptiveControllerSummary,
    pub adaptive_behavior_valid: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SevereWanReportError {
    #[error("severe WAN report used scenario {actual:?}, expected {SEVERE_WAN_SCENARIO}")]
    WrongScenario { actual: String },
    #[error("severe WAN profile must use 180 ms RTT")]
    WrongRtt,
    #[error("severe WAN profile must use 1% loss")]
    WrongLoss,
    #[error("severe WAN profile must specify non-zero bandwidth and throttling")]
    MissingThrottle,
    #[error("severe WAN report requires at least 10 paired runs; found {actual}")]
    TooFewPairs { actual: usize },
    #[error("severe WAN pair {pair} did not produce equal non-empty output hashes and lengths")]
    OutputMismatch { pair: u32 },
    #[error("severe WAN report summary did not confirm all output hashes")]
    SummaryHashMismatch,
    #[error("adaptive traces must cover each benchmark pair exactly once")]
    TraceCoverage,
    #[error("adaptive trace for pair {pair} has invalid interval, target, or byte accounting")]
    InvalidTrace { pair: u32 },
    #[error("adaptive trace for pair {pair} does not show loss/throttle backoff and recovery")]
    MissingAdaptiveResponse { pair: u32 },
}

impl SevereWanBenchmarkReport {
    /// Renders the paired throughput and adaptive-controller evidence in a stable form.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let paired = &self.paired_report;
        let summary = &self.controller_summary;
        let verdict = if self.adaptive_behavior_valid {
            "PASS"
        } else {
            "FAIL"
        };
        format!(
            "# Severe shaped-WAN adaptive-controller benchmark\n\nRTT: **{} ms**; loss: **{:.2}%**; bandwidth: **{} Mbps**; throttle: **{:?}**  \nProtocol: `{}`; pairs: `{}`\n\n| Paired throughput | Baseline | Candidate |\n|---|---:|---:|\n| Median goodput (B/s) | {:.2} | {:.2} |\n| p10 goodput (B/s) | {:.2} | {:.2} |\n| p90 goodput (B/s) | {:.2} | {:.2} |\n\n| Controller evidence | Value |\n|---|---:|\n| Pressure events (loss / throttle) | {} ({} / {}) |\n| Retries | {} |\n| Wire / useful bytes | {} / {} |\n| Wire overhead | {:.2}% |\n| Maximum target connections | {} |\n| Pairs backed off / recovered | {} / {} |\n\nAll output hashes match: **{}**  \nAdaptive-controller gate: **{}**\n",
            self.profile.rtt_millis,
            f64::from(self.profile.loss_basis_points) / 100.0,
            self.profile.bandwidth_mbps,
            self.profile.throttle,
            paired.protocol,
            paired.samples.len(),
            paired.summary.baseline_goodput_bytes_per_second.median,
            paired.summary.candidate_goodput_bytes_per_second.median,
            paired.summary.baseline_goodput_bytes_per_second.p10,
            paired.summary.candidate_goodput_bytes_per_second.p10,
            paired.summary.baseline_goodput_bytes_per_second.p90,
            paired.summary.candidate_goodput_bytes_per_second.p90,
            summary.pressure_events,
            summary.loss_events,
            summary.throttle_events,
            summary.retries,
            summary.wire_bytes,
            summary.useful_bytes,
            summary.wire_overhead_percent,
            summary.maximum_target_connections,
            summary.pairs_backed_off,
            summary.pairs_recovered,
            paired.summary.all_output_hashes_match,
            verdict,
        )
    }
}

/// Builds auditable evidence for the 180 ms, 1% loss, throttled WAN profile.
///
/// Each benchmark pair must have one candidate controller trace. Every trace must show a
/// loss/throttle pressure interval that reduces the connection target and a later productive
/// interval that raises it again.
///
/// # Errors
/// Returns an error when the paired result, shaping profile, or controller trace is incomplete.
pub fn build_severe_wan_report(
    paired_report: &PairedBenchmarkReport,
    profile: SevereWanProfile,
    controller_traces: Vec<AdaptivePairTrace>,
) -> Result<SevereWanBenchmarkReport, SevereWanReportError> {
    validate_paired_report(paired_report)?;
    validate_profile(profile)?;

    let expected_pairs = paired_report
        .samples
        .iter()
        .map(|sample| sample.pair)
        .collect::<BTreeSet<_>>();
    let traces_by_pair = controller_traces
        .iter()
        .map(|trace| (trace.pair, trace))
        .collect::<BTreeMap<_, _>>();
    if traces_by_pair.len() != controller_traces.len()
        || traces_by_pair.keys().copied().collect::<BTreeSet<_>>() != expected_pairs
    {
        return Err(SevereWanReportError::TraceCoverage);
    }

    let mut summary = AdaptiveControllerSummary {
        pressure_events: 0,
        loss_events: 0,
        throttle_events: 0,
        retries: 0,
        useful_bytes: 0,
        wire_bytes: 0,
        wire_overhead_percent: 0.0,
        maximum_target_connections: 1,
        pairs_backed_off: 0,
        pairs_recovered: 0,
    };
    for trace in &controller_traces {
        summarize_trace(trace, &mut summary)?;
    }
    summary.wire_overhead_percent = if summary.useful_bytes == 0 {
        0.0
    } else {
        ((u64_to_f64(summary.wire_bytes) / u64_to_f64(summary.useful_bytes)) - 1.0) * 100.0
    };
    let adaptive_behavior_valid = summary.pairs_backed_off == controller_traces.len()
        && summary.pairs_recovered == controller_traces.len();

    Ok(SevereWanBenchmarkReport {
        schema_version: REPORT_SCHEMA_VERSION,
        profile,
        paired_report: paired_report.clone(),
        controller_traces,
        controller_summary: summary,
        adaptive_behavior_valid,
    })
}

fn validate_profile(profile: SevereWanProfile) -> Result<(), SevereWanReportError> {
    if profile.rtt_millis != SEVERE_WAN_RTT_MILLIS {
        return Err(SevereWanReportError::WrongRtt);
    }
    if profile.loss_basis_points != SEVERE_WAN_LOSS_BASIS_POINTS {
        return Err(SevereWanReportError::WrongLoss);
    }
    if profile.bandwidth_mbps == 0 {
        return Err(SevereWanReportError::MissingThrottle);
    }
    Ok(())
}

fn validate_paired_report(report: &PairedBenchmarkReport) -> Result<(), SevereWanReportError> {
    if report.scenario != SEVERE_WAN_SCENARIO {
        return Err(SevereWanReportError::WrongScenario {
            actual: report.scenario.clone(),
        });
    }
    if report.samples.len() < SEVERE_WAN_MINIMUM_PAIRS {
        return Err(SevereWanReportError::TooFewPairs {
            actual: report.samples.len(),
        });
    }
    for sample in &report.samples {
        if !sample.output_hash_matches
            || sample.baseline.bytes == 0
            || sample.baseline.bytes != sample.candidate.bytes
            || sample.baseline.sha256 != sample.candidate.sha256
        {
            return Err(SevereWanReportError::OutputMismatch { pair: sample.pair });
        }
    }
    if !report.summary.all_output_hashes_match {
        return Err(SevereWanReportError::SummaryHashMismatch);
    }
    Ok(())
}

fn summarize_trace(
    trace: &AdaptivePairTrace,
    summary: &mut AdaptiveControllerSummary,
) -> Result<(), SevereWanReportError> {
    let mut previous_interval = None;
    let mut backoff_target = None;
    let mut recovered = false;
    for step in &trace.steps {
        let valid_interval = previous_interval.is_none_or(|previous| step.interval > previous);
        let valid_targets =
            (1..=32).contains(&step.target_before) && (1..=32).contains(&step.target_after);
        if !valid_interval || !valid_targets || step.wire_bytes < step.useful_bytes {
            return Err(SevereWanReportError::InvalidTrace { pair: trace.pair });
        }
        previous_interval = Some(step.interval);
        summary.retries = summary.retries.saturating_add(u64::from(step.retries));
        summary.useful_bytes = summary.useful_bytes.saturating_add(step.useful_bytes);
        summary.wire_bytes = summary.wire_bytes.saturating_add(step.wire_bytes);
        summary.maximum_target_connections = summary
            .maximum_target_connections
            .max(step.target_before)
            .max(step.target_after);
        if step.network_loss || step.throttled {
            summary.pressure_events = summary.pressure_events.saturating_add(1);
            summary.loss_events = summary
                .loss_events
                .saturating_add(u64::from(step.network_loss));
            summary.throttle_events = summary
                .throttle_events
                .saturating_add(u64::from(step.throttled));
            if step.target_after < step.target_before {
                backoff_target = Some(step.target_after);
            }
        } else if backoff_target.is_some_and(|target| step.target_after > target) {
            recovered = true;
        }
    }
    if backoff_target.is_none() || !recovered {
        return Err(SevereWanReportError::MissingAdaptiveResponse { pair: trace.pair });
    }
    summary.pairs_backed_off += 1;
    summary.pairs_recovered += 1;
    Ok(())
}
