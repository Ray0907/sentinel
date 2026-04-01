# Sentinel

Continuous-observation agent browser framework. Built in Rust.

**5,180 lines | 54 unit tests | 11 commands | 0 warnings**

## Problem

Agent-browser frameworks operate in a **snapshot-act-snapshot** loop:

```
snapshot → get element refs → click → snapshot again → diff
```

Everything between snapshots is invisible: loading spinners, layout shifts, animation states, intermediate DOM mutations, network timing.

## Solution

Sentinel subscribes to **40+ CDP event streams** and builds a real-time model of the page. It captures **everything that happens** during and after an action.

```
action → continuous event stream → automatic stability detection → structured report
```

## Quick Start

```bash
cargo build --release

# Navigate and observe
sentinel --chrome /path/to/chrome navigate "https://example.com"

# Navigate + click + observe the full journey
sentinel --chrome /path/to/chrome run \
  --url "https://app.example.com" \
  --click "#submit-btn" \
  --duration 5

# Watch in real-time (JSONL stream)
sentinel --chrome /path/to/chrome watch "https://example.com" -d 30

# Filter by event type
sentinel --chrome /path/to/chrome watch "https://example.com" -f lifecycle
sentinel --chrome /path/to/chrome watch "https://example.com" -f network
sentinel --chrome /path/to/chrome watch "https://example.com" -f dom

# Record session for offline replay
sentinel --chrome /path/to/chrome record "https://example.com" -o session.json -d 10

# Replay offline (no Chrome needed)
sentinel replay session.json
sentinel replay session.json --summary

# Daemon mode (persistent Chrome session)
sentinel --chrome /path/to/chrome daemon start
sentinel send navigate --url "https://example.com"
sentinel send click --selector "#button"
sentinel send ping
sentinel daemon stop
```

## Commands

| Command | Description |
|---------|-------------|
| `navigate <url>` | Navigate and produce ObservationReport |
| `run --url <url> --click <sel>` | Multi-step interaction with reports |
| `watch <url> [-f filter]` | Real-time JSONL event stream |
| `record <url> [-o file]` | Record session to JSON for replay |
| `replay <file> [--summary]` | Offline replay/analysis (no Chrome) |
| `daemon start` | Start persistent background session |
| `send <action>` | Send command to running daemon |
| `daemon stop/status` | Manage daemon lifecycle |
| `click/type/snapshot` | Single actions |

## ObservationReport

Every action produces a structured JSON report:

```json
{
  "action": "Click { selector: \"#btn-async\" }",
  "state": "FullySettled",
  "time_to_stable_ms": 2599,
  "dom_mutations": [
    "removed node 77 from parent 76",
    "inserted <#text> into node 76",
    "inline style invalidated on node 75"
  ],
  "layout_shifts": [0.0177, 0.0177],
  "network_requests": ["GET https://api.example.com/data"],
  "errors": [],
  "console_messages": ["[error] 404 Not Found"],
  "visual_diff": {
    "hash_distance": 52,
    "pixel_mismatch_pct": 0.18,
    "changed": true,
    "changed_region_count": 1,
    "changed_regions": [[0, 116, 752, 290]]
  },
  "action_error": null,
  "network_errors": ["https://example.com/favicon.ico: HTTP 404"],
  "total_events": 13
}
```

## Watch Mode (JSONL Streaming)

```bash
sentinel watch "https://news.ycombinator.com" -d 10 -f lifecycle
```

```json
{"time_ms":163,"category":"lifecycle","detail":"commit"}
{"time_ms":163,"category":"lifecycle","detail":"DOMContentLoaded"}
{"time_ms":757,"category":"lifecycle","detail":"firstPaint"}
{"time_ms":757,"category":"lifecycle","detail":"firstContentfulPaint"}
{"time_ms":2365,"category":"lifecycle","detail":"firstMeaningfulPaint"}
{"time_ms":2909,"category":"lifecycle","detail":"networkIdle"}
{"time_ms":6860,"category":"lifecycle","detail":"InteractiveTime"}
```

## Architecture

```
Sensor (40+ CDP events) → Page Actor (single owner) → Actuator (CDP commands)
                                │
                    ┌───────────┼───────────┐
                    │           │           │
                Timeline   Stability    Visual Diff
                    │       Tracker      (screenshots)
                    ▼
            ObservationReport / StreamEvent / Recording
```

- **Sensor**: Subscribes to CDP domains, normalizes events, routes by session
- **Page Actor**: Single tokio task owns all mutable state (zero races)
- **Stability Tracker**: Dual-mode (200ms actionable / 1000ms settled) with hysteresis
- **Visual Diff**: Perceptual hash + pixel diff + region detection
- **DOM Tree**: Incremental updates via slotmap, DOMSnapshot reconciliation
- **Multi-target**: OOPIF/iframe events with domain enablement + target provenance

## Build & Test

```bash
cargo build --release   # ~7s
cargo test              # 54 unit tests
bash tests/integration.sh  # 15 integration tests (requires Chrome)
```

## Comparison

| | Snapshot-based | Sentinel |
|---|---|---|
| Observation | Point-in-time | Continuous event stream |
| DOM model | Rebuilt each time | Incrementally updated |
| Layout shifts | Not detected | CLS via PerformanceTimeline |
| Visual changes | Screenshot pixel diff | Perceptual hash + region detection |
| Timing | Fixed waits | Automatic stability detection |
| Intermediate states | Invisible | Fully captured in timeline |
| Network | Not tracked | Full request lifecycle |
| Animations | Not tracked | Animation.* events |
| Console/errors | Not tracked | Captured with timing |
| Recording | Not available | Full session record + offline replay |

## License

MIT
