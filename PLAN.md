# Sentinel: Continuous-Observation Agent Browser Framework

> v2 — incorporates Codex review feedback (2026-04-01)

## Problem Statement

Current agent-browser frameworks operate in a **snapshot-act-snapshot** loop:
1. Take accessibility tree snapshot → get element refs
2. Perform action (click, type, navigate)
3. Take another snapshot → compare diffs

This misses:
- **Intermediate states**: Button hover effects, loading spinners, dropdown animations
- **Layout shifts**: Elements being pushed down during render (CLS)
- **Timing-dependent bugs**: Race conditions between JS hydration and user interaction
- **Visual regressions**: Subpixel shifts, font rendering changes, z-index overlaps
- **State transitions**: The "journey" between two states is invisible

A real human tester sees ALL of this continuously. Sentinel replicates that.

## v1 Scope (from Codex review)

**Target**: reliable event timeline + tracked-node pull refresh + action-scoped stability

**NOT in v1**: full live DOM/style/layout mirror, prediction engine, pretext-style text measurement

## Architecture: Single Page Actor

### Core Insight (revised)

Instead of polling snapshots, **subscribe to CDP event streams** and build a
real-time event timeline. Use a **single page actor** that owns all mutable state
(no DashMap/crossbeam races). Sensor normalizes events, Cortex owns the model,
Actuator sends intents — all communicate via `tokio::mpsc`.

```
┌──────────────────────────────────────────────────────┐
│                     Sentinel                          │
│                                                       │
│  ┌──────────┐    ┌────────────────┐   ┌──────────┐   │
│  │  Sensor  │───▶│   Page Actor   │◀──│ Actuator │   │
│  │(normalize│    │  (owns model)  │   │ (intents)│   │
│  │  events) │    │                │   │          │   │
│  └──────────┘    │  ┌──────────┐  │   └──────────┘   │
│       ▲          │  │ DOM Tree │  │        │          │
│       │          │  │ Timeline │  │        │          │
│       │          │  │ Stability│  │        ▼          │
│       │          │  └──────────┘  │   ┌──────────┐   │
│       │          └────────────────┘   │   CDP    │   │
│       │                 │            │ Commands │   │
│       │                 ▼            └──────────┘   │
│       │          ┌──────────┐              │          │
│       │          │  Query   │              │          │
│       │          │   API    │              │          │
│       │          └──────────┘              │          │
│       │                                    │          │
│  ═══CDP WebSocket (events)═══════(commands)═══       │
│       ▲                                    │          │
│       │                                    ▼          │
│  ┌──────────────────────────────────────────┐        │
│  │              Browser (Chrome)             │        │
│  │  Target.setAutoAttach(flatten=true)        │        │
│  └──────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────┘
```

### CDP Event Coverage (complete)

#### DOM Events
- `DOM.documentUpdated` — full document refresh (triggers epoch bump)
- `DOM.setChildNodes` — batch child node delivery (CRITICAL for tree building)
- `DOM.childNodeInserted` / `DOM.childNodeRemoved` — incremental updates
- `DOM.childNodeCountUpdated` — child count change notification
- `DOM.attributeModified` / `DOM.attributeRemoved` — attribute changes
- `DOM.characterDataModified` — text content changes
- `DOM.shadowRootPushed` / `DOM.shadowRootPopped` — shadow DOM
- `DOM.pseudoElementAdded` / `DOM.pseudoElementRemoved` — pseudo elements
- `DOM.inlineStyleInvalidated` — inline style changes
- `DOM.distributedNodesUpdated` — slot distribution changes
- `DOM.topLayerElementsUpdated` — dialog/popover layer changes

#### CSS Events
- `CSS.styleSheetAdded` / `CSS.styleSheetChanged` / `CSS.styleSheetRemoved`
- `CSS.fontsUpdated` — font loading
- **Note**: CSS events alone cannot maintain a live style_map; use pull-based
  `CSS.getComputedStyleForNode` for tracked nodes after mutations

#### Target/Context Events (OOPIF + worker support)
- `Target.setAutoAttach(flatten=true)` — auto-attach all child targets
- `Target.attachedToTarget` / `Target.detachedFromTarget`
- `Runtime.executionContextCreated` / `Runtime.executionContextDestroyed`
- `Runtime.bindingCalled` — for injected observer callbacks

#### Page/Navigation Events
- `Page.lifecycleEvent` — DOMContentLoaded, load, networkIdle
- `Page.frameNavigated` / `Page.frameStartedLoading` / `Page.frameStoppedLoading`
- `Page.navigatedWithinDocument` — SPA navigation
- `Page.frameResized`
- `Page.screencastFrame` — continuous viewport streaming

#### Network Events
- `Network.requestWillBeSent` / `Network.responseReceived`
- `Network.loadingFinished` / `Network.loadingFailed`
- `Network.requestServedFromCache`
- `Network.requestWillBeSentExtraInfo` / `Network.responseReceivedExtraInfo`
- `Network.webSocketCreated` / `Network.webSocketFrameSent` / `Network.webSocketFrameReceived`
- `Network.webSocketClosed`

#### Performance/Animation Events
- `Performance.metrics` — layout count, recalc style count
- `PerformanceTimeline.timelineEventAdded` — layout-shift entries (preferred over injected observer)
- `Animation.animationStarted` / `Animation.animationUpdated` / `Animation.animationCanceled`

#### Accessibility Events
- `Accessibility.loadComplete`
- `Accessibility.nodesUpdated` — incremental AX tree updates

#### Console/Error Events
- `Runtime.consoleAPICalled` — console output
- `Runtime.exceptionThrown` — uncaught errors
- `Log.entryAdded` — browser-level log entries

### Page Actor Model

Single actor owns all mutable state. No shared mutable state = no races.

```rust
/// All page state owned by a single tokio task
struct PageActor {
    // Identity
    page_epoch: u64,  // bumped on documentUpdated / navigation
    session_id: String,

    // Live DOM tree (incrementally updated)
    dom_tree: LiveDomTree,

    // Tracked node geometry (pull-based refresh, not live mirror)
    tracked_nodes: HashMap<BackendNodeId, TrackedNode>,

    // Accessibility overlay (lazy-refreshed)
    ax_tree: Option<AccessibilityTree>,

    // Ring buffer of recent frames (bounded, prevents leaks)
    frame_buffer: RingBuffer<Frame, 60>,

    // Event timeline with retention policy
    timeline: Timeline<ObservationEvent>,

    // Stability tracker (dual-mode)
    stability: StabilityTracker,

    // Pending network requests (classified)
    network: NetworkTracker,

    // Active animations
    animations: HashMap<String, AnimationState>,

    // Child targets (OOPIF, workers)
    child_targets: HashMap<String, TargetInfo>,

    // Channels
    event_rx: mpsc::Receiver<SensorEvent>,
    cmd_rx: mpsc::Receiver<ActuatorCommand>,
    query_tx: mpsc::Sender<QueryResponse>,
}
```

**Node Identity** (from Codex review):
```rust
/// Transport identity for CDP events
#[derive(Hash, Eq, PartialEq, Clone)]
struct NodeKey {
    session_id: String,
    node_id: i64,
}

/// Cross-domain correlation identity
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
struct BackendNodeId(i64);

/// Every event carries an epoch to detect stale events
struct EpochEvent<T> {
    epoch: u64,
    timestamp: Instant,
    payload: T,
}
```

### Stability Detection (dual-mode, from Codex review)

```rust
struct StabilityTracker {
    // Timestamps
    last_dom_mutation: Instant,
    last_layout_shift: Instant,
    last_style_change: Instant,
    last_meaningful_network: Instant, // excludes long-lived connections
    last_animation_update: Instant,

    // Active counts
    pending_requests: HashSet<RequestId>,
    active_animations: u32,
    active_navigations: u32,

    // Network classification
    long_lived_connections: HashSet<RequestId>, // WebSocket, EventSource — excluded

    // Thresholds
    actionable_quiet_ms: u64,  // default 200ms — safe to continue next action
    fully_settled_ms: u64,     // default 1000ms — safe for full report
    max_wait_ms: u64,          // default 10000ms — timeout with partial report

    // Hysteresis: minimum consecutive quiet windows before declaring stable
    min_quiet_windows: u32,    // default 2
    current_quiet_streak: u32,
}

enum StabilityState {
    Active,          // mutations/animations/network in progress
    ActionableQuiet, // quiet enough for next action
    FullySettled,    // fully quiet, safe for report
    TimedOut,        // max_wait exceeded, partial report
}
```

### Timeline with Retention

```rust
struct Timeline<T> {
    events: VecDeque<EpochEvent<T>>,
    max_events: usize,         // default 10_000
    max_age: Duration,         // default 60s
    action_markers: Vec<ActionMarker>, // mark action boundaries
}

impl<T> Timeline<T> {
    fn push(&mut self, event: EpochEvent<T>) {
        self.events.push_back(event);
        self.gc(); // evict old/excess events
    }

    fn events_since_action(&self, action_id: u64) -> impl Iterator<Item = &EpochEvent<T>> {
        // Return all events between action start and now
    }
}
```

### Reconciliation (from Codex review)

After navigation or burst mutations (>100 events in 500ms), trigger a
low-frequency reconciliation using `DOMSnapshot.captureSnapshot` to correct
any drift in the incremental DOM tree.

```rust
impl PageActor {
    async fn maybe_reconcile(&mut self, cdp: &CdpClient) {
        if self.should_reconcile() {
            let snapshot = cdp.call("DOMSnapshot.captureSnapshot", json!({
                "computedStyles": [],
                "includeDOMRects": false,
                "includePaintOrder": false,
            })).await;
            self.dom_tree.reconcile_from_snapshot(snapshot);
            self.page_epoch += 1;
        }
    }
}
```

## Technical Stack

### Rust Crates

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime, `mpsc` channels, timers |
| `tokio-tungstenite` | WebSocket client for CDP |
| `tokio-util` | `CancellationToken` for graceful shutdown |
| `serde` / `serde_json` | CDP protocol serialization |
| `slotmap` | Stable-key arena for DOM tree nodes |
| `image` | Frame buffer and visual diff |
| `similar` | Text diff fallback |
| `img_hash` | Perceptual hash for visual comparison |
| `bytes` | Efficient byte buffer handling |
| `tracing` / `tracing-subscriber` | Structured logging + timeline |
| `clap` | CLI interface |
| `flume` | MPSC alternative (if needed for perf) |

**Removed** (per Codex): `dashmap`, `crossbeam-channel` (unnecessary with actor model)

**Not using** `chromiumoxide` as core — too opinionated. Reference its codegen for types only.

### Module Structure

```
sentinel/
├── Cargo.toml
├── PLAN.md
├── src/
│   ├── main.rs                 # CLI entry + daemon mode
│   ├── lib.rs                  # Library root
│   │
│   ├── cdp/                    # CDP WebSocket client
│   │   ├── mod.rs
│   │   ├── client.rs           # WebSocket + message dispatch + event broadcast
│   │   ├── types.rs            # CDP protocol types (minimal, hand-written for v1)
│   │   └── browser.rs          # Chrome process management
│   │
│   ├── sensor/                 # Sensor Layer (normalize CDP events → SensorEvent)
│   │   ├── mod.rs
│   │   ├── dom.rs              # DOM.* events → DomEvent
│   │   ├── network.rs          # Network.* events → NetworkEvent
│   │   ├── page.rs             # Page.* + Animation.* → PageEvent
│   │   └── console.rs          # Runtime.* + Log.* → ConsoleEvent
│   │
│   ├── actor/                  # Page Actor (owns all mutable state)
│   │   ├── mod.rs              # Actor event loop
│   │   ├── dom_tree.rs         # Live DOM tree (slotmap-backed)
│   │   ├── stability.rs        # Dual-mode stability tracker
│   │   ├── timeline.rs         # Event timeline with retention
│   │   ├── network.rs          # Network request tracker
│   │   └── reconcile.rs        # DOMSnapshot reconciliation
│   │
│   ├── actuator/               # Actuator (send CDP commands, produce reports)
│   │   ├── mod.rs
│   │   ├── input.rs            # Mouse + keyboard via CDP Input domain
│   │   ├── navigation.rs       # Page navigation
│   │   └── report.rs           # ObservationReport generation
│   │
│   ├── query/                  # Query API (read-only access to actor state)
│   │   ├── mod.rs
│   │   └── commands.rs         # CLI command handlers
│   │
│   └── diff/                   # Diff Engine
│       ├── mod.rs
│       ├── dom_diff.rs         # Tree-based semantic diff
│       └── visual_diff.rs      # Frame comparison + perceptual hash
```

## Implementation Plan (P0 only for v1)

### Phase 1: CDP Foundation
1. CDP WebSocket client with async event multiplexing
2. Chrome process launcher with `--remote-debugging-port`
3. CDP event type definitions (DOM, Page, Network, Runtime — hand-written)
4. Event broadcast via `tokio::broadcast`

### Phase 2: Sensor + Actor
5. Sensor layer: subscribe to CDP domains, normalize to `SensorEvent` enum
6. Page Actor: event loop consuming `SensorEvent` via `mpsc`
7. Live DOM tree: `DOM.getDocument(pierce=true)` → `DOM.requestChildNodes` → incremental updates
8. Page epoch: bump on `DOM.documentUpdated`, discard stale events
9. Target auto-attach: `Target.setAutoAttach(flatten=true)` for OOPIF

### Phase 3: Stability + Timeline
10. Dual-mode stability tracker (actionable_quiet / fully_settled)
11. Network tracker with long-lived connection classification
12. Animation tracking via `Animation.*` events
13. Event timeline with retention policy (max 10k events, 60s age)
14. Reconciliation: `DOMSnapshot.captureSnapshot` after navigation/burst

### Phase 4: Actuator + Report
15. Mouse/keyboard input via CDP Input domain
16. Element resolution via DOM.querySelector + backendNodeId
17. Action-observation loop: action → wait for stability → report
18. ObservationReport: DOM changes, network, errors, stability timing

### Phase 5: CLI
19. Basic CLI: `sentinel navigate <url>`, `sentinel observe`, `sentinel click <selector>`
20. Daemon mode with Unix socket
21. JSON output format for AI agent consumption

## Acceptance Criteria (revised)

### AC1: Continuous DOM Observation
- Subscribe to DOM events via CDP
- Maintain a live DOM tree via incremental updates
- Detect insertions/removals/attribute changes within <50ms
- Handle `documentUpdated` correctly (epoch bump, tree rebuild)

### AC2: Layout Shift Detection
- Detect CLS via `PerformanceTimeline.timelineEventAdded`
- Report which elements shifted and by how much
- Flag shifts above 0.1 CLS score

### AC3: Stability Detection (revised)
- Dual-mode: `actionable_quiet` vs `fully_settled`
- Track animations via `Animation.*` events
- Classify network: exclude WebSocket/EventSource from stability check
- Hysteresis: require N consecutive quiet windows
- Max timeout with partial report (never hang forever)

### AC4: Action-Observation Loop
- Click → capture ALL changes → produce ObservationReport
- Report includes: DOM mutations, layout shifts, network, errors, timing
- Automatic (no manual re-snapshot needed)

### AC5: Resilience
- Stale events (wrong epoch) are discarded, not applied
- `DOMSnapshot` reconciliation after navigation or burst mutations
- Timeline has bounded memory (retention policy)
- Tracked nodes use `BackendNodeId` for cross-domain correlation

### AC6: CLI Compatibility
- navigate, click, type, observe commands
- JSON output parseable by AI agents
- Daemon mode for persistent sessions

### AC7: Performance
- Event processing < 10ms latency
- Memory < 100MB for typical pages
- Support 10,000+ DOM nodes
