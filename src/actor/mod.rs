//! Page Actor: single tokio task that owns ALL mutable page state.
//! No shared mutable state = no races.

pub mod dom_tree;
pub mod network;
pub mod reconcile;
pub mod stability;
pub mod timeline;

use anyhow::Result;
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::actuator::{ActuatorCommand, ObservationReport, StreamEvent};
use crate::cdp::client::CdpClient;
use crate::diff::visual_diff;
use crate::cdp::types::*;
use crate::sensor::{SensorEvent, TaggedSensorEvent};

use dom_tree::LiveDomTree;
use network::NetworkTracker;
use stability::{StabilityState, StabilityTracker};
use timeline::Timeline;

/// Observation event stored in the timeline.
#[derive(Debug, Clone)]
pub struct ObservationEvent {
    pub timestamp: Instant,
    pub epoch: u64,
    pub kind: ObservationKind,
}

#[derive(Debug, Clone)]
pub enum ObservationKind {
    DomMutation(String), // description of DOM change
    LayoutShift { value: f64 },
    NetworkRequest { url: String, method: String, target: Option<String> },
    NetworkResponse { url: String, status: i32, target: Option<String> },
    NetworkComplete { request_id: String, target: Option<String> },
    NetworkError { url: String, error: String, target: Option<String> },
    ConsoleMessage { level: String, text: String, target: Option<String> },
    Error { text: String, target: Option<String> },
    Navigation { url: String },
    AnimationStarted { id: String },
    AnimationEnded { id: String },
    FrameCapture,
    Lifecycle { name: String },
    AxUpdate { node_count: usize },
}

/// The Page Actor — owns all mutable state for one page.
struct PageActor {
    cdp: CdpClient,
    page_epoch: u64,
    main_frame_id: Option<String>, // H2 fix: track main frame
    dom_tree: LiveDomTree,
    timeline: Timeline<ObservationEvent>,
    stability: StabilityTracker,
    network: NetworkTracker,
    animations: HashMap<String, Instant>,
    child_targets: HashMap<String, TargetInfo>,
    mutation_burst_count: u32,
    last_burst_check: Instant,
    needs_reconciliation: bool,

    // Action tracking
    current_action: Option<String>,
    action_start: Option<Instant>,
    last_action_error: Option<String>,
    pre_action_screenshot: Option<image::DynamicImage>,

    // Streaming mode
    streaming: bool,
    stream_start: Instant,
    stream_tx: Option<mpsc::Sender<StreamEvent>>,

    // Channels
    event_rx: mpsc::Receiver<TaggedSensorEvent>,
    cmd_rx: mpsc::Receiver<ActuatorCommand>,
    report_tx: mpsc::Sender<ObservationReport>,
}

/// Run the page actor event loop.
pub async fn run(
    cdp: CdpClient,
    event_rx: mpsc::Receiver<TaggedSensorEvent>,
    cmd_rx: mpsc::Receiver<ActuatorCommand>,
    report_tx: mpsc::Sender<ObservationReport>,
) -> Result<()> {
    run_with_stream(cdp, event_rx, cmd_rx, report_tx, None).await
}

/// Run the page actor with optional streaming channel.
pub async fn run_with_stream(
    cdp: CdpClient,
    event_rx: mpsc::Receiver<TaggedSensorEvent>,
    cmd_rx: mpsc::Receiver<ActuatorCommand>,
    report_tx: mpsc::Sender<ObservationReport>,
    stream_tx: Option<mpsc::Sender<StreamEvent>>,
) -> Result<()> {
    let mut actor = PageActor {
        cdp,
        page_epoch: 0,
        main_frame_id: None,
        dom_tree: LiveDomTree::new(),
        timeline: Timeline::new(10_000, std::time::Duration::from_secs(60)),
        stability: StabilityTracker::new(),
        network: NetworkTracker::new(),
        animations: HashMap::new(),
        child_targets: HashMap::new(),
        mutation_burst_count: 0,
        last_burst_check: Instant::now(),
        needs_reconciliation: false,
        current_action: None,
        action_start: None,
        last_action_error: None,
        pre_action_screenshot: None,
        streaming: stream_tx.is_some(),
        stream_start: Instant::now(),
        stream_tx,
        event_rx,
        cmd_rx,
        report_tx,
    };

    actor.run_loop().await
}

impl PageActor {
    async fn run_loop(&mut self) -> Result<()> {
        // Stability check interval
        let mut stability_interval = tokio::time::interval(std::time::Duration::from_millis(100));

        loop {
            tokio::select! {
                // Process sensor events (highest priority)
                Some(tagged) = self.event_rx.recv() => {
                    self.handle_tagged_event(tagged).await;
                }
                // Process actuator commands
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                // Periodic stability check and deferred reconciliation
                _ = stability_interval.tick() => {
                    self.check_stability().await;
                    self.maybe_reconcile().await;
                }
                else => {
                    tracing::info!("All channels closed, actor shutting down");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Dispatch a tagged sensor event. Child-target DOM events are skipped
    /// for the main DOM tree, but network, console, and error events from
    /// child targets are still recorded in the timeline with target provenance.
    async fn handle_tagged_event(&mut self, tagged: TaggedSensorEvent) {
        let TaggedSensorEvent { event, session_id } = tagged;
        if let Some(ref sid) = session_id {
            // Child-target event: only record non-DOM events
            match event {
                // Network events from child targets
                SensorEvent::RequestWillBeSent(data) => {
                    self.stability.on_network_activity(Instant::now());
                    self.record_with_target(
                        ObservationKind::NetworkRequest {
                            url: data.request.url,
                            method: data.request.method,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                SensorEvent::ResponseReceived(data) => {
                    self.record_with_target(
                        ObservationKind::NetworkResponse {
                            url: data.response.url,
                            status: data.response.status,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                SensorEvent::LoadingFinished(data) => {
                    self.stability.on_network_complete(Instant::now());
                    self.record_with_target(
                        ObservationKind::NetworkComplete {
                            request_id: data.request_id,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                SensorEvent::LoadingFailed(data) => {
                    self.stability.on_network_complete(Instant::now());
                    self.record_with_target(
                        ObservationKind::NetworkError {
                            url: String::new(),
                            error: data.error_text,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                // Console/error events from child targets
                SensorEvent::ConsoleApiCalled(data) => {
                    let text = data
                        .args
                        .iter()
                        .map(|a| {
                            a.description
                                .clone()
                                .or_else(|| a.value.as_ref().map(|v| v.to_string()))
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    self.record_with_target(
                        ObservationKind::ConsoleMessage {
                            level: data.call_type,
                            text,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                SensorEvent::ExceptionThrown(data) => {
                    self.record_with_target(
                        ObservationKind::Error {
                            text: data.exception_details.text,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                SensorEvent::LogEntryAdded(data) => {
                    self.record_with_target(
                        ObservationKind::ConsoleMessage {
                            level: data.level,
                            text: data.text,
                            target: Some(sid.clone()),
                        },
                        Some(sid.clone()),
                    );
                }
                // Target events are always processed
                SensorEvent::AttachedToTarget(data) => {
                    tracing::info!(
                        session = data.session_id,
                        target_type = data.target_info.target_type,
                        url = data.target_info.url,
                        "Target attached"
                    );
                    self.child_targets.insert(data.session_id, data.target_info);
                }
                SensorEvent::DetachedFromTarget(data) => {
                    self.child_targets.remove(&data.session_id);
                    tracing::info!(session = data.session_id, "Target detached");
                }
                // Skip DOM events from child targets — don't apply to main DOM tree
                _ => {
                    tracing::trace!(
                        session_id = sid,
                        "Skipping child-target DOM/page event"
                    );
                }
            }
            return;
        }

        // Main target event: full processing
        self.handle_sensor_event(event).await;
    }

    async fn handle_sensor_event(&mut self, event: SensorEvent) {
        let now = Instant::now();

        match event {
            // ── DOM Events ──
            SensorEvent::DocumentUpdated => {
                self.page_epoch += 1;
                self.dom_tree.clear();
                self.stability.on_dom_mutation(now);
                self.record(ObservationKind::DomMutation(
                    "documentUpdated (epoch bump)".into(),
                ));
                tracing::info!(epoch = self.page_epoch, "Document updated, tree reset");

                // Re-request the document tree, then schedule reconciliation
                // to verify the tree is correct
                if let Ok(result) = self
                    .cdp
                    .call(
                        "DOM.getDocument",
                        serde_json::json!({"depth": -1, "pierce": true}),
                    )
                    .await
                {
                    if let Some(root) = result.get("root") {
                        if let Ok(node) = serde_json::from_value::<DomNode>(root.clone()) {
                            self.dom_tree.set_root(node);
                        }
                    }
                }
                // Schedule reconciliation to ensure tree correctness after navigation
                self.needs_reconciliation = true;
            }

            SensorEvent::SetChildNodes(data) => {
                self.dom_tree.set_children(data.parent_id, data.nodes);
                self.stability.on_dom_mutation(now);
                self.track_mutation_burst();
            }

            SensorEvent::ChildNodeInserted(data) => {
                let desc = format!(
                    "inserted <{}> into node {}",
                    data.node.node_name, data.parent_node_id
                );
                self.dom_tree
                    .insert_child(data.parent_node_id, data.previous_node_id, data.node);
                self.stability.on_dom_mutation(now);
                self.record(ObservationKind::DomMutation(desc));
                self.track_mutation_burst();
            }

            SensorEvent::ChildNodeRemoved(data) => {
                let desc = format!(
                    "removed node {} from parent {}",
                    data.node_id, data.parent_node_id
                );
                self.dom_tree
                    .remove_child(data.parent_node_id, data.node_id);
                self.stability.on_dom_mutation(now);
                self.record(ObservationKind::DomMutation(desc));
                self.track_mutation_burst();
            }

            SensorEvent::AttributeModified(data) => {
                self.dom_tree
                    .set_attribute(data.node_id, &data.name, &data.value);
                self.stability.on_dom_mutation(now);
                self.record(ObservationKind::DomMutation(format!(
                    "attr {}=\"{}\" on node {}",
                    data.name, data.value, data.node_id
                )));
            }

            SensorEvent::AttributeRemoved(data) => {
                self.dom_tree.remove_attribute(data.node_id, &data.name);
                self.stability.on_dom_mutation(now);
            }

            SensorEvent::CharacterDataModified(data) => {
                self.dom_tree
                    .set_character_data(data.node_id, &data.character_data);
                self.stability.on_dom_mutation(now);
                self.record(ObservationKind::DomMutation(format!(
                    "text changed on node {}",
                    data.node_id
                )));
            }

            SensorEvent::ChildNodeCountUpdated(data) => {
                self.dom_tree
                    .update_child_count(data.node_id, data.child_node_count);
            }

            SensorEvent::InlineStyleInvalidated(data) => {
                self.stability.on_style_change(now);
                for node_id in &data.node_ids {
                    self.record(ObservationKind::DomMutation(format!(
                        "inline style invalidated on node {}",
                        node_id
                    )));
                }
            }

            SensorEvent::ShadowRootPushed(data) => {
                self.dom_tree.add_shadow_root(data.host_id, data.root);
                self.stability.on_dom_mutation(now);
            }

            SensorEvent::ShadowRootPopped(data) => {
                self.dom_tree
                    .remove_shadow_root(data.host_id, data.root.node_id);
                self.stability.on_dom_mutation(now);
            }

            SensorEvent::PseudoElementAdded(data) => {
                self.dom_tree
                    .add_pseudo_element(data.parent_id, data.pseudo_element);
                self.stability.on_dom_mutation(now);
            }

            SensorEvent::PseudoElementRemoved(data) => {
                self.dom_tree
                    .remove_pseudo_element(data.parent_id, data.pseudo_element.node_id);
                self.stability.on_dom_mutation(now);
            }

            // ── Page Events ──
            SensorEvent::LifecycleEvent(data) => {
                self.stability.on_lifecycle(&data.name, now);
                self.record(ObservationKind::Lifecycle {
                    name: data.name.clone(),
                });
                tracing::debug!(event = data.name, frame = data.frame_id, "Lifecycle");
            }

            SensorEvent::FrameNavigated(data) => {
                let is_main_frame = data.frame.parent_id.is_none();
                if is_main_frame {
                    // H2 fix: only bump epoch for main frame navigation
                    self.page_epoch += 1;
                    self.main_frame_id = Some(data.frame.id.clone());
                    self.stability.on_navigation(now);
                    tracing::info!(
                        url = data.frame.url,
                        epoch = self.page_epoch,
                        "Main frame navigated"
                    );
                }
                self.record(ObservationKind::Navigation {
                    url: data.frame.url.clone(),
                });
            }

            SensorEvent::FrameStartedLoading { frame_id } => {
                // Don't increment active_navigations here — FrameNavigated already did
                tracing::debug!(frame = frame_id, "Frame loading started");
            }

            SensorEvent::FrameStoppedLoading { frame_id } => {
                self.stability.on_navigation_end(now);
                tracing::debug!(frame = frame_id, "Frame loading stopped");
            }

            SensorEvent::NavigatedWithinDocument { url, .. } => {
                // SPA navigation: don't increment active_navigations (no FrameStoppedLoading)
                // Just signal activity for the stability tracker
                self.stability.on_dom_mutation(now);
                self.record(ObservationKind::Navigation { url });
            }

            SensorEvent::FrameResized => {
                self.stability.on_layout_shift(now);
            }

            SensorEvent::ScreencastFrame(_frame) => {
                self.record(ObservationKind::FrameCapture);
                // TODO: store frame in ring buffer
            }

            // ── Network Events ──
            SensorEvent::RequestWillBeSent(data) => {
                self.network.on_request_sent(
                    &data.request_id,
                    &data.request.url,
                    &data.request.method,
                );
                self.stability.on_network_activity(now);
                self.record(ObservationKind::NetworkRequest {
                    url: data.request.url,
                    method: data.request.method,
                    target: None,
                });
            }

            SensorEvent::ResponseReceived(data) => {
                self.network
                    .on_response(&data.request_id, data.response.status);
                self.record(ObservationKind::NetworkResponse {
                    url: data.response.url,
                    status: data.response.status,
                    target: None,
                });
            }

            SensorEvent::LoadingFinished(data) => {
                self.network.on_complete(&data.request_id);
                self.stability.on_network_complete(now);
                self.record(ObservationKind::NetworkComplete {
                    request_id: data.request_id,
                    target: None,
                });
            }

            SensorEvent::LoadingFailed(data) => {
                let error = data.error_text.clone();
                // Get URL BEFORE removing from tracker (C3 regression fix)
                let url = self.network.get_url(&data.request_id).unwrap_or_default();
                self.network.on_failed(&data.request_id, &error);
                self.stability.on_network_complete(now);
                self.record(ObservationKind::NetworkError { url, error, target: None });
            }

            SensorEvent::WebSocketCreated(data) => {
                self.network
                    .on_websocket_opened(&data.request_id, &data.url);
                self.stability.on_long_lived_connection(&data.request_id);
            }

            SensorEvent::WebSocketClosed(data) => {
                self.network.on_websocket_closed(&data.request_id);
                self.stability.on_long_lived_disconnection(&data.request_id);
            }

            // ── Runtime Events ──
            SensorEvent::ConsoleApiCalled(data) => {
                let text = data
                    .args
                    .iter()
                    .map(|a| {
                        a.description
                            .clone()
                            .or_else(|| a.value.as_ref().map(|v| v.to_string()))
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.record(ObservationKind::ConsoleMessage {
                    level: data.call_type,
                    text,
                    target: None,
                });
            }

            SensorEvent::ExceptionThrown(data) => {
                self.record(ObservationKind::Error {
                    text: data.exception_details.text.clone(),
                    target: None,
                });
                tracing::warn!(error = data.exception_details.text, "JS exception");
            }

            SensorEvent::LogEntryAdded(data) => {
                self.record(ObservationKind::ConsoleMessage {
                    level: data.level,
                    text: data.text,
                    target: None,
                });
            }

            // ── Animation Events ──
            SensorEvent::AnimationStarted(data) => {
                let id = data.animation.id.clone();
                self.animations.insert(id.clone(), now);
                self.stability.on_animation_start(now);
                self.record(ObservationKind::AnimationStarted { id });
            }

            SensorEvent::AnimationCanceled(data) => {
                self.animations.remove(&data.id);
                self.stability.on_animation_end(now);
                self.record(ObservationKind::AnimationEnded { id: data.id });
            }

            // ── Performance Events ──
            SensorEvent::PerformanceMetrics(_) => {
                // Stored but not acted on for now
            }

            SensorEvent::LayoutShift(data) => {
                self.stability.on_layout_shift(now);
                self.record(ObservationKind::LayoutShift { value: data.value });
                if data.value > 0.1 {
                    tracing::warn!(cls = data.value, "Significant layout shift detected");
                }
            }

            // ── Target Events ──
            SensorEvent::AttachedToTarget(data) => {
                tracing::info!(
                    session = data.session_id,
                    target_type = data.target_info.target_type,
                    url = data.target_info.url,
                    "Target attached"
                );
                // Enable CDP domains on the child session so we receive its events
                let sid = data.session_id.clone();
                let target_type = data.target_info.target_type.clone();
                self.child_targets
                    .insert(data.session_id, data.target_info);

                // Only enable domains for page/iframe targets (not service workers etc.)
                if target_type == "page" || target_type == "iframe" {
                    self.enable_child_target_domains(&sid).await;
                }
            }

            SensorEvent::DetachedFromTarget(data) => {
                self.child_targets.remove(&data.session_id);
                tracing::info!(session = data.session_id, "Target detached");
            }

            // ── Accessibility Events ──
            SensorEvent::AxNodesUpdated(data) => {
                self.record(ObservationKind::AxUpdate {
                    node_count: data.nodes.len(),
                });
            }
        }
    }

    /// Enable CDP domains on a child target session so we receive its events.
    async fn enable_child_target_domains(&self, session_id: &str) {
        let sid = Some(session_id.to_string());
        // Enable Network, Runtime, and Log on child targets
        // (DOM is intentionally skipped to avoid cross-contaminating the main tree)
        for domain in &["Network", "Runtime", "Log"] {
            let method = format!("{domain}.enable");
            if let Err(e) = self
                .cdp
                .call_with_session(&method, serde_json::json!({}), sid.clone())
                .await
            {
                tracing::debug!(
                    session = session_id,
                    domain = *domain,
                    error = %e,
                    "Failed to enable domain on child target"
                );
            }
        }
        tracing::info!(session = session_id, "Child target domains enabled (Network, Runtime, Log)");
    }

    /// Capture a screenshot via CDP for visual diff.
    async fn capture_screenshot(&self) -> Option<image::DynamicImage> {
        match self.cdp.call("Page.captureScreenshot", serde_json::json!({"format": "png"})).await {
            Ok(result) => {
                if let Some(data) = result["data"].as_str() {
                    visual_diff::decode_screenshot(data).ok()
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "Screenshot capture failed");
                None
            }
        }
    }

    async fn handle_command(&mut self, cmd: ActuatorCommand) {
        let now = Instant::now();
        self.current_action = Some(format!("{:?}", cmd));
        self.action_start = Some(now);
        self.last_action_error = None;
        self.timeline.mark_action_start();
        self.stability.begin_action(now);

        // Capture pre-action screenshot for visual diff
        self.pre_action_screenshot = self.capture_screenshot().await;

        match cmd {
            ActuatorCommand::Navigate { url } => {
                tracing::info!(url = url, "Executing navigate");
                if let Err(e) = self
                    .cdp
                    .call("Page.navigate", serde_json::json!({"url": url}))
                    .await
                {
                    tracing::error!(error = %e, "Navigate failed");
                    self.last_action_error = Some(format!("Navigate failed: {e}"));
                }
            }
            ActuatorCommand::Click { selector } => {
                tracing::info!(selector = selector, "Executing click");
                if let Err(e) = crate::actuator::execute_click(&self.cdp, &selector).await {
                    tracing::error!(error = %e, "Click failed");
                    self.last_action_error = Some(format!("Click failed: {e}"));
                }
            }
            ActuatorCommand::Type { selector, text } => {
                tracing::info!(selector = selector, text = text, "Executing type");
                if let Err(e) = crate::actuator::execute_type(&self.cdp, &selector, &text).await {
                    tracing::error!(error = %e, "Type failed");
                    self.last_action_error = Some(format!("Type failed: {e}"));
                }
            }
            ActuatorCommand::Snapshot => {
                tracing::info!("Taking compatibility snapshot");
                let tree = self.dom_tree.render();
                println!("--- DOM Tree ({} nodes) ---", self.dom_tree.node_count());
                println!("{tree}");
                println!("--- End DOM Tree ---");
            }
            ActuatorCommand::EnableStreaming => {
                // Streaming is set up via set_stream_tx, just mark start time
                self.streaming = true;
                self.stream_start = Instant::now();
                tracing::info!("Streaming mode enabled");
            }
        }
    }


    async fn check_stability(&mut self) {
        let state = self
            .stability
            .check(self.network.pending_count(), self.animations.len() as u32);

        match state {
            StabilityState::FullySettled | StabilityState::TimedOut => {
                if let Some(action) = self.current_action.take() {
                    let elapsed = self
                        .action_start
                        .take()
                        .map(|s| s.elapsed())
                        .unwrap_or_default();

                    let events = self.timeline.events_since_last_action();

                    let network_errors: Vec<String> = events
                        .iter()
                        .filter_map(|e| match &e.kind {
                            ObservationKind::NetworkError { url, error, .. } => {
                                Some(format!("{url}: {error}"))
                            }
                            ObservationKind::NetworkResponse { url, status, .. } if *status >= 400 => {
                                Some(format!("{url}: HTTP {status}"))
                            }
                            _ => None,
                        })
                        .collect();

                    // Capture post-action screenshot and compute visual diff
                    let visual_diff_result = if let Some(ref before) = self.pre_action_screenshot {
                        self.capture_screenshot()
                            .await
                            .map(|after| visual_diff::compare_frames(before, &after))
                    } else {
                        None
                    };
                    self.pre_action_screenshot = None;

                    let report = ObservationReport {
                        action,
                        state: format!("{:?}", state),
                        time_to_stable_ms: elapsed.as_millis() as u64,
                        dom_mutations: events
                            .iter()
                            .filter_map(|e| match &e.kind {
                                ObservationKind::DomMutation(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect(),
                        layout_shifts: events
                            .iter()
                            .filter_map(|e| match &e.kind {
                                ObservationKind::LayoutShift { value } => Some(*value),
                                _ => None,
                            })
                            .collect(),
                        network_requests: events
                            .iter()
                            .filter_map(|e| match &e.kind {
                                ObservationKind::NetworkRequest { url, method, .. } => {
                                    Some(format!("{method} {url}"))
                                }
                                _ => None,
                            })
                            .collect(),
                        errors: events
                            .iter()
                            .filter_map(|e| match &e.kind {
                                ObservationKind::Error { text, .. } => Some(text.clone()),
                                _ => None,
                            })
                            .collect(),
                        console_messages: events
                            .iter()
                            .filter_map(|e| match &e.kind {
                                ObservationKind::ConsoleMessage { level, text, .. } => {
                                    Some(format!("[{level}] {text}"))
                                }
                                _ => None,
                            })
                            .collect(),
                        total_events: events.len(),
                        action_error: self.last_action_error.take(),
                        network_errors,
                        visual_diff: visual_diff_result,
                    };

                    let _ = self.report_tx.send(report).await;
                }
            }
            _ => {}
        }
    }

    fn record(&mut self, kind: ObservationKind) {
        self.record_with_target(kind, None);
    }

    fn record_with_target(&mut self, kind: ObservationKind, target: Option<String>) {
        // Stream event in real-time if streaming mode is enabled
        if self.streaming {
            if let Some(ref tx) = self.stream_tx {
                let (category, detail) = match &kind {
                    ObservationKind::DomMutation(s) => ("dom", s.clone()),
                    ObservationKind::LayoutShift { value } => ("layout", format!("CLS {value:.4}")),
                    ObservationKind::NetworkRequest { url, method, .. } => ("network", format!("{method} {url}")),
                    ObservationKind::NetworkResponse { url, status, .. } => ("network", format!("{status} {url}")),
                    ObservationKind::NetworkComplete { request_id, .. } => ("network", format!("complete {request_id}")),
                    ObservationKind::NetworkError { url, error, .. } => ("error", format!("{url}: {error}")),
                    ObservationKind::ConsoleMessage { level, text, .. } => ("console", format!("[{level}] {text}")),
                    ObservationKind::Error { text, .. } => ("error", text.clone()),
                    ObservationKind::Navigation { url } => ("navigation", url.clone()),
                    ObservationKind::AnimationStarted { id } => ("animation", format!("started {id}")),
                    ObservationKind::AnimationEnded { id } => ("animation", format!("ended {id}")),
                    ObservationKind::FrameCapture => ("visual", "frame captured".into()),
                    ObservationKind::Lifecycle { name } => ("lifecycle", name.clone()),
                    ObservationKind::AxUpdate { node_count } => ("accessibility", format!("{node_count} nodes updated")),
                };
                let elapsed = self.stream_start.elapsed().as_millis() as u64;
                let _ = tx.try_send(StreamEvent {
                    time_ms: elapsed,
                    category: category.to_string(),
                    detail,
                    target: target.clone(),
                });
            }
        }

        self.timeline.push(ObservationEvent {
            timestamp: Instant::now(),
            epoch: self.page_epoch,
            kind,
        });
    }

    fn track_mutation_burst(&mut self) {
        self.mutation_burst_count += 1;
        let now = Instant::now();
        if now.duration_since(self.last_burst_check) > std::time::Duration::from_millis(500) {
            if self.mutation_burst_count > 100 {
                tracing::warn!(
                    count = self.mutation_burst_count,
                    "Mutation burst detected, triggering DOMSnapshot reconciliation"
                );
                self.needs_reconciliation = true;
            }
            self.mutation_burst_count = 0;
            self.last_burst_check = now;
        }
    }

    async fn maybe_reconcile(&mut self) {
        if !self.needs_reconciliation {
            return;
        }
        self.needs_reconciliation = false;

        match reconcile::reconcile_from_snapshot(&self.cdp, &mut self.dom_tree).await {
            Ok(node_count) => {
                tracing::info!(nodes = node_count, "Reconciliation succeeded");
            }
            Err(e) => {
                tracing::error!(error = %e, "Reconciliation failed, falling back to DOM.getDocument");
                // Fallback: request the full tree via DOM.getDocument
                if let Ok(result) = self
                    .cdp
                    .call(
                        "DOM.getDocument",
                        serde_json::json!({"depth": -1, "pierce": true}),
                    )
                    .await
                {
                    if let Some(root) = result.get("root") {
                        if let Ok(node) = serde_json::from_value::<DomNode>(root.clone()) {
                            self.dom_tree.set_root(node);
                        }
                    }
                }
            }
        }
    }
}
