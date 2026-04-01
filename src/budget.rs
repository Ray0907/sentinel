//! Performance budget checking for CI integration.
//!
//! Parse budget rules like "CLS<0.1,TTI<3000,requests<50,errors=0"
//! and check recorded metrics against them.

use crate::recording::RecordingSummary;

/// A single budget rule.
#[derive(Debug)]
pub struct BudgetRule {
    pub metric: String,
    pub op: BudgetOp,
    pub threshold: f64,
}

#[derive(Debug)]
pub enum BudgetOp {
    LessThan,
    LessOrEqual,
    Equal,
    GreaterThan,
}

/// Result of checking one rule.
#[derive(Debug)]
pub struct BudgetResult {
    pub rule: String,
    pub actual: f64,
    pub threshold: f64,
    pub passed: bool,
}

/// Parse a budget string like "CLS<0.1,TTI<3000,requests<50,errors=0"
pub fn parse_budget(budget: &str) -> Vec<BudgetRule> {
    budget
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }

            // Find operator position
            let (metric, op, value) = if let Some(pos) = part.find("<=") {
                (&part[..pos], BudgetOp::LessOrEqual, &part[pos + 2..])
            } else if let Some(pos) = part.find('<') {
                (&part[..pos], BudgetOp::LessThan, &part[pos + 1..])
            } else if let Some(pos) = part.find('>') {
                (&part[..pos], BudgetOp::GreaterThan, &part[pos + 1..])
            } else if let Some(pos) = part.find('=') {
                (&part[..pos], BudgetOp::Equal, &part[pos + 1..])
            } else {
                return None;
            };

            let threshold: f64 = value.trim().parse().ok()?;
            Some(BudgetRule {
                metric: metric.trim().to_lowercase(),
                op,
                threshold,
            })
        })
        .collect()
}

/// Check budget rules against recorded summary.
pub fn check_budget(rules: &[BudgetRule], summary: &RecordingSummary) -> Vec<BudgetResult> {
    rules
        .iter()
        .map(|rule| {
            let actual = get_metric_value(&rule.metric, summary);
            let passed = match rule.op {
                BudgetOp::LessThan => actual < rule.threshold,
                BudgetOp::LessOrEqual => actual <= rule.threshold,
                BudgetOp::Equal => (actual - rule.threshold).abs() < f64::EPSILON,
                BudgetOp::GreaterThan => actual > rule.threshold,
            };

            let op_str = match rule.op {
                BudgetOp::LessThan => "<",
                BudgetOp::LessOrEqual => "<=",
                BudgetOp::Equal => "=",
                BudgetOp::GreaterThan => ">",
            };

            BudgetResult {
                rule: format!("{}{}{}", rule.metric, op_str, rule.threshold),
                actual,
                threshold: rule.threshold,
                passed,
            }
        })
        .collect()
}

/// Map metric name to value from summary.
fn get_metric_value(metric: &str, summary: &RecordingSummary) -> f64 {
    match metric {
        "cls" => summary.total_cls,
        "tti" | "interactive" => summary.time_to_interactive_ms.unwrap_or(0) as f64,
        "requests" | "network" => summary.network_requests as f64,
        "errors" => summary.errors as f64,
        "dom" | "mutations" | "dom_mutations" => summary.dom_mutations as f64,
        "layout_shifts" | "shifts" => summary.layout_shifts as f64,
        "console" | "console_messages" => summary.console_messages as f64,
        "events" | "total_events" | "total" => summary.total_events as f64,
        "animations" => summary.animations as f64,
        _ => 0.0,
    }
}

/// Print budget results and return whether all passed.
pub fn print_results(results: &[BudgetResult]) -> bool {
    let all_passed = results.iter().all(|r| r.passed);

    println!("=== Performance Budget Check ===");
    println!();

    for r in results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        let icon = if r.passed { "  " } else { "! " };
        println!(
            "{icon}{status}  {:<30}  actual: {:<10}  budget: {}",
            r.rule,
            format_value(r.actual),
            format_value(r.threshold),
        );
    }

    println!();
    if all_passed {
        println!("All budgets passed.");
    } else {
        let failed = results.iter().filter(|r| !r.passed).count();
        println!("{failed} budget(s) exceeded.");
    }

    all_passed
}

fn format_value(v: f64) -> String {
    if v == v.floor() && v < 1_000_000.0 {
        format!("{}", v as i64)
    } else {
        format!("{:.4}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_rules() {
        let rules = parse_budget("CLS<0.1,TTI<3000,errors=0");
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].metric, "cls");
        assert_eq!(rules[1].metric, "tti");
        assert_eq!(rules[2].metric, "errors");
    }

    #[test]
    fn parse_with_spaces() {
        let rules = parse_budget("CLS < 0.1, requests < 50");
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn check_passing_budget() {
        let rules = parse_budget("errors=0,requests<100");
        let summary = RecordingSummary {
            total_events: 10,
            dom_mutations: 2,
            network_requests: 5,
            layout_shifts: 0,
            errors: 0,
            console_messages: 0,
            lifecycle_events: 3,
            animations: 0,
            time_to_interactive_ms: Some(1000),
            total_cls: 0.0,
        };
        let results = check_budget(&rules, &summary);
        assert!(results.iter().all(|r| r.passed));
    }

    #[test]
    fn check_failing_budget() {
        let rules = parse_budget("errors=0,CLS<0.05");
        let summary = RecordingSummary {
            total_events: 10,
            dom_mutations: 2,
            network_requests: 5,
            layout_shifts: 2,
            errors: 3,
            console_messages: 0,
            lifecycle_events: 3,
            animations: 0,
            time_to_interactive_ms: Some(5000),
            total_cls: 0.1,
        };
        let results = check_budget(&rules, &summary);
        assert!(!results[0].passed); // errors=0 but got 3
        assert!(!results[1].passed); // CLS<0.05 but got 0.1
    }
}
