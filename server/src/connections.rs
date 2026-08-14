use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub type ConnectionId = u64;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_connection_id() -> ConnectionId {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct ConnectionEntry {
    id: ConnectionId,
    tx: mpsc::Sender<String>,
}

/// In-memory registry of live device connections, keyed by `device_id`.
/// Each entry holds a channel sender for outbound WebSocket text frames.
/// Entries are connection-scoped: dropped on disconnect. Pairings and
/// device registrations persist in SQLite and are unaffected by churn.
#[derive(Clone)]
pub struct Connections {
    map: Arc<Mutex<HashMap<String, ConnectionEntry>>>,
}

impl Connections {
    pub fn new() -> Self {
        Self {
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn insert(&self, device_id: &str, connection_id: ConnectionId, tx: mpsc::Sender<String>) {
        self.map
            .lock()
            .expect("connections mutex poisoned")
            .insert(
                device_id.to_string(),
                ConnectionEntry {
                    id: connection_id,
                    tx,
                },
            );
    }

    /// Remove an entry only if it still belongs to `connection_id`. A quick
    /// reconnect of the same device replaces the entry; the old session then
    /// must not remove the new connection's sender.
    pub fn remove_if(&self, device_id: &str, connection_id: ConnectionId) {
        let mut map = self.map.lock().expect("connections mutex poisoned");
        if map
            .get(device_id)
            .is_some_and(|entry| entry.id == connection_id)
        {
            map.remove(device_id);
        }
    }

    pub fn get(&self, device_id: &str) -> Option<mpsc::Sender<String>> {
        self.map
            .lock()
            .expect("connections mutex poisoned")
            .get(device_id)
            .map(|entry| entry.tx.clone())
    }

    /// Remove unconditionally (used by tests and teardown paths).
    pub fn remove(&self, device_id: &str) {
        self.map
            .lock()
            .expect("connections mutex poisoned")
            .remove(device_id);
    }
}

impl Default for Connections {
    fn default() -> Self {
        Self {
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
