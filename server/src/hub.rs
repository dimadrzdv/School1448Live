//! Live runtime state shared with the HTTP layer and pushed to browsers over SSE.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::broadcast;

#[derive(Debug, Default, Clone, Copy)]
pub struct Runtime {
    /// The phone's media connection is up.
    pub publisher: bool,
    pub viewers: u32,
}

pub struct Hub {
    tx: broadcast::Sender<Arc<str>>,
    runtime: Mutex<HashMap<String, Runtime>>,
}

impl Default for Hub {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(256);
        Hub { tx, runtime: Mutex::new(HashMap::new()) }
    }
}

impl Hub {
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.tx.subscribe()
    }

    pub fn runtime(&self, stream_id: &str) -> Option<Runtime> {
        self.runtime.lock().unwrap().get(stream_id).copied()
    }

    pub fn set_publisher(&self, stream_id: &str, connected: bool) {
        self.runtime.lock().unwrap().entry(stream_id.to_string()).or_default().publisher = connected;
    }

    pub fn remove(&self, stream_id: &str) {
        self.runtime.lock().unwrap().remove(stream_id);
    }

    pub fn set_counts(&self, counts: &HashMap<String, u32>) {
        let mut rt = self.runtime.lock().unwrap();
        for (id, r) in rt.iter_mut() {
            r.viewers = counts.get(id).copied().unwrap_or(0);
        }
        drop(rt);
        self.send(json!({ "type": "viewers", "counts": counts }));
    }

    /// Tells browsers to refetch the stream list.
    pub fn streams_changed(&self) {
        self.send(json!({ "type": "streams" }));
    }

    pub fn archive_changed(&self) {
        self.send(json!({ "type": "archive" }));
    }

    fn send(&self, value: serde_json::Value) {
        let _ = self.tx.send(Arc::from(value.to_string()));
    }
}
