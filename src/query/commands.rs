//! CLI command handlers. These coordinate between the CDP client,
//! actuator commands, and observation reports.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::actuator::{ObservationReport, StreamEvent};

/// Observe and print reports for a given duration.
pub async fn observe(
    mut report_rx: mpsc::Receiver<ObservationReport>,
    duration_secs: u64,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(duration_secs);

    tracing::info!(duration = duration_secs, "Observing page");

    loop {
        tokio::select! {
            Some(report) = report_rx.recv() => {
                let json = serde_json::to_string_pretty(&report)?;
                println!("{json}");
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!("Observation period ended");
                break;
            }
        }
    }

    Ok(())
}

/// Wait for the next observation report (action settled), print it, return the receiver.
pub async fn observe_until_settled(
    mut report_rx: mpsc::Receiver<ObservationReport>,
) -> Result<mpsc::Receiver<ObservationReport>> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15); // max 15s wait

    tokio::select! {
        Some(report) = report_rx.recv() => {
            let json = serde_json::to_string_pretty(&report)?;
            println!("{json}");
        }
        _ = tokio::time::sleep_until(deadline) => {
            tracing::warn!("Timed out waiting for action to settle");
        }
    }
    Ok(report_rx)
}

/// Stream events in real-time during watch mode.
pub async fn watch_stream(
    mut stream_rx: mpsc::Receiver<StreamEvent>,
    duration_secs: u64,
    filter: &str,
) -> Result<()> {
    let deadline = if duration_secs > 0 {
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(duration_secs)
    } else {
        // Effectively infinite
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(86400)
    };

    tracing::info!(duration = duration_secs, filter = filter, "Watching page");

    let filter_all = filter == "all";

    loop {
        tokio::select! {
            Some(event) = stream_rx.recv() => {
                // Apply filter
                if filter_all || event.category == filter {
                    // Print as compact JSONL for real-time consumption
                    if let Ok(json) = serde_json::to_string(&event) {
                        println!("{json}");
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!("Watch period ended");
                break;
            }
        }
    }

    Ok(())
}
