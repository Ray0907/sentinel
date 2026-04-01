//! Dual-mode stability tracker.
//!
//! Two stability levels:
//! - ActionableQuiet: safe to continue next action (200ms quiet)
//! - FullySettled: safe for full report (1000ms quiet)
//!
//! Tracks: DOM mutations, layout shifts, style changes, network activity,
//! animations, and navigation. Classifies long-lived connections (WebSocket)
//! separately so they don't block stability.

use std::collections::HashSet;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilityState {
    Active,
    ActionableQuiet,
    FullySettled,
    TimedOut,
}

pub struct StabilityTracker {
    // Timestamps of last activity
    last_dom_mutation: Instant,
    last_layout_shift: Instant,
    last_style_change: Instant,
    last_meaningful_network: Instant,
    last_animation_update: Instant,
    last_navigation: Instant,

    // Active navigation tracking
    active_navigations: u32,

    // Long-lived connections (excluded from stability checks)
    long_lived_connections: HashSet<String>,

    // Configuration
    actionable_quiet_ms: u64,
    fully_settled_ms: u64,
    max_wait_ms: u64,

    // Hysteresis
    min_quiet_windows: u32,
    current_quiet_streak: u32,
    last_quiet_check: Instant,

    // Tracking when we started waiting
    wait_start: Option<Instant>,
}

impl Default for StabilityTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl StabilityTracker {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_dom_mutation: now,
            last_layout_shift: now,
            last_style_change: now,
            last_meaningful_network: now,
            last_animation_update: now,
            last_navigation: now,
            active_navigations: 0,
            long_lived_connections: HashSet::new(),
            actionable_quiet_ms: 200,
            fully_settled_ms: 1000,
            max_wait_ms: 10000,
            min_quiet_windows: 2,
            current_quiet_streak: 0,
            last_quiet_check: now,
            wait_start: None,
        }
    }

    /// Reset stability state for a new action (H3 fix).
    pub fn begin_action(&mut self, now: Instant) {
        self.wait_start = Some(now);
        self.current_quiet_streak = 0;
        self.last_quiet_check = now;
    }

    /// Check the current stability state.
    pub fn check(&mut self, pending_requests: usize, active_animations: u32) -> StabilityState {
        let now = Instant::now();

        // Don't auto-start wait timer — it must be set by begin_action() or on_navigation()
        if self.wait_start.is_none() {
            return StabilityState::Active;
        }

        // Check max timeout first
        if let Some(start) = self.wait_start {
            if now.duration_since(start) > Duration::from_millis(self.max_wait_ms) {
                self.wait_start = None;
                self.current_quiet_streak = 0;
                return StabilityState::TimedOut;
            }
        }

        // Check if there's active work
        if self.active_navigations > 0 || active_animations > 0 {
            self.current_quiet_streak = 0;
            return StabilityState::Active;
        }

        // Check if there are pending meaningful network requests
        // (long-lived connections are excluded)
        if pending_requests > 0 {
            self.current_quiet_streak = 0;
            return StabilityState::Active;
        }

        let actionable_threshold = Duration::from_millis(self.actionable_quiet_ms);
        let settled_threshold = Duration::from_millis(self.fully_settled_ms);

        let since_dom = now.duration_since(self.last_dom_mutation);
        let since_layout = now.duration_since(self.last_layout_shift);
        let since_style = now.duration_since(self.last_style_change);
        let since_network = now.duration_since(self.last_meaningful_network);
        let since_animation = now.duration_since(self.last_animation_update);

        let min_quiet = since_dom
            .min(since_layout)
            .min(since_style)
            .min(since_network)
            .min(since_animation);

        // Hysteresis: require consecutive quiet windows
        if now.duration_since(self.last_quiet_check) >= Duration::from_millis(100) {
            if min_quiet >= actionable_threshold {
                self.current_quiet_streak += 1;
            } else {
                self.current_quiet_streak = 0;
            }
            self.last_quiet_check = now;
        }

        if self.current_quiet_streak < self.min_quiet_windows {
            return StabilityState::Active;
        }

        if min_quiet >= settled_threshold {
            self.wait_start = None;
            self.current_quiet_streak = 0;
            return StabilityState::FullySettled;
        }

        if min_quiet >= actionable_threshold {
            return StabilityState::ActionableQuiet;
        }

        StabilityState::Active
    }

    // ── Event handlers ──

    pub fn on_dom_mutation(&mut self, now: Instant) {
        self.last_dom_mutation = now;
    }

    pub fn on_layout_shift(&mut self, now: Instant) {
        self.last_layout_shift = now;
    }

    pub fn on_style_change(&mut self, now: Instant) {
        self.last_style_change = now;
    }

    pub fn on_network_activity(&mut self, now: Instant) {
        self.last_meaningful_network = now;
    }

    pub fn on_network_complete(&mut self, now: Instant) {
        // Codex fix: update timer on completion so quiet window starts from actual idle
        self.last_meaningful_network = now;
    }

    pub fn on_animation_start(&mut self, now: Instant) {
        self.last_animation_update = now;
    }

    pub fn on_animation_end(&mut self, now: Instant) {
        self.last_animation_update = now;
    }

    pub fn on_navigation(&mut self, now: Instant) {
        self.last_navigation = now;
        self.active_navigations += 1;
        self.wait_start = Some(now);
    }

    pub fn on_navigation_end(&mut self, _now: Instant) {
        self.active_navigations = self.active_navigations.saturating_sub(1);
    }

    pub fn on_lifecycle(&mut self, name: &str, now: Instant) {
        // H4 fix: don't decrement active_navigations here — FrameStoppedLoading handles it
        if name == "DOMContentLoaded" {
            self.last_dom_mutation = now;
        }
    }

    pub fn on_long_lived_connection(&mut self, request_id: &str) {
        self.long_lived_connections.insert(request_id.to_string());
    }

    pub fn on_long_lived_disconnection(&mut self, request_id: &str) {
        self.long_lived_connections.remove(request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: advance an Instant by a given duration.
    /// Because Instant::now() is monotonic and we can't construct arbitrary instants,
    /// we use sleep-free time manipulation by setting last-activity timestamps in the past.
    fn make_tracker_with_quiet(actionable_ms: u64, settled_ms: u64, max_ms: u64) -> StabilityTracker {
        let mut t = StabilityTracker::new();
        t.actionable_quiet_ms = actionable_ms;
        t.fully_settled_ms = settled_ms;
        t.max_wait_ms = max_ms;
        t
    }

    #[test]
    fn begin_action_then_check_returns_active() {
        let mut tracker = StabilityTracker::new();
        let now = Instant::now();
        tracker.begin_action(now);
        // Immediately after begin_action, should be Active (quiet streak is 0)
        let state = tracker.check(0, 0);
        assert_eq!(state, StabilityState::Active);
    }

    #[test]
    fn returns_active_when_no_action_started() {
        let mut tracker = StabilityTracker::new();
        // No begin_action called, wait_start is None
        let state = tracker.check(0, 0);
        assert_eq!(state, StabilityState::Active);
    }

    #[test]
    fn returns_active_with_pending_requests() {
        let mut tracker = StabilityTracker::new();
        tracker.begin_action(Instant::now());
        // Even after time passes, pending requests keep state Active
        let state = tracker.check(3, 0);
        assert_eq!(state, StabilityState::Active);
    }

    #[test]
    fn returns_active_with_active_animations() {
        let mut tracker = StabilityTracker::new();
        tracker.begin_action(Instant::now());
        // Animations keep state Active
        let state = tracker.check(0, 2);
        assert_eq!(state, StabilityState::Active);
    }

    #[test]
    fn returns_active_during_navigation() {
        let mut tracker = StabilityTracker::new();
        let now = Instant::now();
        tracker.on_navigation(now);
        // active_navigations > 0 -> Active
        let state = tracker.check(0, 0);
        assert_eq!(state, StabilityState::Active);
    }

    #[test]
    fn actionable_quiet_after_sufficient_wait() {
        // Use very short thresholds so we can actually wait
        let mut tracker = make_tracker_with_quiet(5, 5000, 60000);
        let past = Instant::now() - Duration::from_millis(500);

        // Set all activity timestamps far in the past
        tracker.last_dom_mutation = past;
        tracker.last_layout_shift = past;
        tracker.last_style_change = past;
        tracker.last_meaningful_network = past;
        tracker.last_animation_update = past;

        // Start action far in the past so we don't time out
        tracker.wait_start = Some(past);
        tracker.last_quiet_check = past;

        // First check builds quiet streak
        let s1 = tracker.check(0, 0);
        // Need min_quiet_windows (2) consecutive quiet checks
        // Force the last_quiet_check backward so next check registers as a new window
        tracker.last_quiet_check = Instant::now() - Duration::from_millis(200);
        let s2 = tracker.check(0, 0);

        // After 2 quiet windows with min_quiet >= actionable threshold, should be ActionableQuiet
        assert!(
            s2 == StabilityState::ActionableQuiet || s2 == StabilityState::FullySettled,
            "Expected ActionableQuiet or FullySettled, got {s2:?} (s1={s1:?})"
        );
    }

    #[test]
    fn fully_settled_after_long_quiet() {
        let mut tracker = make_tracker_with_quiet(5, 50, 60000);
        let past = Instant::now() - Duration::from_millis(500);

        tracker.last_dom_mutation = past;
        tracker.last_layout_shift = past;
        tracker.last_style_change = past;
        tracker.last_meaningful_network = past;
        tracker.last_animation_update = past;
        tracker.wait_start = Some(past);
        tracker.last_quiet_check = past;

        // Build up quiet streak
        let _ = tracker.check(0, 0);
        tracker.last_quiet_check = Instant::now() - Duration::from_millis(200);
        let state = tracker.check(0, 0);

        assert_eq!(state, StabilityState::FullySettled);
    }

    #[test]
    fn timed_out_after_max_wait() {
        let mut tracker = make_tracker_with_quiet(5000, 10000, 10);
        // Start action 50ms ago — well past the 10ms max_wait
        let past = Instant::now() - Duration::from_millis(50);
        tracker.wait_start = Some(past);
        // Keep activity timestamps fresh so it wouldn't settle naturally
        tracker.last_dom_mutation = Instant::now();

        let state = tracker.check(0, 0);
        assert_eq!(state, StabilityState::TimedOut);
    }

    #[test]
    fn dom_mutation_resets_quiet_streak() {
        let mut tracker = make_tracker_with_quiet(5, 50, 60000);
        let past = Instant::now() - Duration::from_millis(500);

        tracker.last_dom_mutation = past;
        tracker.last_layout_shift = past;
        tracker.last_style_change = past;
        tracker.last_meaningful_network = past;
        tracker.last_animation_update = past;
        tracker.wait_start = Some(past);
        tracker.last_quiet_check = past;

        // Build streak
        let _ = tracker.check(0, 0);

        // New DOM mutation resets the quiet window
        tracker.on_dom_mutation(Instant::now());
        tracker.last_quiet_check = Instant::now() - Duration::from_millis(200);
        let state = tracker.check(0, 0);

        // Should be Active because the DOM mutation was recent
        assert_eq!(state, StabilityState::Active);
    }

    #[test]
    fn navigation_increments_and_decrements() {
        let mut tracker = StabilityTracker::new();
        let now = Instant::now();

        tracker.on_navigation(now);
        assert_eq!(tracker.active_navigations, 1);

        tracker.on_navigation(now);
        assert_eq!(tracker.active_navigations, 2);

        tracker.on_navigation_end(now);
        assert_eq!(tracker.active_navigations, 1);

        tracker.on_navigation_end(now);
        assert_eq!(tracker.active_navigations, 0);

        // Saturating sub: doesn't go below 0
        tracker.on_navigation_end(now);
        assert_eq!(tracker.active_navigations, 0);
    }

    #[test]
    fn long_lived_connections_tracked() {
        let mut tracker = StabilityTracker::new();
        tracker.on_long_lived_connection("ws-1");
        tracker.on_long_lived_connection("ws-2");
        assert_eq!(tracker.long_lived_connections.len(), 2);

        tracker.on_long_lived_disconnection("ws-1");
        assert_eq!(tracker.long_lived_connections.len(), 1);
        assert!(tracker.long_lived_connections.contains("ws-2"));
    }
}
