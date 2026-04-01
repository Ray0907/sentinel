//! Event timeline with retention policy and action markers.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct Timeline<T> {
    events: VecDeque<T>,
    max_events: usize,
    max_age: Duration,
    /// Index into events where the last action started
    last_action_index: Option<usize>,
    total_pushed: u64,
}

impl<T> Timeline<T> {
    pub fn new(max_events: usize, max_age: Duration) -> Self {
        Self {
            events: VecDeque::with_capacity(max_events.min(1024)),
            max_events,
            max_age,
            last_action_index: None,
            total_pushed: 0,
        }
    }

    pub fn push(&mut self, event: T) {
        self.events.push_back(event);
        self.total_pushed += 1;

        // Enforce max_events
        while self.events.len() > self.max_events {
            self.events.pop_front();
            // Adjust action index
            if let Some(ref mut idx) = self.last_action_index {
                if *idx > 0 {
                    *idx -= 1;
                } else {
                    self.last_action_index = None;
                }
            }
        }
    }

    /// Mark the current position as an action start.
    pub fn mark_action_start(&mut self) {
        self.last_action_index = Some(self.events.len());
    }

    /// Get all events since the last action marker.
    pub fn events_since_last_action(&self) -> Vec<&T> {
        let start = self.last_action_index.unwrap_or(0);
        self.events.iter().skip(start).collect()
    }

    /// Get total event count (including evicted).
    pub fn total_count(&self) -> u64 {
        self.total_pushed
    }

    /// Get current buffer size.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Check if the timeline is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Garbage collect old events based on max_age.
    pub fn gc<F>(&mut self, get_timestamp: F)
    where
        F: Fn(&T) -> Instant,
    {
        let cutoff = Instant::now() - self.max_age;
        while let Some(front) = self.events.front() {
            if get_timestamp(front) < cutoff {
                self.events.pop_front();
                if let Some(ref mut idx) = self.last_action_index {
                    if *idx > 0 {
                        *idx -= 1;
                    } else {
                        self.last_action_index = None;
                    }
                }
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestEvent {
        ts: Instant,
        value: i32,
    }

    impl TestEvent {
        fn new(value: i32) -> Self {
            Self {
                ts: Instant::now(),
                value,
            }
        }

        fn with_time(value: i32, ts: Instant) -> Self {
            Self { ts, value }
        }
    }

    #[test]
    fn new_timeline_is_empty() {
        let t: Timeline<TestEvent> = Timeline::new(100, Duration::from_secs(60));
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert_eq!(t.total_count(), 0);
    }

    #[test]
    fn push_increments_counts() {
        let mut t = Timeline::new(100, Duration::from_secs(60));
        t.push(TestEvent::new(1));
        t.push(TestEvent::new(2));
        t.push(TestEvent::new(3));

        assert_eq!(t.len(), 3);
        assert_eq!(t.total_count(), 3);
        assert!(!t.is_empty());
    }

    #[test]
    fn push_respects_max_events() {
        let mut t = Timeline::new(3, Duration::from_secs(60));
        for i in 0..10 {
            t.push(TestEvent::new(i));
        }

        // Buffer should only hold max_events
        assert_eq!(t.len(), 3);
        // Total pushed should reflect all pushes
        assert_eq!(t.total_count(), 10);
    }

    #[test]
    fn eviction_removes_oldest() {
        let mut t = Timeline::new(3, Duration::from_secs(60));
        t.push(TestEvent::new(1));
        t.push(TestEvent::new(2));
        t.push(TestEvent::new(3));
        t.push(TestEvent::new(4)); // evicts 1

        let all: Vec<&TestEvent> = t.events_since_last_action();
        let values: Vec<i32> = all.iter().map(|e| e.value).collect();
        assert_eq!(values, vec![2, 3, 4]);
    }

    #[test]
    fn mark_action_start_and_events_since() {
        let mut t = Timeline::new(100, Duration::from_secs(60));
        t.push(TestEvent::new(1));
        t.push(TestEvent::new(2));

        t.mark_action_start();

        t.push(TestEvent::new(3));
        t.push(TestEvent::new(4));

        let since = t.events_since_last_action();
        let values: Vec<i32> = since.iter().map(|e| e.value).collect();
        assert_eq!(values, vec![3, 4]);
    }

    #[test]
    fn events_since_last_action_without_marker_returns_all() {
        let mut t = Timeline::new(100, Duration::from_secs(60));
        t.push(TestEvent::new(1));
        t.push(TestEvent::new(2));

        let since = t.events_since_last_action();
        assert_eq!(since.len(), 2);
    }

    #[test]
    fn action_index_adjusts_on_eviction() {
        let mut t = Timeline::new(3, Duration::from_secs(60));
        t.push(TestEvent::new(1));
        t.mark_action_start(); // at index 1
        t.push(TestEvent::new(2));
        t.push(TestEvent::new(3));
        t.push(TestEvent::new(4)); // evicts 1, adjusts action index

        let since = t.events_since_last_action();
        // Action was marked after event 1, so events 2, 3, 4 are since action.
        // After eviction of event 1, action index adjusts.
        assert!(!since.is_empty());
    }

    #[test]
    fn gc_removes_old_events() {
        let mut t = Timeline::new(100, Duration::from_secs(1)); // 1 second max_age
        let old_time = Instant::now() - Duration::from_secs(10);
        let recent_time = Instant::now();

        t.push(TestEvent::with_time(1, old_time));
        t.push(TestEvent::with_time(2, old_time));
        t.push(TestEvent::with_time(3, recent_time));

        assert_eq!(t.len(), 3);

        t.gc(|e| e.ts);

        // Old events should be removed
        assert_eq!(t.len(), 1);
        let remaining: Vec<&TestEvent> = t.events_since_last_action();
        assert_eq!(remaining[0].value, 3);
    }

    #[test]
    fn gc_adjusts_action_index() {
        let mut t = Timeline::new(100, Duration::from_secs(1));
        let old_time = Instant::now() - Duration::from_secs(10);

        t.push(TestEvent::with_time(1, old_time));
        t.push(TestEvent::with_time(2, old_time));
        t.mark_action_start(); // at index 2
        t.push(TestEvent::new(3));

        t.gc(|e| e.ts);

        // After GC removes 2 old events, action index should adjust
        let since = t.events_since_last_action();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].value, 3);
    }

    #[test]
    fn gc_clears_action_index_if_all_before_it_removed() {
        let mut t = Timeline::new(100, Duration::from_secs(1));
        let old_time = Instant::now() - Duration::from_secs(10);

        t.push(TestEvent::with_time(1, old_time));
        t.mark_action_start(); // at index 1
        t.push(TestEvent::with_time(2, old_time));
        t.push(TestEvent::new(3));

        t.gc(|e| e.ts);

        // Action index was at 1, both events before and at index got removed
        // so action index is now None, returning all remaining events
        let since = t.events_since_last_action();
        assert_eq!(since.len(), 1);
    }

    #[test]
    fn multiple_action_markers() {
        let mut t = Timeline::new(100, Duration::from_secs(60));
        t.push(TestEvent::new(1));
        t.mark_action_start();
        t.push(TestEvent::new(2));
        t.mark_action_start();
        t.push(TestEvent::new(3));

        let since = t.events_since_last_action();
        let values: Vec<i32> = since.iter().map(|e| e.value).collect();
        // Only events after the last mark
        assert_eq!(values, vec![3]);
    }
}
