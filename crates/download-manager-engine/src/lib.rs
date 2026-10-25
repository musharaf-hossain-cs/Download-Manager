#![doc = "Core transfer-engine boundary for Download Manager."]

pub mod checkpoint;
pub mod disk;
pub mod domain;
pub mod filename;
pub mod finalize;
pub mod integrity;
pub mod ipc;
pub mod probe;
pub mod recovery;
pub mod resume;
pub mod retry;
pub mod segmented;
pub mod state;
pub mod storage;
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
