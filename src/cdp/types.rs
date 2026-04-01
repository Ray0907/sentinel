//! Hand-written CDP types for v1. Covers DOM, Page, Network, Runtime, Input,
//! Target, Animation, Accessibility, Performance, CSS, Log, DOMSnapshot domains.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Generic CDP message types ──

#[derive(Debug, Serialize)]
pub struct CdpCommand {
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CdpResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<CdpError>,
    pub method: Option<String>,
    pub params: Option<serde_json::Value>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: serde_json::Value,
    pub session_id: Option<String>,
}

// ── DOM types ──

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DomNode {
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    #[serde(rename = "parentId")]
    pub parent_id: Option<i64>,
    #[serde(rename = "backendNodeId")]
    pub backend_node_id: i64,
    #[serde(rename = "nodeType")]
    pub node_type: i32,
    #[serde(rename = "nodeName")]
    pub node_name: String,
    #[serde(rename = "localName")]
    pub local_name: Option<String>,
    #[serde(rename = "nodeValue")]
    pub node_value: String,
    #[serde(rename = "childNodeCount")]
    pub child_node_count: Option<i32>,
    pub children: Option<Vec<DomNode>>,
    pub attributes: Option<Vec<String>>,
    #[serde(rename = "documentURL")]
    pub document_url: Option<String>,
    #[serde(rename = "baseURL")]
    pub base_url: Option<String>,
    #[serde(rename = "frameId")]
    pub frame_id: Option<String>,
    #[serde(rename = "contentDocument")]
    pub content_document: Option<Box<DomNode>>,
    #[serde(rename = "shadowRoots")]
    pub shadow_roots: Option<Vec<DomNode>>,
    #[serde(rename = "pseudoType")]
    pub pseudo_type: Option<String>,
    #[serde(rename = "pseudoIdentifier")]
    pub pseudo_identifier: Option<String>,
    #[serde(rename = "distributedNodes")]
    pub distributed_nodes: Option<Vec<BackendNode>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BackendNode {
    #[serde(rename = "nodeType")]
    pub node_type: i32,
    #[serde(rename = "nodeName")]
    pub node_name: String,
    #[serde(rename = "backendNodeId")]
    pub backend_node_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoxModel {
    pub content: Vec<f64>,
    pub padding: Vec<f64>,
    pub border: Vec<f64>,
    pub margin: Vec<f64>,
    pub width: i32,
    pub height: i32,
}

// ── DOM Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct ChildNodeInserted {
    #[serde(rename = "parentNodeId")]
    pub parent_node_id: i64,
    #[serde(rename = "previousNodeId")]
    pub previous_node_id: i64,
    pub node: DomNode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChildNodeRemoved {
    #[serde(rename = "parentNodeId")]
    pub parent_node_id: i64,
    #[serde(rename = "nodeId")]
    pub node_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttributeModified {
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttributeRemoved {
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CharacterDataModified {
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    #[serde(rename = "characterData")]
    pub character_data: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetChildNodes {
    #[serde(rename = "parentId")]
    pub parent_id: i64,
    pub nodes: Vec<DomNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChildNodeCountUpdated {
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    #[serde(rename = "childNodeCount")]
    pub child_node_count: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InlineStyleInvalidated {
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShadowRootEvent {
    #[serde(rename = "hostId")]
    pub host_id: i64,
    pub root: DomNode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PseudoElementEvent {
    #[serde(rename = "parentId")]
    pub parent_id: i64,
    #[serde(rename = "pseudoElement")]
    pub pseudo_element: DomNode,
}

// ── Page Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct LifecycleEvent {
    #[serde(rename = "frameId")]
    pub frame_id: String,
    #[serde(rename = "loaderId")]
    pub loader_id: Option<String>,
    pub name: String,
    pub timestamp: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FrameNavigated {
    pub frame: FrameInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FrameInfo {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    #[serde(rename = "loaderId")]
    pub loader_id: Option<String>,
    pub url: String,
    #[serde(rename = "securityOrigin")]
    pub security_origin: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScreencastFrame {
    pub data: String, // base64 encoded
    pub metadata: ScreencastMetadata,
    #[serde(rename = "sessionId")]
    pub session_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScreencastMetadata {
    #[serde(rename = "offsetTop")]
    pub offset_top: f64,
    #[serde(rename = "pageScaleFactor")]
    pub page_scale_factor: f64,
    #[serde(rename = "deviceWidth")]
    pub device_width: f64,
    #[serde(rename = "deviceHeight")]
    pub device_height: f64,
    #[serde(rename = "scrollOffsetX")]
    pub scroll_offset_x: f64,
    #[serde(rename = "scrollOffsetY")]
    pub scroll_offset_y: f64,
    pub timestamp: Option<f64>,
}

// ── Network Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct RequestWillBeSent {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "loaderId")]
    pub loader_id: Option<String>,
    pub request: RequestData,
    pub timestamp: f64,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequestData {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseReceived {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub response: ResponseData,
    pub timestamp: f64,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseData {
    pub url: String,
    pub status: i32,
    #[serde(rename = "statusText")]
    pub status_text: String,
    pub headers: HashMap<String, String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadingFinished {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub timestamp: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadingFailed {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub timestamp: f64,
    #[serde(rename = "errorText")]
    pub error_text: String,
    pub canceled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketCreated {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketClosed {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub timestamp: f64,
}

// ── Runtime Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct ConsoleApiCalled {
    #[serde(rename = "type")]
    pub call_type: String,
    pub args: Vec<RemoteObject>,
    pub timestamp: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteObject {
    #[serde(rename = "type")]
    pub object_type: String,
    pub value: Option<serde_json::Value>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExceptionThrown {
    pub timestamp: f64,
    #[serde(rename = "exceptionDetails")]
    pub exception_details: ExceptionDetails,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExceptionDetails {
    #[serde(rename = "exceptionId")]
    pub exception_id: i64,
    pub text: String,
    #[serde(rename = "lineNumber")]
    pub line_number: i32,
    #[serde(rename = "columnNumber")]
    pub column_number: i32,
    pub url: Option<String>,
    pub exception: Option<RemoteObject>,
}

// ── Target Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct AttachedToTarget {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "targetInfo")]
    pub target_info: TargetInfo,
    #[serde(rename = "waitingForDebugger")]
    pub waiting_for_debugger: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetachedFromTarget {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TargetInfo {
    #[serde(rename = "targetId")]
    pub target_id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub attached: Option<bool>,
}

// ── Animation Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct AnimationCreated {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnimationStarted {
    pub animation: AnimationData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnimationData {
    pub id: String,
    pub name: String,
    #[serde(rename = "pausedState")]
    pub paused_state: bool,
    #[serde(rename = "playState")]
    pub play_state: String,
    #[serde(rename = "playbackRate")]
    pub playback_rate: f64,
    #[serde(rename = "startTime")]
    pub start_time: f64,
    #[serde(rename = "currentTime")]
    pub current_time: f64,
    #[serde(rename = "type")]
    pub animation_type: String,
    pub source: Option<AnimationEffect>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnimationEffect {
    pub delay: f64,
    #[serde(rename = "endDelay")]
    pub end_delay: f64,
    #[serde(rename = "iterationStart")]
    pub iteration_start: f64,
    pub iterations: Option<f64>,
    pub duration: f64,
    pub direction: String,
    pub fill: String,
    #[serde(rename = "backendNodeId")]
    pub backend_node_id: Option<i64>,
    pub easing: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnimationCanceled {
    pub id: String,
}

// ── Performance Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct PerformanceMetrics {
    pub metrics: Vec<Metric>,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
}

// ── Performance Timeline Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct TimelineEventAdded {
    pub event: TimelineEvent,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimelineEvent {
    #[serde(rename = "frameId")]
    pub frame_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub name: String,
    pub time: f64,
    pub duration: Option<f64>,
    #[serde(rename = "lcpDetails")]
    pub lcp_details: Option<serde_json::Value>,
    #[serde(rename = "layoutShiftDetails")]
    pub layout_shift_details: Option<LayoutShiftDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutShiftDetails {
    pub value: f64,
    #[serde(rename = "hadRecentInput")]
    pub had_recent_input: bool,
    #[serde(rename = "lastInputTime")]
    pub last_input_time: f64,
    pub sources: Option<serde_json::Value>, // Can be array or map depending on Chrome version
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutShiftSource {
    #[serde(rename = "nodeId")]
    pub node_id: Option<i64>,
    #[serde(rename = "previousRect")]
    pub previous_rect: Vec<f64>,
    #[serde(rename = "currentRect")]
    pub current_rect: Vec<f64>,
}

// ── Log Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct LogEntry {
    pub source: String,
    pub level: String,
    pub text: String,
    pub timestamp: f64,
    pub url: Option<String>,
    #[serde(rename = "lineNumber")]
    pub line_number: Option<i32>,
}

// ── Accessibility Events ──

#[derive(Debug, Clone, Deserialize)]
pub struct AxNodesUpdated {
    pub nodes: Vec<AxNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AxNode {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub ignored: Option<bool>,
    pub role: Option<AxValue>,
    pub name: Option<AxValue>,
    pub description: Option<AxValue>,
    pub value: Option<AxValue>,
    pub properties: Option<Vec<AxProperty>>,
    #[serde(rename = "childIds")]
    pub child_ids: Option<Vec<String>>,
    #[serde(rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    #[serde(rename = "frameId")]
    pub frame_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AxValue {
    #[serde(rename = "type")]
    pub value_type: String,
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AxProperty {
    pub name: String,
    pub value: AxValue,
}
