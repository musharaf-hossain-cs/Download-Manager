//! Acceptance checks for controlled LAN benchmark reports.

use thiserror::Error;

use crate::benchmark_runner::PairedBenchmarkReport;

pub const CONTROLLED_LAN_MINIMUM_PAIRS: usize = 10;
pub const HTTP_1_1_PROTOCOL: &str = "HTTP/1.1";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ControlledLanReportError {
    #[error("controlled HTTP/1.1 report used protocol {actual:?}")]
    WrongProtocol { actual: String },
    #[error("controlled LAN report requires at least 10 paired runs; found {actual}")]
    TooFewPairs { actual: usize },
    #[error("controlled LAN pair {pair} did not produce equal non-empty output hashes and lengths")]
    OutputMismatch { pair: u32 },
    #[error("controlled LAN report summary did not confirm all output hashes")]
    SummaryHashMismatch,
}

/// Verifies the release evidence required from a controlled HTTP/1.1 LAN run.
///
/// # Errors
/// Returns an error unless the report identifies HTTP/1.1, contains at least ten pairs, and every
/// pair produced equal non-empty output bytes and SHA-256 values.
pub fn validate_http1_lan_report(
    report: &PairedBenchmarkReport,
) -> Result<(), ControlledLanReportError> {
    if report.protocol != HTTP_1_1_PROTOCOL {
        return Err(ControlledLanReportError::WrongProtocol {
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
