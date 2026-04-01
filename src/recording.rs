//! Session recording and replay.
//!
//! Records all StreamEvents during a session to a JSON file.
//! Can replay/analyze recordings offline without Chrome.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::actuator::{ObservationReport, StreamEvent};

/// A complete session recording.
#[derive(Debug, Serialize, Deserialize)]
pub struct Recording {
    pub version: String,
    pub url: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub events: Vec<StreamEvent>,
    pub reports: Vec<ObservationReport>,
    pub summary: RecordingSummary,
}

/// Summary statistics for a recording.
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub total_events: usize,
    pub dom_mutations: usize,
    pub network_requests: usize,
    pub layout_shifts: usize,
    pub errors: usize,
    pub console_messages: usize,
    pub lifecycle_events: usize,
    pub animations: usize,
    pub time_to_interactive_ms: Option<u64>,
    pub total_cls: f64,
}

impl Recording {
    /// Create a new recording.
    pub fn new(url: &str) -> Self {
        Self {
            version: "1.0".to_string(),
            url: url.to_string(),
            started_at: chrono_now(),
            duration_ms: 0,
            events: Vec::new(),
            reports: Vec::new(),
            summary: RecordingSummary {
                total_events: 0,
                dom_mutations: 0,
                network_requests: 0,
                layout_shifts: 0,
                errors: 0,
                console_messages: 0,
                lifecycle_events: 0,
                animations: 0,
                time_to_interactive_ms: None,
                total_cls: 0.0,
            },
        }
    }

    /// Add a stream event to the recording.
    pub fn add_event(&mut self, event: StreamEvent) {
        self.summary.total_events += 1;
        match event.category.as_str() {
            "dom" => self.summary.dom_mutations += 1,
            "network" => self.summary.network_requests += 1,
            "layout" => {
                self.summary.layout_shifts += 1;
                // Parse CLS value from detail
                if let Some(cls_str) = event.detail.strip_prefix("CLS ") {
                    if let Ok(cls) = cls_str.parse::<f64>() {
                        self.summary.total_cls += cls;
                    }
                }
            }
            "error" => self.summary.errors += 1,
            "console" => self.summary.console_messages += 1,
            "lifecycle" => {
                self.summary.lifecycle_events += 1;
                if event.detail == "InteractiveTime" {
                    self.summary.time_to_interactive_ms = Some(event.time_ms);
                }
            }
            "animation" => self.summary.animations += 1,
            _ => {}
        }
        self.duration_ms = event.time_ms;
        self.events.push(event);
    }

    /// Add an observation report.
    pub fn add_report(&mut self, report: ObservationReport) {
        self.reports.push(report);
    }

    /// Save recording to a JSON file.
    pub fn save(&self, path: &str) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a recording from a JSON file.
    pub fn load(path: &str) -> Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let recording: Self = serde_json::from_str(&json)?;
        Ok(recording)
    }

    /// Print a human-readable summary.
    pub fn print_summary(&self) {
        println!("=== Sentinel Recording ===");
        println!("URL:        {}", self.url);
        println!("Started:    {}", self.started_at);
        println!("Duration:   {}ms", self.duration_ms);
        println!();
        println!("--- Event Summary ---");
        println!("Total events:      {}", self.summary.total_events);
        println!("DOM mutations:     {}", self.summary.dom_mutations);
        println!("Network requests:  {}", self.summary.network_requests);
        println!(
            "Layout shifts:     {} (CLS: {:.4})",
            self.summary.layout_shifts, self.summary.total_cls
        );
        println!("Errors:            {}", self.summary.errors);
        println!("Console messages:  {}", self.summary.console_messages);
        println!("Lifecycle events:  {}", self.summary.lifecycle_events);
        println!("Animations:        {}", self.summary.animations);
        if let Some(tti) = self.summary.time_to_interactive_ms {
            println!("Time to Interactive: {}ms", tti);
        }
        println!();
        println!("--- Reports ({}) ---", self.reports.len());
        for (i, report) in self.reports.iter().enumerate() {
            println!(
                "  {}. {} → {} ({}ms)",
                i + 1,
                report.action,
                report.state,
                report.time_to_stable_ms
            );
        }
    }

    /// Print the full timeline.
    pub fn print_timeline(&self) {
        self.print_summary();
        println!();
        println!("--- Full Timeline ---");
        for event in &self.events {
            let target = event.target.as_deref().unwrap_or("");
            let target_str = if target.is_empty() {
                String::new()
            } else {
                format!(" [{}]", target)
            };
            println!(
                "{:>6}ms  {:>12}  {}{}",
                event.time_ms, event.category, event.detail, target_str
            );
        }
    }
}

/// Get current timestamp as ISO string (no chrono dependency).
fn chrono_now() -> String {
    // Use std::time for a basic timestamp
    let now = std::time::SystemTime::now();
    let since_epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s since epoch", since_epoch.as_secs())
}
