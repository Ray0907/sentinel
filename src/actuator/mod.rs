//! Actuator Layer: sends CDP commands for user interaction
//! and produces ObservationReports.

pub mod input;
pub mod navigation;
pub mod report;

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use crate::cdp::client::CdpClient;

/// Commands the actor can execute.
#[derive(Debug, Clone)]
pub enum ActuatorCommand {
    Navigate { url: String },
    Click { selector: String },
    Type { selector: String, text: String },
    Snapshot,
    /// Enable streaming mode: actor sends every event to the stream channel.
    EnableStreaming,
}

/// A real-time event emitted during watch/streaming mode.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct StreamEvent {
    /// Milliseconds since streaming started
    pub time_ms: u64,
    /// Event category
    pub category: String,
    /// Event description
    pub detail: String,
    /// Target session ID for child-target events (OOPIF, iframe, worker)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Observation report produced after an action settles.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ObservationReport {
    pub action: String,
    pub state: String,
    pub time_to_stable_ms: u64,
    pub dom_mutations: Vec<String>,
    pub layout_shifts: Vec<f64>,
    pub network_requests: Vec<String>,
    pub errors: Vec<String>,
    pub console_messages: Vec<String>,
    pub total_events: usize,
    /// Error from the action itself (e.g., element not found for click/type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_error: Option<String>,
    /// HTTP-level errors observed during the action window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_errors: Vec<String>,
    /// Visual diff result comparing before/after screenshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual_diff: Option<crate::diff::visual_diff::VisualDiffResult>,
}

/// Execute a click on an element matched by CSS selector.
pub async fn execute_click(cdp: &CdpClient, selector: &str) -> Result<()> {
    // Resolve the element's position via DOM.querySelector + DOM.getBoxModel
    let doc = cdp.call("DOM.getDocument", json!({})).await?;
    let root_node_id = doc["root"]["nodeId"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("No root nodeId"))?;

    let result = cdp
        .call(
            "DOM.querySelector",
            json!({
                "nodeId": root_node_id,
                "selector": selector
            }),
        )
        .await?;

    let node_id = result["nodeId"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Element not found: {selector}"))?;

    if node_id == 0 {
        return Err(anyhow::anyhow!("Element not found: {selector}"));
    }

    // Scroll element into view before clicking (Codex review M10 fix)
    let _ = cdp
        .call("DOM.scrollIntoViewIfNeeded", json!({"nodeId": node_id}))
        .await;

    // Get the box model to find click coordinates
    let box_model = cdp
        .call("DOM.getBoxModel", json!({"nodeId": node_id}))
        .await?;

    let content = &box_model["model"]["content"];
    let points: Vec<f64> = content
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid box model"))?
        .iter()
        .filter_map(|v| v.as_f64())
        .collect();

    if points.len() < 8 {
        return Err(anyhow::anyhow!("Invalid box model points"));
    }

    // Calculate center point (content quad: x1,y1 x2,y2 x3,y3 x4,y4)
    let x = (points[0] + points[2] + points[4] + points[6]) / 4.0;
    let y = (points[1] + points[3] + points[5] + points[7]) / 4.0;

    // Dispatch mouse events: move → down → up
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseMoved",
            "x": x,
            "y": y
        }),
    )
    .await?;

    cdp.call(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mousePressed",
            "x": x,
            "y": y,
            "button": "left",
            "clickCount": 1
        }),
    )
    .await?;

    cdp.call(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseReleased",
            "x": x,
            "y": y,
            "button": "left",
            "clickCount": 1
        }),
    )
    .await?;

    Ok(())
}

/// Execute typing into an element matched by CSS selector.
pub async fn execute_type(cdp: &CdpClient, selector: &str, text: &str) -> Result<()> {
    // Focus the element first
    let doc = cdp.call("DOM.getDocument", json!({})).await?;
    let root_node_id = doc["root"]["nodeId"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("No root nodeId"))?;

    let result = cdp
        .call(
            "DOM.querySelector",
            json!({
                "nodeId": root_node_id,
                "selector": selector
            }),
        )
        .await?;

    let node_id = result["nodeId"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Element not found: {selector}"))?;

    if node_id == 0 {
        return Err(anyhow::anyhow!("Element not found: {selector}"));
    }

    cdp.call("DOM.focus", json!({"nodeId": node_id})).await?;

    // M3 fix: use Input.insertText for reliable text input
    cdp.call("Input.insertText", json!({"text": text})).await?;

    Ok(())
}
