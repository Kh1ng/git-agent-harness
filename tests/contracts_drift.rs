mod support;

use serde_json::Value;
use std::collections::{BTreeSet, HashSet};

use support::scenario::ScenarioHarness;

const CONTRACTS_GAH_TS: &str = include_str!("../packages/contracts/src/gah.ts");

#[test]
fn status_json_matches_contract_snapshot_fields() {
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    let payload = harness.run_status_json().expect("status --json must run");
    assert_interface_fields_match(&payload, "StatusSnapshot", "gah status --json");
}

#[test]
fn quota_snapshot_json_matches_contract_fields() {
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    let payload = harness
        .run_quota_snapshot_json("7d")
        .expect("quota snapshot --json must run");
    assert_interface_fields_match(&payload, "QuotaSnapshot", "gah quota snapshot --json");
}

#[test]
fn report_json_matches_contract_fields() {
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    let payload = harness
        .run_report_json("backend")
        .expect("gah report --json must run");
    assert_interface_fields_match(&payload, "ReportData", "gah report --json");
}

fn assert_interface_fields_match(payload: &Value, interface_name: &str, label: &str) {
    let rust_fields = json_top_level_fields(payload);
    let ts_fields = contract_interface_fields(interface_name);

    let missing_in_contract: BTreeSet<_> = rust_fields
        .difference(&ts_fields)
        .map(String::from)
        .collect();
    let extra_in_contract: BTreeSet<_> = ts_fields
        .difference(&rust_fields)
        .map(String::from)
        .collect();

    assert!(
        missing_in_contract.is_empty() && extra_in_contract.is_empty(),
        "{} field drift\n  fields in Rust JSON missing from `{}`: {:?}\n  fields in contracts not present in Rust JSON: {:?}",
        label,
        interface_name,
        missing_in_contract,
        extra_in_contract
    );
}

fn json_top_level_fields(value: &Value) -> HashSet<String> {
    value
        .as_object()
        .map(|obj| obj.keys().map(|key| key.to_string()).collect())
        .unwrap_or_default()
}

fn contract_interface_fields(interface_name: &str) -> HashSet<String> {
    let marker = format!("export interface {}", interface_name);
    let start = CONTRACTS_GAH_TS
        .find(&marker)
        .unwrap_or_else(|| panic!("cannot find interface `{interface_name}` in contracts"));
    let after_marker = &CONTRACTS_GAH_TS[start..];
    let body_start = after_marker
        .find('{')
        .unwrap_or_else(|| panic!("interface `{interface_name}` has no opening brace"));
    let mut depth = 0;
    let mut body_end = 0;
    for (offset, ch) in after_marker[body_start + 1..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    body_end = offset + body_start;
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    if body_end == 0 {
        panic!("interface `{interface_name}` has no closing brace");
    }

    let body = &after_marker[body_start + 1..body_end];
    body.lines()
        .filter_map(parse_interface_key)
        .collect::<HashSet<_>>()
}

fn parse_interface_key(line: &str) -> Option<String> {
    let mut text = line.trim();
    if text.is_empty() {
        return None;
    }
    if text.starts_with("//") || text.starts_with("/*") || text.starts_with("* ") || text == "*/" {
        return None;
    }

    if let Some(comment_start) = text.find("//") {
        text = text[..comment_start].trim();
        if text.is_empty() {
            return None;
        }
    }

    let part = match text.find(':') {
        Some(idx) => text[..idx].trim(),
        None => return None,
    };
    let part = part.trim_end_matches('?').trim();
    if part.is_empty() || part.starts_with('[') || part.starts_with('*') || part.contains(' ') {
        return None;
    }
    if !is_identifier(part) {
        return None;
    }

    Some(part.to_string())
}

fn is_identifier(token: &str) -> bool {
    let mut chars = token.chars();
    let first = match chars.next() {
        Some(ch) => ch,
        None => return false,
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}
