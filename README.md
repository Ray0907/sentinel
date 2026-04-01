# Sentinel

[![CI](https://github.com/Ray0907/sentinel/actions/workflows/ci.yml/badge.svg)](https://github.com/Ray0907/sentinel/actions/workflows/ci.yml)

Web performance monitor for CI pipelines. One binary, one command, zero config.

```bash
sentinel budget "https://your-app.com" -b "CLS<0.1,TTI<3000,errors=0"
# exit 0 = pass, exit 1 = fail
```

## Install

```bash
cargo install --path .
```

Requires Chrome or Chromium installed.

## Usage

### Performance Budget (CI)

Add to your CI pipeline to catch performance regressions before deploy:

```bash
sentinel budget "https://your-app.com" \
  --chrome /path/to/chrome \
  -b "CLS<0.1,TTI<3000,errors=0,requests<50" \
  -d 10
```

Output:
```
=== Performance Budget Check ===

  PASS  errors=0                        actual: 0           budget: 0
  PASS  requests<50                     actual: 7           budget: 50
  PASS  cls<0.1                         actual: 0.0354      budget: 0.1000
! FAIL  tti<3000                        actual: 6860        budget: 3000

1 budget(s) exceeded.
```

Available metrics: `CLS`, `TTI`, `errors`, `requests`, `dom_mutations`, `layout_shifts`, `console_messages`, `animations`, `total_events`

Operators: `<`, `<=`, `=`, `>`

#### GitHub Actions

Use the [Sentinel Action](https://github.com/Ray0907/sentinel-action) — zero setup:

```yaml
- uses: Ray0907/sentinel-action@v1
  with:
    url: "https://your-staging-url.com"
    budget: "CLS<0.1,TTI<5000,errors=0"
```

Or install from source:

```yaml
jobs:
  perf:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install --git https://github.com/Ray0907/sentinel.git
      - run: sentinel budget "https://your-app.com" --chrome google-chrome -b "CLS<0.1,errors=0" -d 10
```

### Watch Mode

Stream page events in real-time as JSONL:

```bash
# All events
sentinel watch "https://your-app.com" -d 30

# Filter by category
sentinel watch "https://your-app.com" -f lifecycle
sentinel watch "https://your-app.com" -f network
sentinel watch "https://your-app.com" -f dom
sentinel watch "https://your-app.com" -f error
```

```json
{"time_ms":163,"category":"lifecycle","detail":"DOMContentLoaded"}
{"time_ms":757,"category":"lifecycle","detail":"firstPaint"}
{"time_ms":2365,"category":"lifecycle","detail":"firstMeaningfulPaint"}
{"time_ms":6860,"category":"lifecycle","detail":"InteractiveTime"}
```

Pipe to `jq`, `grep`, or your monitoring tool.

### Navigate + Interact

```bash
# Navigate and get observation report
sentinel navigate "https://your-app.com"

# Navigate, click, observe the result
sentinel run --url "https://your-app.com" --click "#submit-btn" --duration 5
```

Every action produces a JSON report with:
- DOM mutations (what changed)
- Layout shifts (CLS values)
- Network requests (URLs + status codes)
- JS errors and console messages
- Visual diff (which screen region changed, by how much)
- Time to stable (how long until the page settled)

### Record + Replay

```bash
# Record a full session
sentinel record "https://your-app.com" -o session.json -d 10

# Replay offline (no Chrome needed)
sentinel replay session.json --summary
sentinel replay session.json  # full timeline
```

```
=== Sentinel Recording ===
URL:        https://news.ycombinator.com
Duration:   3850ms

--- Event Summary ---
Total events:      42
DOM mutations:     2
Network requests:  21
Layout shifts:     0 (CLS: 0.0000)
Errors:            0
Time to Interactive: 3850ms
```

### Daemon Mode

Keep Chrome open between commands:

```bash
sentinel daemon start
sentinel send navigate --url "https://your-app.com"
sentinel send click --selector "#button"
sentinel daemon stop
```

## How It Works

Sentinel opens a headless Chrome, connects via Chrome DevTools Protocol (CDP),
and subscribes to 40+ event streams: DOM mutations, network requests, layout
shifts, animations, console output, lifecycle events. Instead of taking
snapshots, it continuously records everything that happens.

When you perform an action (navigate, click), Sentinel automatically detects
when the page has stabilized (no more DOM changes, network idle, animations
finished) and produces a structured report.

```
Chrome ─── CDP WebSocket ──→ Sensor (event normalization)
                                    │
                              Page Actor (single-thread state owner)
                                    │
                         ┌──────────┼──────────┐
                         │          │          │
                     Timeline   Stability   Visual Diff
                         │       Tracker
                         ▼
                  Report / Stream / Recording
```

## Build

```bash
cargo build --release   # single binary, ~10MB
cargo test              # 58 unit tests
```

## License

MIT
