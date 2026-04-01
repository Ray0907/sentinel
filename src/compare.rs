//! Baseline comparison: compare two recordings or a recording vs live run.

use crate::recording::RecordingSummary;

#[derive(Debug)]
pub struct CompareResult {
    pub metrics: Vec<MetricDiff>,
    pub regressions: usize,
}

#[derive(Debug)]
pub struct MetricDiff {
    pub name: String,
    pub baseline: f64,
    pub current: f64,
    pub change_pct: f64,
    pub status: DiffStatus,
}

#[derive(Debug, PartialEq)]
pub enum DiffStatus {
    Ok,
    Improved,
    Regression,
    New, // was 0, now > 0
}

/// Compare two recording summaries.
pub fn compare(baseline: &RecordingSummary, current: &RecordingSummary) -> CompareResult {
    let metrics = vec![
        diff("CLS", baseline.total_cls, current.total_cls, 0.1, false),
        diff(
            "TTI (ms)",
            baseline.time_to_interactive_ms.unwrap_or(0) as f64,
            current.time_to_interactive_ms.unwrap_or(0) as f64,
            0.2,
            false,
        ),
        diff(
            "Errors",
            baseline.errors as f64,
            current.errors as f64,
            0.0,
            true,
        ),
        diff(
            "Network Requests",
            baseline.network_requests as f64,
            current.network_requests as f64,
            0.3,
            false,
        ),
        diff(
            "DOM Mutations",
            baseline.dom_mutations as f64,
            current.dom_mutations as f64,
            0.5,
            false,
        ),
        diff(
            "Layout Shifts",
            baseline.layout_shifts as f64,
            current.layout_shifts as f64,
            0.0,
            true,
        ),
        diff(
            "Console Messages",
            baseline.console_messages as f64,
            current.console_messages as f64,
            0.5,
            false,
        ),
        diff(
            "Total Events",
            baseline.total_events as f64,
            current.total_events as f64,
            0.5,
            false,
        ),
    ];

    let regressions = metrics
        .iter()
        .filter(|m| m.status == DiffStatus::Regression || m.status == DiffStatus::New)
        .count();
    CompareResult {
        metrics,
        regressions,
    }
}

/// Build a MetricDiff. `threshold` is the % change that triggers regression.
/// `any_increase_is_regression` means even +1 is a regression (for errors).
fn diff(
    name: &str,
    baseline: f64,
    current: f64,
    threshold: f64,
    any_increase_is_regression: bool,
) -> MetricDiff {
    let change_pct = if baseline == 0.0 {
        if current == 0.0 {
            0.0
        } else {
            100.0
        }
    } else {
        ((current - baseline) / baseline) * 100.0
    };

    let status = if any_increase_is_regression {
        if current > baseline {
            if baseline == 0.0 {
                DiffStatus::New
            } else {
                DiffStatus::Regression
            }
        } else if current < baseline {
            DiffStatus::Improved
        } else {
            DiffStatus::Ok
        }
    } else if change_pct > threshold * 100.0 {
        DiffStatus::Regression
    } else if change_pct < -(threshold * 100.0) {
        DiffStatus::Improved
    } else {
        DiffStatus::Ok
    };

    MetricDiff {
        name: name.to_string(),
        baseline,
        current,
        change_pct: (change_pct * 10.0).round() / 10.0,
        status,
    }
}

/// Print comparison results. Returns true if no regressions.
pub fn print_results(result: &CompareResult) -> bool {
    println!("=== Baseline Comparison ===");
    println!();
    println!(
        "{:<20} {:>12} {:>12} {:>10}  Status",
        "Metric", "Baseline", "Current", "Change"
    );
    println!("{}", "-".repeat(72));

    for m in &result.metrics {
        let icon = match m.status {
            DiffStatus::Ok => "  ",
            DiffStatus::Improved => "* ",
            DiffStatus::Regression => "! ",
            DiffStatus::New => "! ",
        };
        let status = match m.status {
            DiffStatus::Ok => "OK",
            DiffStatus::Improved => "IMPROVED",
            DiffStatus::Regression => "REGRESSION",
            DiffStatus::New => "NEW",
        };
        let change = if m.change_pct == 0.0 {
            "—".to_string()
        } else {
            format!("{:+.1}%", m.change_pct)
        };

        println!(
            "{icon}{:<20} {:>12} {:>12} {:>10}  {status}",
            m.name,
            fmt_val(m.baseline),
            fmt_val(m.current),
            change,
        );
    }

    println!();
    if result.regressions == 0 {
        println!("No regressions detected.");
    } else {
        println!("{} regression(s) detected.", result.regressions);
    }

    result.regressions == 0
}

fn fmt_val(v: f64) -> String {
    if v == v.floor() && v < 1_000_000.0 {
        format!("{}", v as i64)
    } else {
        format!("{:.4}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary(cls: f64, tti: u64, errors: usize, requests: usize) -> RecordingSummary {
        RecordingSummary {
            total_events: 10,
            dom_mutations: 2,
            network_requests: requests,
            layout_shifts: 0,
            errors,
            console_messages: 0,
            lifecycle_events: 3,
            animations: 0,
            time_to_interactive_ms: Some(tti),
            total_cls: cls,
        }
    }

    #[test]
    fn no_change() {
        let a = make_summary(0.05, 2000, 0, 10);
        let r = compare(&a, &a);
        assert_eq!(r.regressions, 0);
    }

    #[test]
    fn regression_detected() {
        let baseline = make_summary(0.05, 2000, 0, 10);
        let current = make_summary(0.2, 4000, 2, 10);
        let r = compare(&baseline, &current);
        assert!(r.regressions > 0);
    }

    #[test]
    fn improvement_detected() {
        let baseline = make_summary(0.2, 5000, 0, 50);
        let current = make_summary(0.01, 1000, 0, 10);
        let r = compare(&baseline, &current);
        assert_eq!(r.regressions, 0);
        assert!(r.metrics.iter().any(|m| m.status == DiffStatus::Improved));
    }

    #[test]
    fn new_errors() {
        let baseline = make_summary(0.0, 1000, 0, 5);
        let current = make_summary(0.0, 1000, 3, 5);
        let r = compare(&baseline, &current);
        assert!(r.metrics.iter().any(|m| m.status == DiffStatus::New));
    }
}
