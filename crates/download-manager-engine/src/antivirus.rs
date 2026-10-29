//! Job-bound antivirus scanning through a mockable registered-scanner boundary.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use std::{
    ffi::OsString,
    process::{Command, Stdio},
};

use thiserror::Error;

use crate::domain::JobId;

const MAX_SCANNER_NAME_CHARS: usize = 128;

/// Fail-closed interpretation of a registered scanner's process result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanVerdict {
    Clean,
    /// A threat, scanner failure, or other non-success result needs user attention.
    AttentionRequired,
}

/// Raw result returned by one registered scanner invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScannerResult {
    pub verdict: ScanVerdict,
    pub exit_code: Option<i32>,
}

/// Stable scan evidence associated with the completed download job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobScanRecord {
    pub job_id: JobId,
    pub scanner_name: String,
    pub verdict: ScanVerdict,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Error)]
pub enum AntivirusScanError {
    #[error("scanner name must be trimmed, non-empty, control-free, and at most 128 characters")]
    InvalidScannerName,
    #[error("scan target must be an absolute local path: {0}")]
    NonAbsoluteTarget(PathBuf),
    #[error("scan target cannot be opened: {path}")]
    TargetUnavailable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("scan target is not a regular file: {0}")]
    TargetNotFile(PathBuf),
    #[error("registered antivirus scanning is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("Microsoft Defender's registered command-line scanner is unavailable")]
    ScannerUnavailable,
    #[error("failed to launch the registered antivirus scanner")]
    Launch(#[source] io::Error),
}

/// Boundary implemented by a trusted, OS-registered antivirus provider.
pub trait AntivirusScannerAdapter {
    /// Human-readable registered product name stored with the job result.
    fn scanner_name(&self) -> &str;

    /// Scans one canonical, absolute regular-file path without using a shell.
    ///
    /// # Errors
    /// Returns an adapter error when the scanner cannot be invoked. Non-success scanner exits
    /// should instead return [`ScanVerdict::AttentionRequired`] so the result can be recorded.
    fn scan_file(&mut self, canonical_path: &Path) -> Result<ScannerResult, AntivirusScanError>;
}

/// Validates a completed file, invokes the registered scanner, and binds its result to the job.
///
/// The target is canonicalized before the adapter sees it. A non-success scanner result remains a
/// successful call with an attention-required record; callers must not treat it as a clean file.
///
/// # Errors
/// Rejects invalid scanner identity or file paths, and propagates adapter launch failures.
pub fn scan_completed_download(
    job_id: JobId,
    target: &Path,
    adapter: &mut dyn AntivirusScannerAdapter,
) -> Result<JobScanRecord, AntivirusScanError> {
    validate_scanner_name(adapter.scanner_name())?;
    if !target.is_absolute() {
        return Err(AntivirusScanError::NonAbsoluteTarget(target.to_path_buf()));
    }

    let canonical_path =
        fs::canonicalize(target).map_err(|source| AntivirusScanError::TargetUnavailable {
            path: target.to_path_buf(),
            source,
        })?;
    let metadata =
        fs::metadata(&canonical_path).map_err(|source| AntivirusScanError::TargetUnavailable {
            path: canonical_path.clone(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(AntivirusScanError::TargetNotFile(canonical_path));
    }

    let scanner_name = adapter.scanner_name().to_owned();
    let result = adapter.scan_file(&canonical_path)?;
    Ok(JobScanRecord {
        job_id,
        scanner_name,
        verdict: result.verdict,
        exit_code: result.exit_code,
    })
}

fn validate_scanner_name(name: &str) -> Result<(), AntivirusScanError> {
    if name.is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_SCANNER_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        Err(AntivirusScanError::InvalidScannerName)
    } else {
        Ok(())
    }
}

/// Microsoft Defender adapter discovered from Windows' registered installation directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsDefenderScanner {
    executable: PathBuf,
}

impl WindowsDefenderScanner {
    /// Finds the current Defender platform scanner, falling back to its stable installation path.
    ///
    /// # Errors
    /// Returns a stable unsupported/unavailable result without invoking another executable.
    pub fn discover() -> Result<Self, AntivirusScanError> {
        #[cfg(windows)]
        {
            discover_defender_executable().map(|executable| Self { executable })
        }

        #[cfg(not(windows))]
        {
            Err(AntivirusScanError::UnsupportedPlatform)
        }
    }

    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl AntivirusScannerAdapter for WindowsDefenderScanner {
    fn scanner_name(&self) -> &'static str {
        "Microsoft Defender Antivirus"
    }

    fn scan_file(&mut self, canonical_path: &Path) -> Result<ScannerResult, AntivirusScanError> {
        #[cfg(windows)]
        {
            let status = Command::new(&self.executable)
                .args(defender_arguments(canonical_path))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(AntivirusScanError::Launch)?;
            Ok(ScannerResult {
                verdict: if status.success() {
                    ScanVerdict::Clean
                } else {
                    ScanVerdict::AttentionRequired
                },
                exit_code: status.code(),
            })
        }

        #[cfg(not(windows))]
        {
            let _ = canonical_path;
            Err(AntivirusScanError::UnsupportedPlatform)
        }
    }
}

#[cfg(windows)]
fn defender_arguments(target: &Path) -> [OsString; 6] {
    [
        OsString::from("-Scan"),
        OsString::from("-ScanType"),
        OsString::from("3"),
        OsString::from("-File"),
        target.as_os_str().to_owned(),
        OsString::from("-DisableRemediation"),
    ]
}

#[cfg(windows)]
fn discover_defender_executable() -> Result<PathBuf, AntivirusScanError> {
    let mut candidates = Vec::new();
    if let Some(program_data) = std::env::var_os("ProgramData") {
        let platform = PathBuf::from(program_data)
            .join("Microsoft")
            .join("Windows Defender")
            .join("Platform");
        if let Ok(entries) = fs::read_dir(platform) {
            candidates.extend(entries.filter_map(Result::ok).filter_map(|entry| {
                let executable = entry.path().join("MpCmdRun.exe");
                executable.is_file().then_some(executable)
            }));
        }
    }
    candidates.sort();
    candidates.reverse();

    for variable in ["ProgramW6432", "ProgramFiles"] {
        if let Some(program_files) = std::env::var_os(variable) {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Windows Defender")
                    .join("MpCmdRun.exe"),
            );
        }
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or(AntivirusScanError::ScannerUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MockScanner {
        name: String,
        result: ScannerResult,
        scanned: Vec<PathBuf>,
    }

    impl AntivirusScannerAdapter for MockScanner {
        fn scanner_name(&self) -> &str {
            &self.name
        }

        fn scan_file(
            &mut self,
            canonical_path: &Path,
        ) -> Result<ScannerResult, AntivirusScanError> {
            self.scanned.push(canonical_path.to_path_buf());
            Ok(self.result)
        }
    }

    fn mock(result: ScannerResult) -> MockScanner {
        MockScanner {
            name: "Registered Test Antivirus".to_owned(),
            result,
            scanned: Vec::new(),
        }
    }

    #[test]
    fn harmless_local_file_scan_records_clean_result_on_exact_job() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("harmless download.txt");
        fs::write(&target, b"harmless local scanner workflow").expect("write fixture");
        let job_id = JobId::new();
        let mut scanner = mock(ScannerResult {
            verdict: ScanVerdict::Clean,
            exit_code: Some(0),
        });

        let record = scan_completed_download(job_id, &target, &mut scanner).expect("scan");

        assert_eq!(record.job_id, job_id);
        assert_eq!(record.scanner_name, "Registered Test Antivirus");
        assert_eq!(record.verdict, ScanVerdict::Clean);
        assert_eq!(record.exit_code, Some(0));
        assert_eq!(
            scanner.scanned,
            [fs::canonicalize(target).expect("canonical")]
        );
    }

    #[test]
    fn non_success_scan_is_recorded_fail_closed_instead_of_clean() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("attention.bin");
        fs::write(&target, b"scanner-controlled fixture").expect("write fixture");
        let mut scanner = mock(ScannerResult {
            verdict: ScanVerdict::AttentionRequired,
            exit_code: Some(2),
        });

        let record = scan_completed_download(JobId::new(), &target, &mut scanner).expect("scan");

        assert_eq!(record.verdict, ScanVerdict::AttentionRequired);
        assert_eq!(record.exit_code, Some(2));
    }

    #[test]
    fn invalid_target_and_scanner_identity_fail_before_adapter_use() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut scanner = mock(ScannerResult {
            verdict: ScanVerdict::Clean,
            exit_code: Some(0),
        });

        assert!(matches!(
            scan_completed_download(JobId::new(), Path::new("relative.bin"), &mut scanner),
            Err(AntivirusScanError::NonAbsoluteTarget(_))
        ));
        assert!(matches!(
            scan_completed_download(JobId::new(), directory.path(), &mut scanner),
            Err(AntivirusScanError::TargetNotFile(_))
        ));
        scanner.name = " registered".to_owned();
        assert!(matches!(
            scan_completed_download(JobId::new(), directory.path(), &mut scanner),
            Err(AntivirusScanError::InvalidScannerName)
        ));
        assert_eq!(scanner.scanned, Vec::<PathBuf>::new());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "invokes the locally registered Microsoft Defender scanner"]
    fn windows_defender_scans_a_harmless_local_file() {
        let Ok(mut scanner) = WindowsDefenderScanner::discover() else {
            return;
        };
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("download-manager-harmless-smoke.txt");
        fs::write(&target, b"Download Manager harmless antivirus smoke test\n")
            .expect("write harmless fixture");

        let record = scan_completed_download(JobId::new(), &target, &mut scanner).expect("scan");

        assert_eq!(record.scanner_name, "Microsoft Defender Antivirus");
        assert_eq!(record.verdict, ScanVerdict::Clean);
        assert_eq!(record.exit_code, Some(0));
    }
}
