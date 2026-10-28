//! Metered, roaming, and battery transfer policy behind a mockable Windows-state boundary.

use std::num::{NonZeroU16, NonZeroU64};

use thiserror::Error;

use crate::{domain::JobId, state::DownloadState};

/// The Windows connection-cost classification relevant to transfer policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkCost {
    Unrestricted,
    Metered,
    Roaming,
}

/// The current source of system power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Battery,
}

/// One atomic observation from the Windows network-cost and power APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsSystemState {
    pub network_cost: NetworkCost,
    pub power_source: PowerSource,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SystemPolicyError {
    #[error("Windows system state is unavailable: {0}")]
    StateUnavailable(String),
}

/// Adapter boundary for Windows network-cost, roaming, and battery observations.
///
/// The platform host owns the OS API calls and supplies a single consistent snapshot. Keeping the
/// calls outside the policy evaluator makes policy decisions deterministic and mockable.
pub trait WindowsSystemStateAdapter {
    /// Reads the latest Windows system state.
    ///
    /// # Errors
    /// Returns [`SystemPolicyError::StateUnavailable`] when a complete snapshot cannot be read.
    fn snapshot(&mut self) -> Result<WindowsSystemState, SystemPolicyError>;
}

/// User choice for a restricted network class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestrictedNetworkPolicy {
    Ask,
    Pause,
    Allow,
}

/// Transfer changes to apply while Windows reports battery power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryPolicy {
    Unchanged,
    Reduce {
        max_bytes_per_second: NonZeroU64,
        max_connections: NonZeroU16,
    },
    PauseLarge {
        minimum_total_bytes: NonZeroU64,
    },
}

/// Configured network and power behavior. Roaming is fail-safe by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemPolicyConfig {
    pub metered: RestrictedNetworkPolicy,
    pub roaming: RestrictedNetworkPolicy,
    pub battery: BatteryPolicy,
}

impl Default for SystemPolicyConfig {
    fn default() -> Self {
        Self {
            metered: RestrictedNetworkPolicy::Ask,
            roaming: RestrictedNetworkPolicy::Pause,
            battery: BatteryPolicy::Unchanged,
        }
    }
}

/// Ownership recorded with a paused job. Only system-policy-owned pauses may auto-resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseOwner {
    User,
    SystemPolicy,
    Other,
}

/// Job fields needed to decide policy without mutating the canonical state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyJobSnapshot {
    pub job_id: JobId,
    pub state: DownloadState,
    pub total_bytes: Option<u64>,
    pub pause_owner: Option<PauseOwner>,
}

/// Why a job needs policy intervention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPolicyReason {
    Metered,
    Roaming,
    Battery,
}

/// State-machine intent emitted by policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyControl {
    NoChange,
    Ask { reason: SystemPolicyReason },
    Pause { reason: SystemPolicyReason },
    Resume,
}

/// Optional transfer constraints emitted alongside a control decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyTransferLimits {
    pub max_bytes_per_second: NonZeroU64,
    pub max_connections: NonZeroU16,
}

/// Complete policy result for one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPolicyDecision {
    pub job_id: JobId,
    pub control: PolicyControl,
    pub limits: Option<PolicyTransferLimits>,
}

/// Reads one platform snapshot and evaluates all supplied jobs against it.
///
/// Policy evaluation never mutates download state. A caller safely checkpoints a transfer before
/// applying [`PolicyControl::Pause`] and records [`PauseOwner::SystemPolicy`]. Automatic resume is
/// emitted only for that owner; user and other pauses remain untouched.
///
/// # Errors
/// Returns the adapter error without emitting any decisions when Windows state is unavailable.
pub fn evaluate_system_policy(
    adapter: &mut dyn WindowsSystemStateAdapter,
    config: SystemPolicyConfig,
    jobs: &[PolicyJobSnapshot],
) -> Result<Vec<JobPolicyDecision>, SystemPolicyError> {
    let system_state = adapter.snapshot()?;
    Ok(jobs
        .iter()
        .map(|job| evaluate_job(config, system_state, *job))
        .collect())
}

fn evaluate_job(
    config: SystemPolicyConfig,
    system_state: WindowsSystemState,
    job: PolicyJobSnapshot,
) -> JobPolicyDecision {
    let network_control = match system_state.network_cost {
        NetworkCost::Unrestricted => None,
        NetworkCost::Metered => network_control(config.metered, SystemPolicyReason::Metered),
        NetworkCost::Roaming => network_control(config.roaming, SystemPolicyReason::Roaming),
    };
    let (battery_control, limits) = battery_decision(config.battery, system_state, job);
    let restriction = network_control.or(battery_control);

    let control = match restriction {
        Some(PolicyControl::Pause { reason }) if is_running(job.state) => {
            PolicyControl::Pause { reason }
        }
        Some(PolicyControl::Ask { reason })
            if is_running(job.state) || is_policy_paused(job.state, job.pause_owner) =>
        {
            PolicyControl::Ask { reason }
        }
        None if is_policy_paused(job.state, job.pause_owner) => PolicyControl::Resume,
        Some(_) | None => PolicyControl::NoChange,
    };

    JobPolicyDecision {
        job_id: job.job_id,
        control,
        limits,
    }
}

const fn network_control(
    policy: RestrictedNetworkPolicy,
    reason: SystemPolicyReason,
) -> Option<PolicyControl> {
    match policy {
        RestrictedNetworkPolicy::Ask => Some(PolicyControl::Ask { reason }),
        RestrictedNetworkPolicy::Pause => Some(PolicyControl::Pause { reason }),
        RestrictedNetworkPolicy::Allow => None,
    }
}

fn battery_decision(
    policy: BatteryPolicy,
    state: WindowsSystemState,
    job: PolicyJobSnapshot,
) -> (Option<PolicyControl>, Option<PolicyTransferLimits>) {
    if state.power_source == PowerSource::Ac {
        return (None, None);
    }
    match policy {
        BatteryPolicy::Reduce {
            max_bytes_per_second,
            max_connections,
        } => (
            None,
            Some(PolicyTransferLimits {
                max_bytes_per_second,
                max_connections,
            }),
        ),
        BatteryPolicy::PauseLarge {
            minimum_total_bytes,
        } if job
            .total_bytes
            .is_some_and(|total| total >= minimum_total_bytes.get()) =>
        {
            (
                Some(PolicyControl::Pause {
                    reason: SystemPolicyReason::Battery,
                }),
                None,
            )
        }
        BatteryPolicy::Unchanged | BatteryPolicy::PauseLarge { .. } => (None, None),
    }
}

const fn is_running(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Queued
            | DownloadState::Connecting
            | DownloadState::Downloading
            | DownloadState::RetryWaiting
    )
}

const fn is_policy_paused(state: DownloadState, pause_owner: Option<PauseOwner>) -> bool {
    matches!(state, DownloadState::Paused) && matches!(pause_owner, Some(PauseOwner::SystemPolicy))
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU64};

    use super::*;

    struct MockWindowsState {
        state: Result<WindowsSystemState, SystemPolicyError>,
        reads: usize,
    }

    impl MockWindowsState {
        fn available(network_cost: NetworkCost, power_source: PowerSource) -> Self {
            Self {
                state: Ok(WindowsSystemState {
                    network_cost,
                    power_source,
                }),
                reads: 0,
            }
        }
    }

    impl WindowsSystemStateAdapter for MockWindowsState {
        fn snapshot(&mut self) -> Result<WindowsSystemState, SystemPolicyError> {
            self.reads += 1;
            self.state.clone()
        }
    }

    #[test]
    fn metered_pause_recovers_only_jobs_owned_by_system_policy() {
        let job_id = JobId::new();
        let config = SystemPolicyConfig {
            metered: RestrictedNetworkPolicy::Pause,
            ..SystemPolicyConfig::default()
        };
        let mut adapter = MockWindowsState::available(NetworkCost::Metered, PowerSource::Ac);
        let running = job(job_id, DownloadState::Downloading, None, Some(2_000));

        assert_eq!(
            evaluate_system_policy(&mut adapter, config, &[running]).expect("metered decision"),
            [JobPolicyDecision {
                job_id,
                control: PolicyControl::Pause {
                    reason: SystemPolicyReason::Metered,
                },
                limits: None,
            }]
        );

        adapter.state = Ok(WindowsSystemState {
            network_cost: NetworkCost::Unrestricted,
            power_source: PowerSource::Ac,
        });
        let system_paused = job(
            job_id,
            DownloadState::Paused,
            Some(PauseOwner::SystemPolicy),
            Some(2_000),
        );
        let user_job_id = JobId::new();
        let user_paused = job(
            user_job_id,
            DownloadState::Paused,
            Some(PauseOwner::User),
            Some(2_000),
        );

        assert_eq!(
            evaluate_system_policy(&mut adapter, config, &[system_paused, user_paused])
                .expect("recovery decisions"),
            [
                JobPolicyDecision {
                    job_id,
                    control: PolicyControl::Resume,
                    limits: None,
                },
                JobPolicyDecision {
                    job_id: user_job_id,
                    control: PolicyControl::NoChange,
                    limits: None,
                },
            ]
        );
        assert_eq!(adapter.reads, 2);
    }

    #[test]
    fn default_policy_asks_on_metered_and_pauses_on_roaming() {
        let job_id = JobId::new();
        let running = job(job_id, DownloadState::Queued, None, None);
        let mut adapter = MockWindowsState::available(NetworkCost::Metered, PowerSource::Ac);

        let metered =
            evaluate_system_policy(&mut adapter, SystemPolicyConfig::default(), &[running])
                .expect("metered");
        assert_eq!(
            metered[0].control,
            PolicyControl::Ask {
                reason: SystemPolicyReason::Metered
            }
        );

        adapter.state = Ok(WindowsSystemState {
            network_cost: NetworkCost::Roaming,
            power_source: PowerSource::Ac,
        });
        let roaming =
            evaluate_system_policy(&mut adapter, SystemPolicyConfig::default(), &[running])
                .expect("roaming");
        assert_eq!(
            roaming[0].control,
            PolicyControl::Pause {
                reason: SystemPolicyReason::Roaming
            }
        );
    }

    #[test]
    fn battery_policy_reduces_resources_or_pauses_only_known_large_jobs() {
        let small_id = JobId::new();
        let large_id = JobId::new();
        let unknown_id = JobId::new();
        let jobs = [
            job(small_id, DownloadState::Downloading, None, Some(999)),
            job(large_id, DownloadState::Downloading, None, Some(1_000)),
            job(unknown_id, DownloadState::Downloading, None, None),
        ];
        let mut adapter =
            MockWindowsState::available(NetworkCost::Unrestricted, PowerSource::Battery);
        let reduced = evaluate_system_policy(
            &mut adapter,
            SystemPolicyConfig {
                battery: BatteryPolicy::Reduce {
                    max_bytes_per_second: nonzero_u64(512),
                    max_connections: nonzero_u16(2),
                },
                ..SystemPolicyConfig::default()
            },
            &jobs,
        )
        .expect("reduced policy");
        assert!(reduced.iter().all(|decision| {
            decision.control == PolicyControl::NoChange
                && decision.limits
                    == Some(PolicyTransferLimits {
                        max_bytes_per_second: nonzero_u64(512),
                        max_connections: nonzero_u16(2),
                    })
        }));

        let paused = evaluate_system_policy(
            &mut adapter,
            SystemPolicyConfig {
                battery: BatteryPolicy::PauseLarge {
                    minimum_total_bytes: nonzero_u64(1_000),
                },
                ..SystemPolicyConfig::default()
            },
            &jobs,
        )
        .expect("large policy");
        assert_eq!(paused[0].control, PolicyControl::NoChange);
        assert_eq!(
            paused[1].control,
            PolicyControl::Pause {
                reason: SystemPolicyReason::Battery
            }
        );
        assert_eq!(paused[2].control, PolicyControl::NoChange);
    }

    #[test]
    fn unavailable_state_fails_closed_without_policy_decisions() {
        let mut adapter = MockWindowsState {
            state: Err(SystemPolicyError::StateUnavailable(
                "mock failure".to_owned(),
            )),
            reads: 0,
        };

        assert_eq!(
            evaluate_system_policy(
                &mut adapter,
                SystemPolicyConfig::default(),
                &[job(
                    JobId::new(),
                    DownloadState::Paused,
                    Some(PauseOwner::SystemPolicy),
                    None
                )],
            ),
            Err(SystemPolicyError::StateUnavailable(
                "mock failure".to_owned()
            ))
        );
        assert_eq!(adapter.reads, 1);
    }

    const fn job(
        job_id: JobId,
        state: DownloadState,
        pause_owner: Option<PauseOwner>,
        total_bytes: Option<u64>,
    ) -> PolicyJobSnapshot {
        PolicyJobSnapshot {
            job_id,
            state,
            total_bytes,
            pause_owner,
        }
    }

    fn nonzero_u64(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("non-zero fixture")
    }

    fn nonzero_u16(value: u16) -> NonZeroU16 {
        NonZeroU16::new(value).expect("non-zero fixture")
    }
}
