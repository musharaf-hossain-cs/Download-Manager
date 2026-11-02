//! Acceptance evidence for one profiled shaped-WAN receive-loop regression and its fix.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::benchmark_runner::PairedBenchmarkReport;

pub const WAN_POLL_REGRESSION_SCENARIO: &str = "wan-fixed-poll-delay-regression";
pub const WAN_REGRESSION_RTT_MILLIS: u16 = 80;
pub const WAN_REGRESSION_MINIMUM_PAIRS: usize = 10;
pub const BEFORE_FIXED_POLL_DELAY_MILLIS: u16 = 20;

const REPORT_SCHEMA_VERSION: u32 = 1;
const EXPECTED_HIGH_SHARE: u64 = 4;
const EXPECTED_NORMAL_SHARE: u64 = 2;
const EXPECTED_LOW_SHARE: u64 = 1;
const MAXIMUM_LOW_PRIORITY_GAP: u64 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FairnessEvidence {
    pub selections: u64,
    pub high_selections: u64,
    pub normal_selections: u64,
    pub low_selections: u64,
    pub longest_low_priority_gap: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WanRegressionReport {
    pub schema_version: u32,
    pub rtt_millis: u16,
    pub paired_report: PairedBenchmarkReport,
    pub bottleneck: String,
    pub fix: String,
    pub before_fixed_poll_delay_millis: u16,
    pub after_fixed_poll_delay_millis: u16,
    pub median_goodput_improvement_percent: f64,
    pub fairness: FairnessEvidence,
    pub weighted_fairness_preserved: bool,
    pub correctness_preserved: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WanRegressionReportError {
    #[error(
        "WAN regression report used scenario {actual:?}, expected {WAN_POLL_REGRESSION_SCENARIO}"
    )]
    WrongScenario { actual: String },
    #[error("WAN regression report used protocol {actual:?}, expected HTTP/1.1")]
    WrongProtocol { actual: String },
    #[error("WAN regression report requires at least 10 paired runs; found {actual}")]
    TooFewPairs { actual: usize },
    #[error("WAN regression pair {pair} did not produce equal non-empty output hashes and lengths")]
    OutputMismatch { pair: u32 },
    #[error("WAN regression report summary did not confirm all output hashes")]
    SummaryHashMismatch,
    #[error("WAN regression fix did not improve median paired goodput")]
    NoMeasuredImprovement,
    #[error("WAN regression fairness evidence does not preserve 4:2:1 service without starvation")]
    FairnessRegression,
}

impl WanRegressionReport {
    /// Renders the before/after timing and weighted-fairness evidence in a stable form.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let paired = &self.paired_report.summary;
        format!(
            "# WAN regression profile and fix\n\nRTT: **{} ms**; protocol: `{}`  \nBottleneck: **{}**  \nFix: **{}**\n\n| Metric | Before | After |\n|---|---:|---:|\n| Median goodput (B/s) | {:.2} | {:.2} |\n| Fixed receive poll delay | {} ms | {} ms |\n\nMedian paired goodput improvement: **{:+.2}%**\n\n| Fairness evidence | Value |\n|---|---:|\n| High / normal / low selections | {} / {} / {} |\n| Longest low-priority gap | {} |\n| Weighted 4:2:1 fairness preserved | {} |\n\nAll output hashes match: **{}**  \nCorrectness preserved: **{}**\n",
            self.rtt_millis,
            self.paired_report.protocol,
            self.bottleneck,
            self.fix,
            paired.baseline_goodput_bytes_per_second.median,
            paired.candidate_goodput_bytes_per_second.median,
            self.before_fixed_poll_delay_millis,
            self.after_fixed_poll_delay_millis,
            self.median_goodput_improvement_percent,
            self.fairness.high_selections,
            self.fairness.normal_selections,
            self.fairness.low_selections,
            self.fairness.longest_low_priority_gap,
            self.weighted_fairness_preserved,
            paired.all_output_hashes_match,
            self.correctness_preserved,
        )
    }
}

/// Builds auditable before/after evidence for the fixed receive-poll delay regression.
///
/// The fairness sample must contain complete 4:2:1 cycles. This prevents a throughput fix from
/// being accepted by starving normal or low-priority jobs while the WAN is under pressure.
///
/// # Errors
/// Returns an error when paired transfer correctness, improvement, or weighted fairness is absent.
pub fn build_wan_regression_report(
    paired_report: &PairedBenchmarkReport,
    fairness: FairnessEvidence,
) -> Result<WanRegressionReport, WanRegressionReportError> {
    validate_paired_report(paired_report)?;
    if paired_report.summary.paired_goodput_change_percent.median <= 0.0 {
        return Err(WanRegressionReportError::NoMeasuredImprovement);
    }
    if !fairness_is_valid(fairness) {
        return Err(WanRegressionReportError::FairnessRegression);
    }

    Ok(WanRegressionReport {
        schema_version: REPORT_SCHEMA_VERSION,
        rtt_millis: WAN_REGRESSION_RTT_MILLIS,
        paired_report: paired_report.clone(),
        bottleneck: format!(
            "fixed {BEFORE_FIXED_POLL_DELAY_MILLIS} ms sleep after every receive read"
        ),
        fix: "use blocking readiness-driven reads without a fixed polling sleep".to_owned(),
        before_fixed_poll_delay_millis: BEFORE_FIXED_POLL_DELAY_MILLIS,
        after_fixed_poll_delay_millis: 0,
        median_goodput_improvement_percent: paired_report
            .summary
            .paired_goodput_change_percent
            .median,
        fairness,
        weighted_fairness_preserved: true,
        correctness_preserved: true,
    })
}

fn validate_paired_report(report: &PairedBenchmarkReport) -> Result<(), WanRegressionReportError> {
    if report.scenario != WAN_POLL_REGRESSION_SCENARIO {
        return Err(WanRegressionReportError::WrongScenario {
            actual: report.scenario.clone(),
        });
    }
    if report.protocol != "HTTP/1.1" {
        return Err(WanRegressionReportError::WrongProtocol {
            actual: report.protocol.clone(),
        });
    }
    if report.samples.len() < WAN_REGRESSION_MINIMUM_PAIRS {
        return Err(WanRegressionReportError::TooFewPairs {
            actual: report.samples.len(),
        });
    }
    for sample in &report.samples {
        if !sample.output_hash_matches
            || sample.baseline.bytes == 0
            || sample.baseline.bytes != sample.candidate.bytes
            || sample.baseline.sha256 != sample.candidate.sha256
        {
            return Err(WanRegressionReportError::OutputMismatch { pair: sample.pair });
        }
    }
    if !report.summary.all_output_hashes_match {
        return Err(WanRegressionReportError::SummaryHashMismatch);
    }
    Ok(())
}

fn fairness_is_valid(evidence: FairnessEvidence) -> bool {
    let expected_cycle = EXPECTED_HIGH_SHARE + EXPECTED_NORMAL_SHARE + EXPECTED_LOW_SHARE;
    let cycles = evidence.selections / expected_cycle;
    evidence.selections >= expected_cycle
        && evidence.selections.is_multiple_of(expected_cycle)
        && evidence.high_selections == cycles * EXPECTED_HIGH_SHARE
        && evidence.normal_selections == cycles * EXPECTED_NORMAL_SHARE
        && evidence.low_selections == cycles * EXPECTED_LOW_SHARE
        && evidence.high_selections + evidence.normal_selections + evidence.low_selections
            == evidence.selections
        && evidence.longest_low_priority_gap <= MAXIMUM_LOW_PRIORITY_GAP
}
