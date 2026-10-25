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
    use super::{BenchmarkMetrics, BenchmarkObservation};

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
}
