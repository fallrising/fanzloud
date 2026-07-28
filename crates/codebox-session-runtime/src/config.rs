use std::time::Duration;

use serde::Serialize;

use crate::P0SessionError;

pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(60);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_HISTORY_CAPACITY: usize = 256;
const MIN_HISTORY_CAPACITY: usize = 64;
const MAX_HISTORY_CAPACITY: usize = 1024;
const DEFAULT_MAX_SUBSCRIBERS: usize = 8;
const MIN_MAX_SUBSCRIBERS: usize = 1;
const MAX_MAX_SUBSCRIBERS: usize = 32;
const DEFAULT_LIVE_CAPACITY: usize = 32;
const MIN_LIVE_CAPACITY: usize = 8;
const MAX_LIVE_CAPACITY: usize = 256;

/// Administrator-owned bounds for one process-lifetime P0 session.
///
/// Contract: `CU-SES-P0-01`. This value contains no provider, executable, repository, path, or
/// browser-controlled configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct P0SessionConfig {
    pub(crate) poll_interval: Duration,
    pub(crate) history_capacity: usize,
    pub(crate) max_subscribers: usize,
    pub(crate) live_capacity: usize,
}

/// Safe administrator configuration field associated with a validation failure.
///
/// Contract: `CU-SES-P0-01`. Values never contain the rejected duration or capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum P0SessionConfigField {
    PollInterval,
    HistoryCapacity,
    MaxSubscribers,
    LiveCapacity,
}

impl P0SessionConfig {
    /// Validates the polling and in-memory fanout bounds.
    ///
    /// Contract: `CU-SES-P0-01`. Failure has no side effect.
    pub fn try_new(
        poll_interval: Duration,
        history_capacity: usize,
        max_subscribers: usize,
        live_capacity: usize,
    ) -> Result<Self, P0SessionError> {
        if !(MIN_POLL_INTERVAL..=MAX_POLL_INTERVAL).contains(&poll_interval) {
            return Err(P0SessionError::invalid_config(
                P0SessionConfigField::PollInterval,
            ));
        }
        if !(MIN_HISTORY_CAPACITY..=MAX_HISTORY_CAPACITY).contains(&history_capacity) {
            return Err(P0SessionError::invalid_config(
                P0SessionConfigField::HistoryCapacity,
            ));
        }
        if !(MIN_MAX_SUBSCRIBERS..=MAX_MAX_SUBSCRIBERS).contains(&max_subscribers) {
            return Err(P0SessionError::invalid_config(
                P0SessionConfigField::MaxSubscribers,
            ));
        }
        if !(MIN_LIVE_CAPACITY..=MAX_LIVE_CAPACITY).contains(&live_capacity) {
            return Err(P0SessionError::invalid_config(
                P0SessionConfigField::LiveCapacity,
            ));
        }

        Ok(Self {
            poll_interval,
            history_capacity,
            max_subscribers,
            live_capacity,
        })
    }
}

impl Default for P0SessionConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            history_capacity: DEFAULT_HISTORY_CAPACITY,
            max_subscribers: DEFAULT_MAX_SUBSCRIBERS,
            live_capacity: DEFAULT_LIVE_CAPACITY,
        }
    }
}
