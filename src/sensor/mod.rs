//! Sensor Layer: subscribes to CDP domains, normalizes events into SensorEvent,
//! and forwards them to the Page Actor via mpsc channel.

pub mod console;
pub mod dom;
pub mod network;
pub mod page;

use anyhow::Result;
use serde_json::json;
use tokio::sync::mpsc;

use crate::cdp::client::CdpClient;
use crate::cdp::types::*;

/// Envelope wrapping a SensorEvent with optional target session metadata.
/// Events from the main target have `session_id = None`.
/// Events from child targets (OOPIF, iframe, worker) carry the session_id.
#[derive(Debug, Clone)]
pub struct TaggedSensorEvent {
    pub event: SensorEvent,
    /// CDP session ID for child-target events; None for main target.
    pub session_id: Option<String>,
}

/// Normalized event from any CDP domain, ready for the Page Actor.
#[derive(Debug, Clone)]
pub enum SensorEvent {
    // DOM events
    DocumentUpdated,
    SetChildNodes(SetChildNodes),
    ChildNodeInserted(ChildNodeInserted),
    ChildNodeRemoved(ChildNodeRemoved),
    AttributeModified(AttributeModified),
    AttributeRemoved(AttributeRemoved),
    CharacterDataModified(CharacterDataModified),
    ChildNodeCountUpdated(ChildNodeCountUpdated),
    InlineStyleInvalidated(InlineStyleInvalidated),
    ShadowRootPushed(ShadowRootEvent),
    ShadowRootPopped(ShadowRootEvent),
    PseudoElementAdded(PseudoElementEvent),
    PseudoElementRemoved(PseudoElementEvent),

    // Page events
    LifecycleEvent(LifecycleEvent),
    FrameNavigated(FrameNavigated),
    FrameStartedLoading { frame_id: String },
    FrameStoppedLoading { frame_id: String },
    NavigatedWithinDocument { frame_id: String, url: String },
    FrameResized,
    ScreencastFrame(ScreencastFrame),

    // Network events
    RequestWillBeSent(RequestWillBeSent),
    ResponseReceived(ResponseReceived),
    LoadingFinished(LoadingFinished),
    LoadingFailed(LoadingFailed),
    WebSocketCreated(WebSocketCreated),
    WebSocketClosed(WebSocketClosed),

    // Runtime events
    ConsoleApiCalled(ConsoleApiCalled),
    ExceptionThrown(ExceptionThrown),
    LogEntryAdded(LogEntry),

    // Animation events
    AnimationStarted(AnimationStarted),
    AnimationCanceled(AnimationCanceled),

    // Performance events
    PerformanceMetrics(PerformanceMetrics),
    LayoutShift(LayoutShiftDetails),

    // Target events
    AttachedToTarget(AttachedToTarget),
    DetachedFromTarget(DetachedFromTarget),

    // Accessibility events
    AxNodesUpdated(AxNodesUpdated),
}

/// Enable all CDP domains and start routing events to the actor.
/// The `ready_tx` is signaled when domain enablement is complete.
pub async fn run(
    cdp: CdpClient,
    tx: mpsc::Sender<TaggedSensorEvent>,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<()> {
    // Subscribe BEFORE enabling domains to avoid missing bootstrap events (H1 fix)
    let mut events = cdp.subscribe_events();

    // Enable all required CDP domains
    if let Err(e) = enable_domains(&cdp).await {
        tracing::error!(error = %e, "Failed to enable CDP domains");
        return Err(e);
    }
    tracing::info!("All CDP domains enabled");

    // Signal readiness
    if let Some(ready) = ready_tx {
        let _ = ready.send(());
    }

    // Route events to the actor, tagging child-target events with session_id
    loop {
        match events.recv().await {
            Ok(event) => {
                let session_id = event.session_id.clone();
                if let Some(sensor_event) = normalize_event(&event) {
                    let tagged = TaggedSensorEvent {
                        event: sensor_event,
                        session_id,
                    };
                    if tx.send(tagged).await.is_err() {
                        tracing::warn!("Actor channel closed, sensor shutting down");
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "Sensor lagged — triggering full DOM resync");
                // Force tree rebuild since we lost events (H6 fix)
                let _ = tx
                    .send(TaggedSensorEvent {
                        event: SensorEvent::DocumentUpdated,
                        session_id: None,
                    })
                    .await;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("Event stream closed, sensor shutting down");
                break;
            }
        }
    }

    Ok(())
}

/// Enable all CDP domains needed for continuous observation.
async fn enable_domains(cdp: &CdpClient) -> Result<()> {
    // DOM — with pierce for shadow roots and iframes
    cdp.call("DOM.enable", json!({"includeWhitespace": "none"}))
        .await?;

    // Request the full document tree to bootstrap the live DOM
    cdp.call("DOM.getDocument", json!({"depth": -1, "pierce": true}))
        .await?;

    // Page
    cdp.enable_domain("Page").await?;
    cdp.call("Page.setLifecycleEventsEnabled", json!({"enabled": true}))
        .await?;

    // Network
    cdp.enable_domain("Network").await?;

    // Runtime
    cdp.enable_domain("Runtime").await?;

    // CSS
    cdp.enable_domain("CSS").await?;

    // Log
    cdp.enable_domain("Log").await?;

    // Animation
    cdp.enable_domain("Animation").await?;

    // Performance
    cdp.enable_domain("Performance").await?;

    // Performance Timeline (for layout-shift events)
    // Some Chrome versions don't support all event types — try with fallback
    if let Err(e) = cdp
        .call(
            "PerformanceTimeline.enable",
            json!({"eventTypes": ["layout-shift"]}),
        )
        .await
    {
        tracing::warn!(error = %e, "PerformanceTimeline.enable failed, layout shift detection disabled");
    }

    // Accessibility (non-fatal — may not be available in all Chrome versions)
    if let Err(e) = cdp.call("Accessibility.enable", json!({})).await {
        tracing::warn!(error = %e, "Accessibility.enable failed");
    }

    // Target auto-attach (flatten=true for OOPIF support)
    cdp.call(
        "Target.setAutoAttach",
        json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": false,
            "flatten": true
        }),
    )
    .await?;

    Ok(())
}

/// Deserialize with error logging (C1 fix).
fn try_deser<T: serde::de::DeserializeOwned>(
    method: &str,
    params: &serde_json::Value,
) -> Option<T> {
    match serde_json::from_value::<T>(params.clone()) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(method = method, error = %e, "Failed to deserialize CDP event");
            None
        }
    }
}

/// Normalize a raw CDP event into a SensorEvent.
fn normalize_event(event: &CdpEvent) -> Option<SensorEvent> {
    let params = &event.params;
    let m = event.method.as_str();

    match m {
        // DOM
        "DOM.documentUpdated" => Some(SensorEvent::DocumentUpdated),
        "DOM.setChildNodes" => try_deser(m, params).map(SensorEvent::SetChildNodes),
        "DOM.childNodeInserted" => try_deser(m, params).map(SensorEvent::ChildNodeInserted),
        "DOM.childNodeRemoved" => try_deser(m, params).map(SensorEvent::ChildNodeRemoved),
        "DOM.attributeModified" => try_deser(m, params).map(SensorEvent::AttributeModified),
        "DOM.attributeRemoved" => try_deser(m, params).map(SensorEvent::AttributeRemoved),
        "DOM.characterDataModified" => try_deser(m, params).map(SensorEvent::CharacterDataModified),
        "DOM.childNodeCountUpdated" => try_deser(m, params).map(SensorEvent::ChildNodeCountUpdated),
        "DOM.inlineStyleInvalidated" => {
            try_deser(m, params).map(SensorEvent::InlineStyleInvalidated)
        }
        "DOM.shadowRootPushed" => try_deser(m, params).map(SensorEvent::ShadowRootPushed),
        "DOM.shadowRootPopped" => try_deser(m, params).map(SensorEvent::ShadowRootPopped),
        "DOM.pseudoElementAdded" => try_deser(m, params).map(SensorEvent::PseudoElementAdded),
        "DOM.pseudoElementRemoved" => try_deser(m, params).map(SensorEvent::PseudoElementRemoved),

        // Page
        "Page.lifecycleEvent" => try_deser(m, params).map(SensorEvent::LifecycleEvent),
        "Page.frameNavigated" => try_deser(m, params).map(SensorEvent::FrameNavigated),
        "Page.frameStartedLoading" => {
            let frame_id = params["frameId"].as_str()?.to_string();
            Some(SensorEvent::FrameStartedLoading { frame_id })
        }
        "Page.frameStoppedLoading" => {
            let frame_id = params["frameId"].as_str()?.to_string();
            Some(SensorEvent::FrameStoppedLoading { frame_id })
        }
        "Page.navigatedWithinDocument" => {
            let frame_id = params["frameId"].as_str()?.to_string();
            let url = params["url"].as_str()?.to_string();
            Some(SensorEvent::NavigatedWithinDocument { frame_id, url })
        }
        "Page.frameResized" => Some(SensorEvent::FrameResized),
        "Page.screencastFrame" => try_deser(m, params).map(SensorEvent::ScreencastFrame),

        // Network
        "Network.requestWillBeSent" => try_deser(m, params).map(SensorEvent::RequestWillBeSent),
        "Network.responseReceived" => try_deser(m, params).map(SensorEvent::ResponseReceived),
        "Network.loadingFinished" => try_deser(m, params).map(SensorEvent::LoadingFinished),
        "Network.loadingFailed" => try_deser(m, params).map(SensorEvent::LoadingFailed),
        "Network.webSocketCreated" => try_deser(m, params).map(SensorEvent::WebSocketCreated),
        "Network.webSocketClosed" => try_deser(m, params).map(SensorEvent::WebSocketClosed),

        // Runtime
        "Runtime.consoleAPICalled" => try_deser(m, params).map(SensorEvent::ConsoleApiCalled),
        "Runtime.exceptionThrown" => try_deser(m, params).map(SensorEvent::ExceptionThrown),

        // Log
        "Log.entryAdded" => {
            let entry = params.get("entry")?;
            match serde_json::from_value::<LogEntry>(entry.clone()) {
                Ok(v) => Some(SensorEvent::LogEntryAdded(v)),
                Err(e) => {
                    tracing::warn!(method = m, error = %e, "Failed to deserialize log entry");
                    None
                }
            }
        }

        // Animation
        "Animation.animationStarted" => try_deser(m, params).map(SensorEvent::AnimationStarted),
        "Animation.animationCanceled" => try_deser(m, params).map(SensorEvent::AnimationCanceled),

        // Performance
        "Performance.metrics" => try_deser(m, params).map(SensorEvent::PerformanceMetrics),

        // Performance Timeline
        "PerformanceTimeline.timelineEventAdded" => {
            let event: TimelineEventAdded = try_deser(m, params)?;
            if event.event.event_type == "layout-shift" {
                event
                    .event
                    .layout_shift_details
                    .map(SensorEvent::LayoutShift)
            } else {
                None
            }
        }

        // Target
        "Target.attachedToTarget" => try_deser(m, params).map(SensorEvent::AttachedToTarget),
        "Target.detachedFromTarget" => try_deser(m, params).map(SensorEvent::DetachedFromTarget),

        // Accessibility
        "Accessibility.nodesUpdated" => try_deser(m, params).map(SensorEvent::AxNodesUpdated),

        _ => None, // Unhandled event
    }
}
