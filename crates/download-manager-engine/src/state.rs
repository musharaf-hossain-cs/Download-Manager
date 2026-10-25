//! Canonical download state machine and stable event contracts.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{JobId, UnixMillis};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Created,
    Probing,
    Queued,
    Connecting,
    Downloading,
    Pausing,
    Paused,
    Verifying,
    Assembling,
    Completed,
    RetryWaiting,
    NeedsAuthentication,
    NeedsRefresh,
    Blocked,
    Failed,
    Cancelled,
    MissingLocalFile,
}

impl DownloadState {
    pub const ALL: [Self; 17] = [
        Self::Created,
        Self::Probing,
        Self::Queued,
        Self::Connecting,
        Self::Downloading,
        Self::Pausing,
        Self::Paused,
        Self::Verifying,
        Self::Assembling,
        Self::Completed,
        Self::RetryWaiting,
        Self::NeedsAuthentication,
        Self::NeedsRefresh,
        Self::Blocked,
        Self::Failed,
        Self::Cancelled,
        Self::MissingLocalFile,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Probing => "probing",
            Self::Queued => "queued",
            Self::Connecting => "connecting",
            Self::Downloading => "downloading",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Verifying => "verifying",
            Self::Assembling => "assembling",
            Self::Completed => "completed",
            Self::RetryWaiting => "retry_waiting",
            Self::NeedsAuthentication => "needs_authentication",
            Self::NeedsRefresh => "needs_refresh",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::MissingLocalFile => "missing_local_file",
        }
    }
}

impl fmt::Display for DownloadState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for DownloadState {
    type Err = StateParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or_else(|| StateParseError(value.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown download state: {0}")]
pub struct StateParseError(String);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("illegal download transition from {from} to {to}")]
pub struct TransitionError {
    pub from: DownloadState,
    pub to: DownloadState,
}

/// Applies a transition only when it is part of the canonical state graph.
///
/// # Errors
/// Returns [`TransitionError`] when the requested edge is not legal.
pub fn reduce_state(
    current: DownloadState,
    next: DownloadState,
) -> Result<DownloadState, TransitionError> {
    if is_legal_transition(current, next) {
        Ok(next)
    } else {
        Err(TransitionError {
            from: current,
            to: next,
        })
    }
}

#[must_use]
pub const fn is_legal_transition(from: DownloadState, to: DownloadState) -> bool {
    use DownloadState::{
        Assembling, Blocked, Cancelled, Completed, Connecting, Created, Downloading, Failed,
        MissingLocalFile, NeedsAuthentication, NeedsRefresh, Paused, Pausing, Probing, Queued,
        RetryWaiting, Verifying,
    };

    matches!(
        (from, to),
        (Created, Probing | Cancelled)
            | (
                Probing,
                Queued | NeedsAuthentication | Blocked | Failed | Cancelled
            )
            | (Queued, Connecting | Cancelled | Paused)
            | (
                Connecting,
                Downloading
                    | RetryWaiting
                    | NeedsAuthentication
                    | NeedsRefresh
                    | Blocked
                    | Failed
                    | Cancelled
            )
            | (
                Downloading,
                Pausing
                    | Verifying
                    | RetryWaiting
                    | NeedsAuthentication
                    | NeedsRefresh
                    | Blocked
                    | Failed
                    | Cancelled
            )
            | (Pausing, Paused | Failed | Cancelled)
            | (Paused, Queued | Connecting | Cancelled | MissingLocalFile)
            | (
                Verifying,
                Assembling | Failed | Cancelled | MissingLocalFile
            )
            | (
                Assembling,
                Completed | Failed | Cancelled | MissingLocalFile
            )
            | (Completed, MissingLocalFile)
            | (
                RetryWaiting,
                Connecting | NeedsAuthentication | NeedsRefresh | Failed | Cancelled
            )
            | (
                NeedsAuthentication | NeedsRefresh | Blocked,
                Queued | Connecting | Failed | Cancelled
            )
            | (Failed | MissingLocalFile, Queued | Cancelled)
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateTransition {
    pub job_id: JobId,
    pub from: DownloadState,
    pub to: DownloadState,
    pub at: UnixMillis,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DownloadEvent {
    StateChanged {
        transition: StateTransition,
    },
    Progress {
        job_id: JobId,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        at: UnixMillis,
    },
    Error {
        job_id: JobId,
        code: String,
        message: String,
        retryable: bool,
        at: UnixMillis,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{DownloadEvent, DownloadState, StateTransition, is_legal_transition, reduce_state};
    use crate::domain::{JobId, UnixMillis};

    #[test]
    fn transition_table_is_exhaustive() {
        let legal: HashSet<(DownloadState, DownloadState)> = [
            (DownloadState::Created, DownloadState::Probing),
            (DownloadState::Created, DownloadState::Cancelled),
            (DownloadState::Probing, DownloadState::Queued),
            (DownloadState::Queued, DownloadState::Connecting),
            (DownloadState::Connecting, DownloadState::Downloading),
            (DownloadState::Downloading, DownloadState::Pausing),
            (DownloadState::Pausing, DownloadState::Paused),
            (DownloadState::Paused, DownloadState::Connecting),
            (DownloadState::Downloading, DownloadState::Verifying),
            (DownloadState::Verifying, DownloadState::Assembling),
            (DownloadState::Assembling, DownloadState::Completed),
        ]
        .into_iter()
        .collect();

        for from in DownloadState::ALL {
            for to in DownloadState::ALL {
                let expected = legal.contains(&(from, to)) || is_other_legal_transition(from, to);
                assert_eq!(
                    is_legal_transition(from, to),
                    expected,
                    "transition {from} -> {to}"
                );
                assert_eq!(reduce_state(from, to).is_ok(), expected);
            }
        }
    }

    fn is_other_legal_transition(from: DownloadState, to: DownloadState) -> bool {
        use DownloadState::{
            Blocked, Cancelled, Completed, Connecting, Downloading, Failed, MissingLocalFile,
            NeedsAuthentication, NeedsRefresh, Paused, Probing, Queued, RetryWaiting, Verifying,
        };
        matches!(
            (from, to),
            (Probing, NeedsAuthentication | Blocked | Failed | Cancelled)
                | (Queued, Paused | Cancelled)
                | (
                    Connecting | Downloading,
                    RetryWaiting
                        | NeedsAuthentication
                        | NeedsRefresh
                        | Blocked
                        | Failed
                        | Cancelled
                )
                | (DownloadState::Pausing, Failed | Cancelled)
                | (Paused, Queued | Cancelled | MissingLocalFile)
                | (
                    Verifying | DownloadState::Assembling,
                    Failed | Cancelled | MissingLocalFile
                )
                | (Completed, MissingLocalFile)
                | (
                    RetryWaiting,
                    Connecting | NeedsAuthentication | NeedsRefresh | Failed | Cancelled
                )
                | (
                    NeedsAuthentication | NeedsRefresh | Blocked,
                    Queued | Connecting | Failed | Cancelled
                )
                | (Failed | MissingLocalFile, Queued | Cancelled)
        )
    }

    #[test]
    fn state_event_json_is_stable() {
        let job_id: JobId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd".parse().expect("ID");
        let event = DownloadEvent::StateChanged {
            transition: StateTransition {
                job_id,
                from: DownloadState::Created,
                to: DownloadState::Probing,
                at: UnixMillis::new(42).expect("timestamp"),
                reason: None,
            },
        };
        assert_eq!(
            serde_json::to_string(&event).expect("serialize"),
            r#"{"type":"state_changed","transition":{"job_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","from":"created","to":"probing","at":42,"reason":null}}"#
        );
    }
}
