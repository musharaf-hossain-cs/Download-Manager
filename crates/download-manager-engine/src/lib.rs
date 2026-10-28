#![doc = "Core transfer-engine boundary for Download Manager."]

pub mod adaptive;
pub mod audio_output;
pub mod auth;
pub mod benchmark;
pub mod checkpoint;
pub mod credentials;
pub mod dash;
pub mod dash_download;
pub mod desktop;
pub mod disk;
pub mod domain;
pub mod fairness;
pub mod filename;
pub mod finalize;
pub mod hls;
pub mod integrity;
pub mod ipc;
pub mod limits;
pub mod media;
pub mod media_mux;
pub mod media_refresh;
pub mod media_selection;
pub mod media_worker;
pub mod network;
pub mod network_metrics;
pub mod probe;
pub mod proxy;
pub mod queue_schedule;
pub mod queue_stop;
pub mod recovery;
pub mod request_context;
pub mod resume;
pub mod retry;
pub mod scheduler;
pub mod segmented;
pub mod state;
pub mod storage;
pub mod subtitle;
pub mod transfer;

#[cfg(test)]
pub(crate) mod test_support;

/// The engine version, kept in sync with the Cargo package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns a stable component/version pair for IPC health checks.
#[must_use]
pub const fn version_banner() -> &'static str {
    concat!("download-manager-engine ", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{VERSION, version_banner};

    #[test]
    fn version_contract_is_stable() {
        assert_eq!(VERSION, "0.1.0");
        assert_eq!(version_banner(), "download-manager-engine 0.1.0");
    }
}
