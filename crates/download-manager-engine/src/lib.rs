#![doc = "Core transfer-engine boundary for Download Manager."]

pub mod adaptive;
pub mod antivirus;
pub mod audio_output;
pub mod auth;
pub mod bandwidth_schedule;
pub mod benchmark;
pub mod benchmark_lan;
pub mod benchmark_runner;
pub mod checkpoint;
pub mod credentials;
pub mod dash;
pub mod dash_download;
pub mod desktop;
pub mod diagnostics;
pub mod disk;
pub mod domain;
pub mod export_archive;
pub mod fairness;
pub mod filename;
pub mod finalize;
pub mod grabber;
pub mod grabber_css;
pub mod grabber_filter;
pub mod grabber_limits;
pub mod grabber_offline;
pub mod grabber_policy;
pub mod grabber_sync;
pub mod hls;
pub mod http_range;
pub mod import_archive;
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
pub mod post_processing;
pub mod power_action;
pub mod probe;
pub mod proxy;
pub mod queue_schedule;
pub mod queue_stop;
pub mod recovery;
pub mod redaction;
pub mod request_context;
pub mod resume;
pub mod retry;
pub mod route_reconnect;
pub mod scheduled_wake;
pub mod scheduler;
pub mod segmented;
pub mod state;
pub mod storage;
pub mod structured_log;
pub mod subtitle;
pub mod system_policy;
pub mod transfer;
pub mod transfer_quota;
pub mod update_activation;
pub mod update_staging;
pub mod update_verification;

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
