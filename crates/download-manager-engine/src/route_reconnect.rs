//! Generation-gated reconnect planning for network and VPN route changes.

use std::collections::HashSet;

use thiserror::Error;

use crate::segmented::ByteRange;

const MAX_ROUTE_KEY_LENGTH: usize = 512;

/// One connection's last durable range checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionCheckpoint {
    pub connection_id: u32,
    pub range: ByteRange,
    /// First byte that is not yet durable. `range.end + 1` means complete.
    pub durable_offset: u64,
}

/// Work required after invalidating a connection from the previous route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectAction {
    pub connection_id: u32,
    /// Missing suffix to request on the new route. `None` means the range was already durable.
    pub resume_range: Option<ByteRange>,
}

/// Atomic reconnect plan produced for a changed route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteReconnectPlan {
    pub previous_route: String,
    pub current_route: String,
    /// Generation that every replacement connection and subsequent worker event must carry.
    pub generation: u64,
    pub actions: Vec<ReconnectAction>,
}

/// Result of observing a network route snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteChangeOutcome {
    Unchanged { generation: u64 },
    Reconnect(RouteReconnectPlan),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouteReconnectError {
    #[error("route key must be trimmed, non-empty, bounded, and free of control characters")]
    InvalidRouteKey,
    #[error("connection checkpoint IDs must be unique")]
    DuplicateConnection,
    #[error("connection {connection_id} has an invalid byte range")]
    InvalidRange { connection_id: u32 },
    #[error("connection {connection_id} durable offset is outside its byte range")]
    InvalidDurableOffset { connection_id: u32 },
    #[error("route generation is exhausted")]
    GenerationExhausted,
}

/// Converts route-change notifications into safe, generation-gated reconnect plans.
///
/// Callers close every old connection in a returned plan, then recreate it on the new route and
/// request only `resume_range`. Bytes received after the supplied durable offsets are deliberately
/// overwritten. Worker progress/completion must be ignored unless [`Self::accepts_generation`]
/// returns true, preventing an old socket from committing after a route transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteReconnectCoordinator {
    route_key: String,
    generation: u64,
}

impl RouteReconnectCoordinator {
    /// Starts route monitoring from an operating-system-provided route identity.
    ///
    /// # Errors
    /// Rejects empty, untrimmed, oversized, or control-containing route keys.
    pub fn new(route_key: &str) -> Result<Self, RouteReconnectError> {
        validate_route_key(route_key)?;
        Ok(Self {
            route_key: route_key.to_owned(),
            generation: 0,
        })
    }

    #[must_use]
    pub fn route_key(&self) -> &str {
        &self.route_key
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Reports whether a worker event belongs to the currently active connection generation.
    #[must_use]
    pub const fn accepts_generation(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// Reconciles a route snapshot against the current active route.
    ///
    /// The checkpoint set is validated before state changes, so an invalid snapshot cannot
    /// partially advance the route or generation. Repeated notifications for the same route are
    /// idempotent and do not reset healthy connections.
    ///
    /// # Errors
    /// Rejects invalid route keys, duplicate connections, malformed ranges/offsets, or generation
    /// overflow.
    pub fn observe_route(
        &mut self,
        route_key: &str,
        checkpoints: &[ConnectionCheckpoint],
    ) -> Result<RouteChangeOutcome, RouteReconnectError> {
        validate_route_key(route_key)?;
        if route_key == self.route_key {
            return Ok(RouteChangeOutcome::Unchanged {
                generation: self.generation,
            });
        }

        let actions = reconnect_actions(checkpoints)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(RouteReconnectError::GenerationExhausted)?;
        let previous_route = std::mem::replace(&mut self.route_key, route_key.to_owned());
        self.generation = generation;
        Ok(RouteChangeOutcome::Reconnect(RouteReconnectPlan {
            previous_route,
            current_route: route_key.to_owned(),
            generation,
            actions,
        }))
    }
}

fn validate_route_key(route_key: &str) -> Result<(), RouteReconnectError> {
    if route_key.is_empty()
        || route_key.trim() != route_key
        || route_key.chars().count() > MAX_ROUTE_KEY_LENGTH
        || route_key.chars().any(char::is_control)
    {
        Err(RouteReconnectError::InvalidRouteKey)
    } else {
        Ok(())
    }
}

fn reconnect_actions(
    checkpoints: &[ConnectionCheckpoint],
) -> Result<Vec<ReconnectAction>, RouteReconnectError> {
    let mut connection_ids = HashSet::with_capacity(checkpoints.len());
    let mut actions = Vec::with_capacity(checkpoints.len());
    for checkpoint in checkpoints {
        if !connection_ids.insert(checkpoint.connection_id) {
            return Err(RouteReconnectError::DuplicateConnection);
        }
        if checkpoint.range.is_empty() || checkpoint.range.end == u64::MAX {
            return Err(RouteReconnectError::InvalidRange {
                connection_id: checkpoint.connection_id,
            });
        }
        if checkpoint.durable_offset < checkpoint.range.start
            || checkpoint.durable_offset > checkpoint.range.end + 1
        {
            return Err(RouteReconnectError::InvalidDurableOffset {
                connection_id: checkpoint.connection_id,
            });
        }
        actions.push(ReconnectAction {
            connection_id: checkpoint.connection_id,
            resume_range: (checkpoint.durable_offset <= checkpoint.range.end).then_some(
                ByteRange {
                    start: checkpoint.durable_offset,
                    end: checkpoint.range.end,
                },
            ),
        });
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionCheckpoint, ReconnectAction, RouteChangeOutcome, RouteReconnectCoordinator,
        RouteReconnectError,
    };
    use crate::segmented::ByteRange;

    #[test]
    fn route_change_resets_connections_and_resumes_only_durable_suffixes() {
        let mut coordinator =
            RouteReconnectCoordinator::new("ethernet:10.0.0.1").expect("initial route");
        let checkpoints = [checkpoint(7, 0, 99, 64), checkpoint(8, 100, 199, 200)];

        let RouteChangeOutcome::Reconnect(plan) = coordinator
            .observe_route("vpn:10.8.0.1", &checkpoints)
            .expect("VPN transition")
        else {
            panic!("changed route must reconnect");
        };

        assert_eq!(plan.previous_route, "ethernet:10.0.0.1");
        assert_eq!(plan.current_route, "vpn:10.8.0.1");
        assert_eq!(plan.generation, 1);
        assert_eq!(
            plan.actions,
            [
                ReconnectAction {
                    connection_id: 7,
                    resume_range: Some(ByteRange { start: 64, end: 99 }),
                },
                ReconnectAction {
                    connection_id: 8,
                    resume_range: None,
                },
            ]
        );
        assert!(!coordinator.accepts_generation(0));
        assert!(coordinator.accepts_generation(1));

        assert_eq!(
            coordinator.observe_route("vpn:10.8.0.1", &checkpoints),
            Ok(RouteChangeOutcome::Unchanged { generation: 1 })
        );
        assert_eq!(coordinator.generation(), 1);
    }

    #[test]
    fn rapid_vpn_exit_invalidates_the_previous_reconnect_generation() {
        let mut coordinator =
            RouteReconnectCoordinator::new("ethernet:10.0.0.1").expect("initial route");
        let first = [checkpoint(1, 0, 999, 400)];
        coordinator
            .observe_route("vpn:10.8.0.1", &first)
            .expect("VPN entry");

        let second = [checkpoint(1, 0, 999, 640)];
        let RouteChangeOutcome::Reconnect(plan) = coordinator
            .observe_route("ethernet:10.0.0.1", &second)
            .expect("VPN exit")
        else {
            panic!("exiting VPN must reconnect");
        };
        assert_eq!(plan.generation, 2);
        assert_eq!(
            plan.actions[0].resume_range,
            Some(ByteRange {
                start: 640,
                end: 999,
            })
        );
        assert!(!coordinator.accepts_generation(1));
        assert!(coordinator.accepts_generation(2));
    }

    #[test]
    fn invalid_checkpoint_refuses_route_change_atomically() {
        let mut coordinator =
            RouteReconnectCoordinator::new("ethernet:10.0.0.1").expect("initial route");
        let invalid = [checkpoint(3, 100, 199, 99)];

        assert_eq!(
            coordinator.observe_route("vpn:10.8.0.1", &invalid),
            Err(RouteReconnectError::InvalidDurableOffset { connection_id: 3 })
        );
        assert_eq!(coordinator.route_key(), "ethernet:10.0.0.1");
        assert_eq!(coordinator.generation(), 0);

        let duplicates = [checkpoint(4, 0, 9, 5), checkpoint(4, 10, 19, 15)];
        assert_eq!(
            coordinator.observe_route("vpn:10.8.0.1", &duplicates),
            Err(RouteReconnectError::DuplicateConnection)
        );
        assert_eq!(coordinator.route_key(), "ethernet:10.0.0.1");
    }

    #[test]
    fn malformed_route_keys_are_rejected() {
        assert_eq!(
            RouteReconnectCoordinator::new(" vpn:route"),
            Err(RouteReconnectError::InvalidRouteKey)
        );
        let mut coordinator =
            RouteReconnectCoordinator::new("ethernet:route").expect("initial route");
        assert_eq!(
            coordinator.observe_route("vpn:\nroute", &[]),
            Err(RouteReconnectError::InvalidRouteKey)
        );
    }

    const fn checkpoint(
        connection_id: u32,
        start: u64,
        end: u64,
        durable_offset: u64,
    ) -> ConnectionCheckpoint {
        ConnectionCheckpoint {
            connection_id,
            range: ByteRange { start, end },
            durable_offset,
        }
    }
}
