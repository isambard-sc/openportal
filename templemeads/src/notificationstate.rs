// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::diagnostics;
use crate::domain::Domain;
use crate::domain_static;
use crate::error::Error;
use crate::notification::Notification;

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use tokio::sync::{Mutex, Notify, RwLock};
use uuid::Uuid;

/// Maximum number of notifications held in the delivery queue. When the queue
/// is full all existing entries are cleared (they are stale during an outage)
/// and the incoming notification takes the first slot.
const MAX_PENDING_QUEUE: usize = 500;

struct NotificationState<L: Domain> {
    /// Delivery queue: notifications waiting to be signalled to the web portal.
    queue: Mutex<VecDeque<Notification<L>>>,
    /// Wakes the single consumer task when a new notification is enqueued.
    notify: Notify,
    /// Pending-fetch map: notifications that have been signalled but not yet
    /// fetched by the web portal via POST /fetch_notification.
    pending: RwLock<HashMap<Uuid, Notification<L>>>,
}

impl<L: Domain> NotificationState<L> {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: Notify::const_new(),
            pending: RwLock::new(HashMap::new()),
        }
    }
}

static STATE: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

fn state<L: Domain>() -> Result<&'static NotificationState<L>, Error> {
    domain_static::get_or_init(&STATE, NotificationState::<L>::new)
}

// ---------------------------------------------------------------------------
// Delivery queue
// ---------------------------------------------------------------------------

/// Push a notification onto the delivery queue. If the queue is already full
/// all stale entries are dropped and the failed counter is bumped in bulk before
/// the new notification is pushed.
pub async fn enqueue<L: Domain>(notification: Notification<L>) {
    let state = match state::<L>() {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Error getting notification state: {}", e);
            return;
        }
    };

    let dropped = {
        let mut q = state.queue.lock().await;
        let n = if q.len() >= MAX_PENDING_QUEUE {
            let n = q.len();
            q.clear();
            n
        } else {
            0
        };
        q.push_back(notification);
        n
    };
    state.notify.notify_one();
    if dropped > 0 {
        diagnostics::add_notifications_failed(dropped).await;
        tracing::warn!(
            "Notification delivery queue was full; cleared {} stale notification(s) to make room",
            dropped
        );
    }
}

/// Pop the next notification from the delivery queue, waiting until one is
/// available. This is called exclusively by the single background consumer task.
pub async fn pop_queued<L: Domain>() -> Result<Notification<L>, Error> {
    let state = state::<L>()?;
    loop {
        if let Some(n) = state.queue.lock().await.pop_front() {
            return Ok(n);
        }
        state.notify.notified().await;
    }
}

// ---------------------------------------------------------------------------
// Pending-fetch map
// ---------------------------------------------------------------------------

/// Store a notification so it can be fetched by the web portal via POST /fetch_notification.
pub async fn add<L: Domain>(notification: &Notification<L>) {
    let state = match state::<L>() {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Error getting notification state: {}", e);
            return;
        }
    };

    state
        .pending
        .write()
        .await
        .insert(notification.id(), notification.clone());
}

/// Retrieve a pending notification by UUID. Returns None if it has already been
/// removed or was never stored.
pub async fn get<L: Domain>(id: Uuid) -> Result<Option<Notification<L>>, Error> {
    Ok(state::<L>()?.pending.read().await.get(&id).cloned())
}

/// Remove a notification from the pending store. Called after the web portal
/// has successfully fetched it, or after all signal attempts are exhausted.
pub async fn remove<L: Domain>(id: Uuid) {
    let state = match state::<L>() {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Error getting notification state: {}", e);
            return;
        }
    };

    state.pending.write().await.remove(&id);
}
