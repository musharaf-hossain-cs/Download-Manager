//! Streaming file integrity calculation and optional expected-hash comparison.

use std::{
    fmt,
    fs::File,
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use thiserror::Error;

const BUFFER_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl HashAlgorithm {
    #[must_use]
    pub const fn hex_length(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }

    /// MD5 and SHA-1 are supported only for compatibility comparisons, not security assurance.
    #[must_use]
    pub const fn is_cryptographically_weak(self) -> bool {
        matches!(self, Self::Md5 | Self::Sha1)
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerificationControl {
    cancelled: Arc<AtomicBool>,
}

impl VerificationControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationOutcome {
    pub algorithm: HashAlgorithm,
    pub actual: String,
    pub bytes_verified: u64,
}

#[derive(Debug, Error)]
pub enum VerificationError {
    #[error("file verification failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("verification was cancelled after {bytes_verified} bytes")]
    Cancelled { bytes_verified: u64 },
    #[error("invalid expected {algorithm} hash: {value}")]
    InvalidExpectedHash {
        algorithm: HashAlgorithm,
        value: String,
    },
    #[error("{algorithm} mismatch: expected {expected}, got {actual}")]
    Mismatch {
        algorithm: HashAlgorithm,
        expected: String,
        actual: String,
    },
}

/// Streams a file through the selected digest and optionally compares a user-provided value.
///
/// Expected values may be uppercase or lowercase raw hex, or prefixed as `algorithm:hex`.
///
/// # Errors
/// Returns an I/O, cancellation, invalid-format, or mismatch error.
///
/// # Panics
/// Panics only on a platform where an in-memory buffer length cannot fit in `u64`.
pub fn verify_file<F>(
    path: &Path,
    algorithm: HashAlgorithm,
    expected: Option<&str>,
    control: &VerificationControl,
    mut on_progress: F,
) -> Result<VerificationOutcome, VerificationError>
where
    F: FnMut(u64),
{
    let expected = expected
        .map(|value| normalize_expected(algorithm, value))
        .transpose()?;
    let mut file = File::open(path)?;
    let mut hasher = StreamingHasher::new(algorithm);
    let mut bytes_verified = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        if control.is_cancelled() {
            return Err(VerificationError::Cancelled { bytes_verified });
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes_verified += u64::try_from(count).expect("buffer count fits u64");
        on_progress(bytes_verified);
    }
    let actual = hex(&hasher.finalize());
    if let Some(expected) = expected
        && expected != actual
    {
        return Err(VerificationError::Mismatch {
            algorithm,
            expected,
            actual,
        });
    }
    Ok(VerificationOutcome {
        algorithm,
        actual,
        bytes_verified,
    })
}

fn normalize_expected(algorithm: HashAlgorithm, value: &str) -> Result<String, VerificationError> {
    let trimmed = value.trim();
    let raw = trimmed.split_once(':').map_or(trimmed, |(prefix, hash)| {
        if prefix.eq_ignore_ascii_case(algorithm.label())
            || (algorithm == HashAlgorithm::Sha1 && prefix.eq_ignore_ascii_case("sha-1"))
            || (algorithm == HashAlgorithm::Sha256 && prefix.eq_ignore_ascii_case("sha-256"))
            || (algorithm == HashAlgorithm::Sha512 && prefix.eq_ignore_ascii_case("sha-512"))
        {
            hash.trim()
        } else {
            ""
        }
    });
    if raw.len() != algorithm.hex_length() || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(VerificationError::InvalidExpectedHash {
            algorithm,
            value: value.to_owned(),
        });
    }
    Ok(raw.to_ascii_lowercase())
}

enum StreamingHasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
}

impl StreamingHasher {
    fn new(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Md5 => Self::Md5(Md5::new()),
            HashAlgorithm::Sha1 => Self::Sha1(Sha1::new()),
            HashAlgorithm::Sha256 => Self::Sha256(Sha256::new()),
            HashAlgorithm::Sha512 => Self::Sha512(Sha512::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Md5(hasher) => hasher.update(bytes),
            Self::Sha1(hasher) => hasher.update(bytes),
            Self::Sha256(hasher) => hasher.update(bytes),
            Self::Sha512(hasher) => hasher.update(bytes),
        }
    }

    fn finalize(self) -> Vec<u8> {
        match self {
            Self::Md5(hasher) => hasher.finalize().to_vec(),
            Self::Sha1(hasher) => hasher.finalize().to_vec(),
            Self::Sha256(hasher) => hasher.finalize().to_vec(),
            Self::Sha512(hasher) => hasher.finalize().to_vec(),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            std::fmt::Write::write_fmt(&mut output, format_args!("{byte:02x}"))
                .expect("write to string");
            output
        },
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{HashAlgorithm, VerificationControl, VerificationError, verify_file};

    #[test]
    fn sha256_correct_wrong_and_cancel_during_verify() {
        let directory = tempdir().expect("directory");
        let path = directory.path().join("body.bin");
        fs::write(&path, vec![0x5a; 300_000]).expect("body");
        let expected = "9fc75d3f01b2a06f4232078a136c2297f854c4797fce6c4dc58e1d88589761af";
        let outcome = verify_file(
            &path,
            HashAlgorithm::Sha256,
            Some(expected),
            &VerificationControl::default(),
            |_| {},
        )
        .expect("matching hash");
        assert_eq!(outcome.actual, expected);

        let mismatch = verify_file(
            &path,
            HashAlgorithm::Sha256,
            Some(&"0".repeat(64)),
            &VerificationControl::default(),
            |_| {},
        )
        .expect_err("wrong hash");
        assert!(matches!(mismatch, VerificationError::Mismatch { .. }));

        let control = VerificationControl::default();
        let callback_control = control.clone();
        let cancelled = verify_file(&path, HashAlgorithm::Sha256, None, &control, move |_| {
            callback_control.cancel();
        })
        .expect_err("cancelled");
        assert!(matches!(
            cancelled,
            VerificationError::Cancelled { bytes_verified } if bytes_verified > 0 && bytes_verified < 300_000
        ));
    }

    #[test]
    fn user_hash_algorithm_format_and_mismatch_table() {
        let directory = tempdir().expect("directory");
        let path = directory.path().join("abc.txt");
        fs::write(&path, b"abc").expect("body");
        let cases = [
            (HashAlgorithm::Md5, "MD5:900150983CD24FB0D6963F7D28E17F72"),
            (
                HashAlgorithm::Sha1,
                "sha-1:a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                HashAlgorithm::Sha256,
                "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
            ),
            (
                HashAlgorithm::Sha512,
                "sha512:ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ];
        for (algorithm, expected) in cases {
            verify_file(
                &path,
                algorithm,
                Some(expected),
                &VerificationControl::default(),
                |_| {},
            )
            .expect("matching user hash");
        }
        assert!(HashAlgorithm::Md5.is_cryptographically_weak());
        assert!(HashAlgorithm::Sha1.is_cryptographically_weak());
        assert!(!HashAlgorithm::Sha256.is_cryptographically_weak());
        assert!(matches!(
            verify_file(
                &path,
                HashAlgorithm::Sha1,
                Some("not-a-hash"),
                &VerificationControl::default(),
                |_| {}
            ),
            Err(VerificationError::InvalidExpectedHash { .. })
        ));
    }
}
