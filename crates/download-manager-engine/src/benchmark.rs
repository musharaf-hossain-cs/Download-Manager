//! Stable raw benchmark metrics for engine comparisons.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkObservation {
    pub elapsed_millis: u64,
    pub unique_bytes: u64,
    pub wire_bytes: u64,
    pub cpu_percent: f64,
    pub peak_memory_bytes: u64,
    pub connections: u32,
    pub handshakes: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkMetrics {
    pub elapsed_millis: u64,
    pub goodput_bytes_per_second: u64,
    pub unique_bytes: u64,
    pub wire_bytes: u64,
    pub cpu_percent: f64,
    pub peak_memory_bytes: u64,
    pub connections: u32,
    pub handshakes: u32,
    pub sha256: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BenchmarkError {
    #[error("benchmark elapsed time must be greater than zero")]
    ZeroElapsedTime,
    #[error("benchmark SHA-256 must contain exactly 64 hexadecimal characters")]
    InvalidSha256,
    #[error("baseline goodput must be greater than zero")]
    ZeroBaselineGoodput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedBenchmarkRun {
    pub name: String,
    pub metrics: BenchmarkMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkComparison {
    pub baseline: NamedBenchmarkRun,
    pub candidate: NamedBenchmarkRun,
    pub goodput_change_basis_points: i64,
    pub wire_byte_change: i128,
    pub cpu_change_percent: f64,
    pub peak_memory_change_bytes: i128,
    pub output_hash_matches: bool,
}

impl BenchmarkComparison {
    /// Compares two named observations using integer goodput basis points for reproducibility.
    ///
    /// # Errors
    /// Returns an error when the baseline has zero goodput.
    pub fn compare(
        baseline: NamedBenchmarkRun,
        candidate: NamedBenchmarkRun,
    ) -> Result<Self, BenchmarkError> {
        let baseline_goodput = baseline.metrics.goodput_bytes_per_second;
        if baseline_goodput == 0 {
            return Err(BenchmarkError::ZeroBaselineGoodput);
        }
        let candidate_goodput = candidate.metrics.goodput_bytes_per_second;
        let change = i128::from(candidate_goodput) - i128::from(baseline_goodput);
        let basis_points = change
            .saturating_mul(10_000)
            .saturating_div(i128::from(baseline_goodput));
        let goodput_change_basis_points = i64::try_from(basis_points).unwrap_or_else(|_| {
            if basis_points.is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        });
        Ok(Self {
            goodput_change_basis_points,
            wire_byte_change: i128::from(candidate.metrics.wire_bytes)
                - i128::from(baseline.metrics.wire_bytes),
            cpu_change_percent: candidate.metrics.cpu_percent - baseline.metrics.cpu_percent,
            peak_memory_change_bytes: i128::from(candidate.metrics.peak_memory_bytes)
                - i128::from(baseline.metrics.peak_memory_bytes),
            output_hash_matches: candidate.metrics.sha256 == baseline.metrics.sha256,
            baseline,
            candidate,
        })
    }

    /// Renders a stable, reviewable Markdown summary.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let absolute_change = self.goodput_change_basis_points.unsigned_abs();
        let sign = if self.goodput_change_basis_points >= 0 {
            "+"
        } else {
            "-"
        };
        let percentage = format!(
            "{sign}{}.{:02}%",
            absolute_change / 100,
            absolute_change % 100
        );
        format!(
            "# Benchmark comparison\n\n| Run | Goodput (B/s) | CPU (%) | Peak memory (B) | Connections | Wire bytes |\n|---|---:|---:|---:|---:|---:|\n| {} | {} | {:.2} | {} | {} | {} |\n| {} | {} | {:.2} | {} | {} | {} |\n\nGoodput change: {percentage}\n\nOutput hash matches: {}\n",
            self.baseline.name,
            self.baseline.metrics.goodput_bytes_per_second,
            self.baseline.metrics.cpu_percent,
            self.baseline.metrics.peak_memory_bytes,
            self.baseline.metrics.connections,
            self.baseline.metrics.wire_bytes,
            self.candidate.name,
            self.candidate.metrics.goodput_bytes_per_second,
            self.candidate.metrics.cpu_percent,
            self.candidate.metrics.peak_memory_bytes,
            self.candidate.metrics.connections,
            self.candidate.metrics.wire_bytes,
            self.output_hash_matches,
        )
    }
}

impl BenchmarkMetrics {
    /// Converts one engine observation into a stable serializable metric record.
    ///
    /// # Errors
    /// Returns an error for zero duration or an invalid SHA-256 value.
    pub fn record(observation: &BenchmarkObservation) -> Result<Self, BenchmarkError> {
        if observation.elapsed_millis == 0 {
            return Err(BenchmarkError::ZeroElapsedTime);
        }
        if observation.sha256.len() != 64
            || !observation
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BenchmarkError::InvalidSha256);
        }
        Ok(Self {
            elapsed_millis: observation.elapsed_millis,
            goodput_bytes_per_second: observation.unique_bytes.saturating_mul(1_000)
                / observation.elapsed_millis,
            unique_bytes: observation.unique_bytes,
            wire_bytes: observation.wire_bytes,
            cpu_percent: observation.cpu_percent,
            peak_memory_bytes: observation.peak_memory_bytes,
            connections: observation.connections,
            handshakes: observation.handshakes,
            sha256: observation.sha256.to_ascii_lowercase(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BenchmarkComparison, BenchmarkMetrics, BenchmarkObservation, NamedBenchmarkRun};

    #[test]
    fn raw_metric_json_has_goodput_cpu_memory_connections_and_hash() {
        let metrics = BenchmarkMetrics::record(&BenchmarkObservation {
            elapsed_millis: 2_000,
            unique_bytes: 10_000_000,
            wire_bytes: 10_500_000,
            cpu_percent: 12.5,
            peak_memory_bytes: 64 * 1024 * 1024,
            connections: 8,
            handshakes: 2,
            sha256: "A".repeat(64),
        })
        .expect("metrics");
        let json = serde_json::to_value(&metrics).expect("JSON");
        assert_eq!(json["goodput_bytes_per_second"], 5_000_000);
        assert_eq!(json["cpu_percent"], 12.5);
        assert_eq!(json["peak_memory_bytes"], 64 * 1024 * 1024);
        assert_eq!(json["connections"], 8);
        assert_eq!(json["handshakes"], 2);
        assert_eq!(json["sha256"], "a".repeat(64));
        assert_eq!(json["wire_bytes"], 10_500_000);
    }

    #[test]
    fn two_fixture_runs_produce_reproducible_comparison_report() {
        let hash = "1".repeat(64);
        let baseline = BenchmarkMetrics::record(&BenchmarkObservation {
            elapsed_millis: 2_000,
            unique_bytes: 10_000_000,
            wire_bytes: 10_200_000,
            cpu_percent: 10.0,
            peak_memory_bytes: 50_000_000,
            connections: 4,
            handshakes: 2,
            sha256: hash.clone(),
        })
        .expect("baseline");
        let candidate = BenchmarkMetrics::record(&BenchmarkObservation {
            elapsed_millis: 2_000,
            unique_bytes: 12_000_000,
            wire_bytes: 12_100_000,
            cpu_percent: 11.5,
            peak_memory_bytes: 52_000_000,
            connections: 8,
            handshakes: 2,
            sha256: hash,
        })
        .expect("candidate");
        let make_report = || {
            BenchmarkComparison::compare(
                NamedBenchmarkRun {
                    name: "fixture-baseline".to_owned(),
                    metrics: baseline.clone(),
                },
                NamedBenchmarkRun {
                    name: "fixture-candidate".to_owned(),
                    metrics: candidate.clone(),
                },
            )
            .expect("comparison")
        };
        let report = make_report();
        assert_eq!(report, make_report());
        assert_eq!(report.goodput_change_basis_points, 2_000);
        assert_eq!(report.wire_byte_change, 1_900_000);
        assert!(report.output_hash_matches);
        let markdown = report.render_markdown();
        assert!(markdown.contains("Goodput change: +20.00%"));
        assert!(markdown.contains("fixture-baseline"));
        assert_eq!(markdown, report.render_markdown());
    }
}
