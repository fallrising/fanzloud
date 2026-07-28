use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use codebox_domain::CommandId;
use tokio::sync::Notify;

use crate::error::ApiError;
use crate::transport::CachedResponse;
use crate::types::RequestIdentity;

const MAX_ENTRIES: usize = 128;
const MAX_STORAGE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct Idempotency {
    registry: Mutex<Registry>,
}

impl Idempotency {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            registry: Mutex::new(Registry::default()),
        })
    }

    pub(crate) fn admit(
        self: &Arc<Self>,
        key: CommandId,
        identity: RequestIdentity,
        now: Duration,
    ) -> Result<IdempotencyAdmission, ApiError> {
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| ApiError::service_unavailable())?;
        registry.prune_expired(now);

        if let Some(entry) = registry.entries.get_mut(&key) {
            if entry.identity != identity {
                return Err(ApiError::new(
                    axum::http::StatusCode::CONFLICT,
                    "idempotency_conflict",
                    "idempotency key was used for another request",
                ));
            }
            entry.waiters = entry.waiters.saturating_add(1);
            return Ok(IdempotencyAdmission {
                owner: false,
                waiter: IdempotencyWaiter {
                    idempotency: Arc::clone(self),
                    key,
                    notify: Arc::clone(&entry.notify),
                    active: true,
                },
            });
        }

        let request_bytes = identity.storage_bytes();
        registry.make_capacity(request_bytes)?;
        let notify = Arc::new(Notify::new());
        registry.total_bytes = registry.total_bytes.saturating_add(request_bytes);
        registry.entries.insert(
            key,
            Entry {
                identity,
                request_bytes,
                response_bytes: 0,
                state: EntryState::InFlight,
                notify: Arc::clone(&notify),
                waiters: 1,
                completed_seq: None,
            },
        );
        Ok(IdempotencyAdmission {
            owner: true,
            waiter: IdempotencyWaiter {
                idempotency: Arc::clone(self),
                key,
                notify,
                active: true,
            },
        })
    }

    pub(crate) fn complete(
        &self,
        key: CommandId,
        response: CachedResponse,
        disposition: CacheDisposition,
    ) {
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        registry.completion_seq = registry.completion_seq.saturating_add(1);
        let completion_seq = registry.completion_seq;
        let response_bytes = response.storage_bytes();
        let mut completed = false;
        if let Some(entry) = registry.entries.get_mut(&key)
            && matches!(entry.state, EntryState::InFlight)
        {
            entry.response_bytes = response_bytes;
            entry.completed_seq = Some(completion_seq);
            entry.state = EntryState::Complete {
                response,
                disposition,
            };
            entry.notify.notify_waiters();
            completed = true;
        }
        if completed {
            registry.total_bytes = registry.total_bytes.saturating_add(response_bytes);
        }
        registry.evict_completed_to_storage_bound(Some(key));
    }

    pub(crate) fn release_auth_session(&self, session_seq: u64) {
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        let keys: Vec<_> = registry
            .entries
            .iter()
            .filter_map(|(key, entry)| match entry.state {
                EntryState::Complete {
                    disposition: CacheDisposition::RemoveAfterAuthSession(entry_session_seq),
                    ..
                } if entry_session_seq == session_seq && entry.waiters == 0 => Some(*key),
                _ => None,
            })
            .collect();
        for key in keys {
            registry.remove(key);
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut registry) = self.registry.lock() {
            *registry = Registry::default();
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.registry
            .lock()
            .map(|registry| registry.entries.len())
            .unwrap_or_default()
    }
}

pub(crate) struct IdempotencyAdmission {
    pub(crate) owner: bool,
    pub(crate) waiter: IdempotencyWaiter,
}

pub(crate) struct IdempotencyWaiter {
    idempotency: Arc<Idempotency>,
    key: CommandId,
    notify: Arc<Notify>,
    active: bool,
}

impl IdempotencyWaiter {
    pub(crate) async fn response(mut self) -> CachedResponse {
        loop {
            let notify = Arc::clone(&self.notify);
            let notified = notify.notified();
            match self.take_response() {
                WaitState::Ready(response) => {
                    self.active = false;
                    return response;
                }
                WaitState::Missing => {
                    self.active = false;
                    return CachedResponse::error(ApiError::service_unavailable());
                }
                WaitState::Pending => notified.await,
            }
        }
    }

    fn take_response(&mut self) -> WaitState {
        let Ok(mut registry) = self.idempotency.registry.lock() else {
            return WaitState::Missing;
        };
        let Some(entry) = registry.entries.get_mut(&self.key) else {
            return WaitState::Missing;
        };
        let response = match &entry.state {
            EntryState::InFlight => return WaitState::Pending,
            EntryState::Complete { response, .. } => response.clone(),
        };
        entry.waiters = entry.waiters.saturating_sub(1);
        WaitState::Ready(response)
    }
}

impl Drop for IdempotencyWaiter {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Ok(mut registry) = self.idempotency.registry.lock() else {
            return;
        };
        let Some(entry) = registry.entries.get_mut(&self.key) else {
            return;
        };
        entry.waiters = entry.waiters.saturating_sub(1);
    }
}

enum WaitState {
    Pending,
    Ready(CachedResponse),
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CacheDisposition {
    Retain,
    ExpireAt(Duration),
    RemoveAfterAuthSession(u64),
}

#[derive(Default)]
struct Registry {
    entries: HashMap<CommandId, Entry>,
    total_bytes: usize,
    completion_seq: u64,
}

impl Registry {
    fn prune_expired(&mut self, now: Duration) {
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(key, entry)| match entry.state {
                EntryState::Complete {
                    disposition: CacheDisposition::ExpireAt(deadline),
                    ..
                } if deadline <= now && entry.waiters == 0 => Some(*key),
                _ => None,
            })
            .collect();
        for key in keys {
            self.remove(key);
        }
    }

    fn make_capacity(&mut self, request_bytes: usize) -> Result<(), ApiError> {
        while self.entries.len() >= MAX_ENTRIES
            || self.total_bytes.saturating_add(request_bytes) > MAX_STORAGE_BYTES
        {
            let Some(oldest) = self.oldest_completed(None) else {
                return Err(ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "idempotency_unavailable",
                    "idempotency registry is temporarily unavailable",
                ));
            };
            self.remove(oldest);
        }
        Ok(())
    }

    fn evict_completed_to_storage_bound(&mut self, protected: Option<CommandId>) {
        while self.total_bytes > MAX_STORAGE_BYTES {
            let Some(oldest) = self.oldest_completed(protected) else {
                break;
            };
            self.remove(oldest);
        }
    }

    fn oldest_completed(&self, protected: Option<CommandId>) -> Option<CommandId> {
        self.entries
            .iter()
            .filter(|(key, entry)| {
                Some(**key) != protected && entry.completed_seq.is_some() && entry.waiters == 0
            })
            .min_by_key(|(_, entry)| entry.completed_seq)
            .map(|(key, _)| *key)
    }

    fn remove(&mut self, key: CommandId) {
        if let Some(entry) = self.entries.remove(&key) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(entry.request_bytes)
                .saturating_sub(entry.response_bytes);
        }
    }
}

struct Entry {
    identity: RequestIdentity,
    request_bytes: usize,
    response_bytes: usize,
    state: EntryState,
    notify: Arc<Notify>,
    waiters: usize,
    completed_seq: Option<u64>,
}

enum EntryState {
    InFlight,
    Complete {
        response: CachedResponse,
        disposition: CacheDisposition,
    },
}
