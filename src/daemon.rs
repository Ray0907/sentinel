//! Daemon mode: persistent background process with Unix socket API.
//!
//! The daemon keeps Chrome open between commands, allowing multiple
//! CLI invocations to share the same browser session.
//!
//! Protocol: newline-delimited JSON over Unix socket.
//! Client sends a JSON command, daemon sends a JSON response.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::actuator::{ActuatorCommand, ObservationReport};

/// JSON command sent by the CLI client to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonCommand {
    pub action: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub duration: Option<u64>,
    #[serde(default)]
    pub filter: Option<String>,
}

/// JSON response sent by the daemon to the CLI client.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ObservationReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl DaemonResponse {
    fn ok(message: &str) -> Self {
        Self {
            ok: true,
            error: None,
            report: None,
            message: Some(message.to_string()),
        }
    }

    fn ok_with_report(report: ObservationReport) -> Self {
        Self {
            ok: true,
            error: None,
            report: Some(report),
            message: None,
        }
    }

    fn err(error: &str) -> Self {
        Self {
            ok: false,
            error: Some(error.to_string()),
            report: None,
            message: None,
        }
    }
}

/// Get the daemon socket path for a given port.
pub fn socket_path(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!("sentinel-{port}.sock"))
}

/// Check if a daemon is already running on the given port.
pub async fn is_running(port: u16) -> bool {
    let path = socket_path(port);
    if !path.exists() {
        return false;
    }
    // Try to connect
    UnixStream::connect(&path).await.is_ok()
}

/// Run the daemon server. This blocks until the daemon is shut down.
pub async fn run_daemon(
    port: u16,
    cmd_tx: mpsc::Sender<ActuatorCommand>,
    mut report_rx: mpsc::Receiver<ObservationReport>,
) -> Result<()> {
    let path = socket_path(port);

    // Remove stale socket file
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    let listener = UnixListener::bind(&path)?;
    tracing::info!(socket = %path.display(), "Daemon listening");

    // Shared report channel: daemon holds it, passes reports to active client
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let cmd_tx = cmd_tx.clone();
                // Handle one command per connection
                if let Err(e) = handle_client(stream, &cmd_tx, &mut report_rx).await {
                    tracing::warn!(error = %e, "Client handler error");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Accept error");
            }
        }
    }
}

/// Handle a single client connection.
async fn handle_client(
    stream: UnixStream,
    cmd_tx: &mpsc::Sender<ActuatorCommand>,
    report_rx: &mut mpsc::Receiver<ObservationReport>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Read one line (one JSON command)
    reader.read_line(&mut line).await?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }

    let cmd: DaemonCommand =
        serde_json::from_str(line).map_err(|e| anyhow!("Invalid command JSON: {e}"))?;

    tracing::info!(action = cmd.action, "Daemon received command");

    let response = match cmd.action.as_str() {
        "navigate" => {
            let url = cmd.url.ok_or_else(|| anyhow!("Missing 'url'"))?;
            cmd_tx
                .send(ActuatorCommand::Navigate { url })
                .await
                .map_err(|_| anyhow!("Actor channel closed"))?;

            // Wait for report
            match tokio::time::timeout(std::time::Duration::from_secs(15), report_rx.recv()).await {
                Ok(Some(report)) => DaemonResponse::ok_with_report(report),
                Ok(None) => DaemonResponse::err("Actor shut down"),
                Err(_) => DaemonResponse::err("Timeout waiting for report"),
            }
        }
        "click" => {
            let selector = cmd.selector.ok_or_else(|| anyhow!("Missing 'selector'"))?;
            cmd_tx
                .send(ActuatorCommand::Click { selector })
                .await
                .map_err(|_| anyhow!("Actor channel closed"))?;

            match tokio::time::timeout(std::time::Duration::from_secs(15), report_rx.recv()).await {
                Ok(Some(report)) => DaemonResponse::ok_with_report(report),
                Ok(None) => DaemonResponse::err("Actor shut down"),
                Err(_) => DaemonResponse::err("Timeout waiting for report"),
            }
        }
        "type" => {
            let selector = cmd.selector.ok_or_else(|| anyhow!("Missing 'selector'"))?;
            let text = cmd.text.ok_or_else(|| anyhow!("Missing 'text'"))?;
            cmd_tx
                .send(ActuatorCommand::Type { selector, text })
                .await
                .map_err(|_| anyhow!("Actor channel closed"))?;

            match tokio::time::timeout(std::time::Duration::from_secs(15), report_rx.recv()).await {
                Ok(Some(report)) => DaemonResponse::ok_with_report(report),
                Ok(None) => DaemonResponse::err("Actor shut down"),
                Err(_) => DaemonResponse::err("Timeout waiting for report"),
            }
        }
        "snapshot" => {
            cmd_tx
                .send(ActuatorCommand::Snapshot)
                .await
                .map_err(|_| anyhow!("Actor channel closed"))?;
            DaemonResponse::ok("Snapshot printed to daemon stdout")
        }
        "ping" => DaemonResponse::ok("pong"),
        "shutdown" => {
            // Send response before shutting down
            let resp = DaemonResponse::ok("Shutting down");
            let json = serde_json::to_string(&resp)?;
            writer.write_all(json.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            std::process::exit(0);
        }
        other => DaemonResponse::err(&format!("Unknown action: {other}")),
    };

    let json = serde_json::to_string(&response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    Ok(())
}

/// Send a command to a running daemon and print the response.
pub async fn send_to_daemon(port: u16, cmd: DaemonCommand) -> Result<()> {
    let path = socket_path(port);
    let stream = UnixStream::connect(&path).await.map_err(|_| {
        anyhow!("No daemon running on port {port}. Start one with: sentinel daemon start")
    })?;

    let (reader, mut writer) = stream.into_split();

    // Send command
    let json = serde_json::to_string(&cmd)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    // Read response
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: DaemonResponse = serde_json::from_str(line.trim())?;

    if let Some(report) = response.report {
        let pretty = serde_json::to_string_pretty(&report)?;
        println!("{pretty}");
    } else if let Some(msg) = response.message {
        eprintln!("{msg}");
    } else if let Some(err) = response.error {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }

    Ok(())
}
