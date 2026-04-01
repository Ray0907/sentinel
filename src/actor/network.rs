//! Network request tracker with long-lived connection classification.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TrackedRequest {
    pub url: String,
    pub method: String,
    pub status: Option<i32>,
    pub completed: bool,
    pub failed: bool,
    pub error: Option<String>,
}

pub struct NetworkTracker {
    requests: HashMap<String, TrackedRequest>,
    websockets: HashMap<String, String>, // request_id → url
}

impl Default for NetworkTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkTracker {
    pub fn new() -> Self {
        Self {
            requests: HashMap::new(),
            websockets: HashMap::new(),
        }
    }

    pub fn on_request_sent(&mut self, request_id: &str, url: &str, method: &str) {
        self.requests.insert(
            request_id.to_string(),
            TrackedRequest {
                url: url.to_string(),
                method: method.to_string(),
                status: None,
                completed: false,
                failed: false,
                error: None,
            },
        );
    }

    pub fn on_response(&mut self, request_id: &str, status: i32) {
        if let Some(req) = self.requests.get_mut(request_id) {
            req.status = Some(status);
        }
    }

    pub fn on_complete(&mut self, request_id: &str) {
        // C3 fix: remove completed requests to prevent unbounded growth
        self.requests.remove(request_id);
    }

    pub fn on_failed(&mut self, request_id: &str, _error: &str) {
        // C3 fix: remove failed requests to prevent unbounded growth
        self.requests.remove(request_id);
    }

    pub fn on_websocket_opened(&mut self, request_id: &str, url: &str) {
        self.websockets
            .insert(request_id.to_string(), url.to_string());
    }

    pub fn on_websocket_closed(&mut self, request_id: &str) {
        self.websockets.remove(request_id);
    }

    /// Count of pending (not completed) requests, excluding WebSockets.
    pub fn pending_count(&self) -> usize {
        self.requests
            .iter()
            .filter(|(id, req)| !req.completed && !self.websockets.contains_key(*id))
            .count()
    }

    /// Get the URL for a request ID.
    pub fn get_url(&self, request_id: &str) -> Option<String> {
        self.requests.get(request_id).map(|r| r.url.clone())
    }
}
