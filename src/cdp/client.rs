//! CDP WebSocket client with async event multiplexing.
//!
//! Design: single WebSocket connection, sequential command IDs,
//! oneshot channels for responses, broadcast channel for events.
//! On disconnect, all pending calls are failed immediately (C4, C5 fixes).

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use super::types::{CdpCommand, CdpEvent, CdpResponse};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// CDP client that manages a WebSocket connection to Chrome.
#[derive(Clone)]
pub struct CdpClient {
    inner: Arc<CdpClientInner>,
}

struct CdpClientInner {
    /// Send commands through this channel to the writer task
    write_tx: mpsc::Sender<String>,
    /// Pending response channels, keyed by command ID
    pending: Mutex<HashMap<u64, oneshot::Sender<CdpResponse>>>,
    /// Broadcast channel for CDP events
    event_tx: broadcast::Sender<CdpEvent>,
    /// Sequential command ID counter
    next_id: AtomicU64,
    /// Whether the connection is closed (fail-fast flag)
    closed: AtomicBool,
}

impl CdpClient {
    /// Connect to a Chrome DevTools Protocol WebSocket endpoint.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (ws, _) = connect_async(ws_url).await?;
        let (write, read) = ws.split();

        let (write_tx, write_rx) = mpsc::channel::<String>(256);
        let (event_tx, _) = broadcast::channel::<CdpEvent>(4096);

        let inner = Arc::new(CdpClientInner {
            write_tx,
            pending: Mutex::new(HashMap::new()),
            event_tx: event_tx.clone(),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });

        // Writer task: receives serialized messages and sends them over WebSocket
        let writer_handle = {
            let mut write = write;
            let mut write_rx = write_rx;
            tokio::spawn(async move {
                while let Some(msg) = write_rx.recv().await {
                    if let Err(e) = write.send(Message::Text(msg)).await {
                        tracing::error!("WebSocket write error: {e}");
                        break;
                    }
                }
            })
        };

        // Reader task: reads WebSocket messages, routes responses and events
        let reader_inner = inner.clone();
        let _reader_handle = tokio::spawn(async move {
            let mut read = read;
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        let text_str: &str = &text;
                        if let Ok(response) = serde_json::from_str::<CdpResponse>(text_str) {
                            if let Some(method) = &response.method {
                                // This is an event
                                let event = CdpEvent {
                                    method: method.clone(),
                                    params: response.params.clone().unwrap_or(json!({})),
                                    session_id: response.session_id.clone(),
                                };
                                let _ = reader_inner.event_tx.send(event);
                            } else if let Some(id) = response.id {
                                // This is a command response
                                let mut pending = reader_inner.pending.lock().await;
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(response);
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        tracing::info!("WebSocket closed by server");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("WebSocket read error: {e}");
                        break;
                    }
                    _ => {} // Ignore ping/pong/binary
                }
            }

            // Connection closed: mark as closed and drain all pending waiters (C4, C5 fix)
            reader_inner.closed.store(true, Ordering::SeqCst);
            let mut pending = reader_inner.pending.lock().await;
            let count = pending.len();
            if count > 0 {
                tracing::warn!(count, "Draining pending CDP calls on disconnect");
            }
            pending.clear(); // Dropping senders will cause receivers to get RecvError
            drop(pending);

            // Abort the writer task (C4 fix: don't just drop the handle)
            writer_handle.abort();
        });

        Ok(Self { inner })
    }

    /// Send a CDP command and wait for the response.
    pub async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.call_with_session(method, params, None).await
    }

    /// Send a CDP command to a specific session and wait for the response.
    pub async fn call_with_session(
        &self,
        method: &str,
        params: serde_json::Value,
        session_id: Option<String>,
    ) -> Result<serde_json::Value> {
        // Fail-fast if connection is closed
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(anyhow!("CDP connection closed"));
        }

        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed); // M7 fix: Relaxed is sufficient
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.inner.pending.lock().await;
            pending.insert(id, tx);
        }

        let cmd = CdpCommand {
            id,
            method: method.to_string(),
            params: if params.is_null() { None } else { Some(params) },
            session_id,
        };

        let msg = serde_json::to_string(&cmd)?;
        if self.inner.write_tx.send(msg).await.is_err() {
            // Clean up pending entry on send failure (C5 fix)
            self.inner.pending.lock().await.remove(&id);
            return Err(anyhow!("WebSocket writer closed"));
        }

        // Wait for response with timeout, clean up pending on failure (C5 fix)
        let response = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(anyhow!("CDP response channel dropped: {method}"));
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(anyhow!("CDP command timeout: {method}"));
            }
        };

        if let Some(error) = response.error {
            return Err(anyhow!("CDP error {}: {}", error.code, error.message));
        }

        Ok(response.result.unwrap_or(json!({})))
    }

    /// Subscribe to CDP events. Returns a broadcast receiver.
    pub fn subscribe_events(&self) -> broadcast::Receiver<CdpEvent> {
        self.inner.event_tx.subscribe()
    }

    /// Enable a CDP domain.
    pub async fn enable_domain(&self, domain: &str) -> Result<()> {
        self.call(&format!("{domain}.enable"), json!({})).await?;
        Ok(())
    }

    /// Enable a CDP domain with params.
    pub async fn enable_domain_with(&self, domain: &str, params: serde_json::Value) -> Result<()> {
        self.call(&format!("{domain}.enable"), params).await?;
        Ok(())
    }
}
