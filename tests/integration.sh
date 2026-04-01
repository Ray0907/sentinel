#!/bin/bash
# Sentinel Integration Test Suite
# Tests all 11 CLI commands end-to-end with Chrome
set -e

SENTINEL="./target/release/sentinel"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
TEST_PAGE="file://$(pwd)/test-page.html"
PASS=0
FAIL=0

green() { printf "\033[32m%s\033[0m\n" "$1"; }
red() { printf "\033[31m%s\033[0m\n" "$1"; }
bold() { printf "\033[1m%s\033[0m\n" "$1"; }

assert_contains() {
    local output="$1"
    local expected="$2"
    local test_name="$3"
    if echo "$output" | grep -q "$expected"; then
        green "  PASS: $test_name"
        PASS=$((PASS + 1))
    else
        red "  FAIL: $test_name (expected '$expected')"
        FAIL=$((FAIL + 1))
    fi
}

cleanup() {
    pkill -f "sentinel-chrome" 2>/dev/null || true
    pkill -9 -f "remote-debugging-port=9222" 2>/dev/null || true
    pkill -9 -f "Google Chrome.*headless" 2>/dev/null || true
    rm -f /var/folders/*/T/sentinel-9222.sock 2>/dev/null || true
    rm -f /tmp/sentinel-9222.sock 2>/dev/null || true
    sleep 3
}

bold "=== Sentinel Integration Tests ==="
echo ""

# Build first
bold "Building..."
cargo build --release 2>&1 | tail -1
echo ""

# ── Test 1: navigate ──
bold "Test 1: navigate"
cleanup
OUTPUT=$(RUST_LOG=warn $SENTINEL --chrome "$CHROME" navigate "https://example.com" 2>&1 | grep -v "^\[2m" || true)
assert_contains "$OUTPUT" "FullySettled" "navigate returns FullySettled"
assert_contains "$OUTPUT" "example.com" "navigate tracks URL"
assert_contains "$OUTPUT" "network_requests" "navigate has network_requests"

# ── Test 2: run (navigate + click) ──
bold "Test 2: run --click"
cleanup
OUTPUT=$(RUST_LOG=warn $SENTINEL --chrome "$CHROME" run --url "$TEST_PAGE" --click "#btn-simple" --duration 1 2>&1 | grep -v "^\[2m" || true)
assert_contains "$OUTPUT" "FullySettled" "run returns FullySettled"
assert_contains "$OUTPUT" "btn-simple" "run tracks click selector"
assert_contains "$OUTPUT" "dom_mutations" "run has dom_mutations"

# ── Test 3: watch ──
bold "Test 3: watch"
cleanup
OUTPUT=$(RUST_LOG=error $SENTINEL --chrome "$CHROME" watch "https://example.com" -d 5 -f lifecycle 2>/dev/null || true)
assert_contains "$OUTPUT" "lifecycle" "watch streams lifecycle events"
assert_contains "$OUTPUT" "DOMContentLoaded" "watch captures DOMContentLoaded"
assert_contains "$OUTPUT" "networkIdle" "watch captures networkIdle"

# ── Test 4: record ──
bold "Test 4: record"
cleanup
OUTPUT=$(RUST_LOG=warn $SENTINEL --chrome "$CHROME" record "$TEST_PAGE" -o /tmp/sentinel-test-recording.json -d 3 2>&1 | grep -v "^\[2m" || true)
assert_contains "$OUTPUT" "Recording saved" "record saves file"
assert_contains "$OUTPUT" "Total events" "record prints summary"

# ── Test 5: replay --summary ──
bold "Test 5: replay --summary"
OUTPUT=$($SENTINEL replay /tmp/sentinel-test-recording.json --summary 2>&1)
assert_contains "$OUTPUT" "Sentinel Recording" "replay shows header"
assert_contains "$OUTPUT" "Total events" "replay shows event count"
assert_contains "$OUTPUT" "DOM mutations" "replay shows DOM mutations"

# ── Test 6: replay (full timeline) ──
bold "Test 6: replay (full timeline)"
OUTPUT=$($SENTINEL replay /tmp/sentinel-test-recording.json 2>&1)
assert_contains "$OUTPUT" "Full Timeline" "replay shows timeline"
assert_contains "$OUTPUT" "lifecycle" "replay has lifecycle events"

# ── Test 7: visual diff ──
bold "Test 7: visual diff in report"
cleanup
OUTPUT=$(RUST_LOG=warn $SENTINEL --chrome "$CHROME" run --url "$TEST_PAGE" --click "#btn-async" --duration 1 2>&1 | grep -v "^\[2m" || true)
assert_contains "$OUTPUT" "visual_diff" "report includes visual_diff"
assert_contains "$OUTPUT" "hash_distance" "visual_diff has hash_distance"
assert_contains "$OUTPUT" "changed_regions" "visual_diff has changed_regions"
assert_contains "$OUTPUT" "layout_shifts" "report includes layout_shifts"

# ── Test 8: action error reporting ──
bold "Test 8: action error reporting"
cleanup
OUTPUT=$(RUST_LOG=warn $SENTINEL --chrome "$CHROME" run --url "https://example.com" --click "#nonexistent" --duration 1 2>&1 | grep -v "^\[2m" || true)
assert_contains "$OUTPUT" "action_error" "error click reports action_error"
assert_contains "$OUTPUT" "Element not found" "error has Element not found message"

# ── Test 9: daemon start/send/stop ──
bold "Test 9: daemon mode"
cleanup
sleep 2
RUST_LOG=info $SENTINEL --chrome "$CHROME" daemon start &
DAEMON_PID=$!
sleep 15

# daemon status
OUTPUT=$($SENTINEL daemon status 2>&1)
assert_contains "$OUTPUT" "running" "daemon status shows running"

# send ping
OUTPUT=$($SENTINEL send ping 2>&1)
assert_contains "$OUTPUT" "pong" "send ping returns pong"

# send navigate
OUTPUT=$($SENTINEL send navigate --url "https://example.com" 2>&1)
assert_contains "$OUTPUT" "FullySettled" "send navigate returns report"

# daemon stop
OUTPUT=$($SENTINEL daemon stop 2>&1)
assert_contains "$OUTPUT" "Shutting down" "daemon stop works"
wait $DAEMON_PID 2>/dev/null || true

# ── Test 10: help ──
bold "Test 10: help"
OUTPUT=$($SENTINEL --help 2>&1)
assert_contains "$OUTPUT" "navigate" "help shows navigate"
assert_contains "$OUTPUT" "watch" "help shows watch"
assert_contains "$OUTPUT" "record" "help shows record"
assert_contains "$OUTPUT" "daemon" "help shows daemon"

# ── Cleanup ──
cleanup
rm -f /tmp/sentinel-test-recording.json

# ── Results ──
echo ""
bold "=== Results ==="
TOTAL=$((PASS + FAIL))
green "Passed: $PASS/$TOTAL"
if [ $FAIL -gt 0 ]; then
    red "Failed: $FAIL/$TOTAL"
    exit 1
else
    green "All tests passed!"
fi
