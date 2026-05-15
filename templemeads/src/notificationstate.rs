// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::notification::Notification;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

static STATE: Lazy<Arc<RwLock<HashMap<Uuid, Notification>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

/// Store a notification so it can be fetched by the web portal via POST /fetch_notification.
pub async fn add(notification: &Notification) {
    STATE
        .write()
        .await
        .insert(notification.id(), notification.clone());
}

/// Retrieve a pending notification by UUID. Returns None if it has already been
/// removed or was never stored.
pub async fn get(id: Uuid) -> Option<Notification> {
    STATE.read().await.get(&id).cloned()
}

/// Remove a notification from the pending store. Called after the web portal has
/// successfully fetched it, or after all signal attempts have been exhausted.
pub async fn remove(id: Uuid) {
    STATE.write().await.remove(&id);
}
