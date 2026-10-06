use super::*;
use serde_json::{json, Value};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mistral-dashboard")
            .join(format!("{name}.json")),
    )
    .unwrap()
}
fn period() -> (OffsetDateTime, OffsetDateTime) {
    (
        OffsetDateTime::parse("2026-10-01T00:00:00Z", &Rfc3339).unwrap(),
        OffsetDateTime::parse("2026-10-03T00:00:00Z", &Rfc3339).unwrap(),
    )
}
fn parsed(
    cost: &[u8],
    requests: &[u8],
    prices: &[u8],
    budget: Option<&[u8]>,
) -> Result<QuotaObservationRecord> {
    let (start, end) = period();
    parse::parse(cost, requests, prices, budget, start, end)
}

#[test]
fn dashboard_consumption_joins_prices_and_uses_api_calls_not_event_counts() {
    let record = parsed(
        &fixture("costBreakdown"),
        &fixture("breakdownByModel"),
        &fixture("prices"),
        Some(&fixture("budget")),
    )
    .unwrap();
    assert_eq!(record.backend, "mistral-dashboard");
    assert_eq!(
        record.quota_window.as_deref(),
        Some("vibe-code-included-monthly")
    );
    assert_eq!(record.quota_remaining_percent, Some(40.0));
    let usage = record.account_usage.unwrap();
    assert_eq!(usage.requests, Some(4));
    assert_eq!(usage.input_tokens, Some(300));
    assert_eq!(usage.cached_input_tokens, Some(1000));
    assert_eq!(usage.output_tokens, Some(50));
    assert!((usage.cost.unwrap() - 0.03079).abs() < 1e-12);
    assert!(usage.workspace_id.is_none());
}

#[test]
fn display_alias_does_not_change_billing_price_join() {
    let mut costs: Value = serde_json::from_slice(&fixture("costBreakdown")).unwrap();
    let mut requests: Value = serde_json::from_slice(&fixture("breakdownByModel")).unwrap();
    costs["result"]["data"]["json"]["groups"][3]["billingDisplayName"] =
        json!("mistral-vibe-cli-latest");
    requests["result"]["data"]["json"]["groups"][1]["billingDisplayName"] =
        json!("mistral-vibe-cli-latest");
    let record = parsed(
        &serde_json::to_vec(&costs).unwrap(),
        &serde_json::to_vec(&requests).unwrap(),
        &fixture("prices"),
        None,
    )
    .unwrap();
    let usage = record.account_usage.unwrap();
    assert!(usage
        .models
        .iter()
        .any(|model| model.model == "mistral-vibe-cli-latest"));
    assert!((usage.cost.unwrap() - 0.03079).abs() < 1e-12);
}

#[test]
fn missing_billing_rows_or_price_never_becomes_zero_cost() {
    let mut costs: Value = serde_json::from_slice(&fixture("costBreakdown")).unwrap();
    costs["result"]["data"]["json"]["groups"]
        .as_array_mut()
        .unwrap()
        .remove(3);
    let usage = parsed(
        &serde_json::to_vec(&costs).unwrap(),
        &fixture("breakdownByModel"),
        &fixture("prices"),
        None,
    )
    .unwrap()
    .account_usage
    .unwrap();
    assert!(usage.cost.is_none());
    assert!(usage
        .models
        .iter()
        .find(|model| model.model == "mistral-medium-3-5")
        .unwrap()
        .cost
        .is_none());
    let mut prices: Value = serde_json::from_slice(&fixture("prices")).unwrap();
    prices["result"]["data"]["json"]["prices"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    assert!(parsed(
        &fixture("costBreakdown"),
        &fixture("breakdownByModel"),
        &serde_json::to_vec(&prices).unwrap(),
        None
    )
    .unwrap()
    .account_usage
    .unwrap()
    .cost
    .is_none());
}

#[test]
fn partial_scopes_duplicates_and_unknown_vibe_groups_fail_closed() {
    for mutation in ["partial", "scope", "duplicate", "group"] {
        let mut costs: Value = serde_json::from_slice(&fixture("costBreakdown")).unwrap();
        let body = &mut costs["result"]["data"]["json"];
        match mutation {
            "partial" => body["metadata"]["hasMore"] = json!(true),
            "scope" => body["metadata"]["queryParams"]["workspaceIds"] = json!(["other"]),
            "duplicate" => {
                let first = body["groups"][0].clone();
                body["groups"].as_array_mut().unwrap().push(first);
            }
            _ => body["groups"][0]["billingGroup"] = json!("new-metric"),
        }
        assert!(
            parsed(
                &serde_json::to_vec(&costs).unwrap(),
                &fixture("breakdownByModel"),
                &fixture("prices"),
                None
            )
            .is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn absent_vibe_budget_clears_old_allowance_without_using_api_budget() {
    let mut budget: Value = serde_json::from_slice(&fixture("budget")).unwrap();
    budget["result"]["data"]["json"]["vibe_budget"] = Value::Null;
    let record = parsed(
        &fixture("costBreakdown"),
        &fixture("breakdownByModel"),
        &fixture("prices"),
        Some(&serde_json::to_vec(&budget).unwrap()),
    )
    .unwrap();
    assert!(record.quota_remaining_percent.is_none());
    assert!(record.quota_reset_at.is_none());
    assert!(record.account_usage.is_some());
    let previous = parsed(
        &fixture("costBreakdown"),
        &fixture("breakdownByModel"),
        &fixture("prices"),
        Some(&fixture("budget")),
    )
    .unwrap();
    let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "mistral-dashboard",
        None::<String>,
        record.quota_pool.as_deref(),
    );
    identity.backend_instance = record.backend_instance.clone().unwrap();
    let readings = [previous, record];
    let latest = quota_store::latest_windows_for_identity(&readings, &identity);
    assert_eq!(latest.len(), 1);
    assert!(latest[0].quota_remaining_percent.is_none());
}

#[test]
fn auth_failure_invalidates_same_scoped_customer_and_usage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota.jsonl");
    let record = parsed(
        &fixture("costBreakdown"),
        &fixture("breakdownByModel"),
        &fixture("prices"),
        Some(&fixture("budget")),
    )
    .unwrap();
    let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "mistral-dashboard",
        None::<String>,
        record.quota_pool.as_deref(),
    );
    identity.backend_instance = record.backend_instance.clone().unwrap();
    quota_store::append(&path, &record).unwrap();
    assert!(refresh_and_store_with(
        &path,
        || bail!("auth_required: session expired"),
        period().1
    )
    .unwrap()
    .unwrap()
    .check_error
    .is_some());
    let records = quota_store::load(&path).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].backend_instance, record.backend_instance);
    assert_eq!(records[1].quota_pool, record.quota_pool);
    assert!(quota_store::latest_windows_for_identity(&records, &identity).is_empty());
}

#[test]
fn fixed_reads_use_utc_month_and_preserve_usage_when_budget_fails() {
    let mut endpoints = Vec::new();
    let record = refresh_with(
        "private-test-cookie",
        period().1,
        |cookie, endpoint, input| {
            assert_eq!(cookie, "private-test-cookie");
            endpoints.push(endpoint.to_string());
            match endpoint {
                "usage.costBreakdown" => {
                    assert_eq!(input["json"]["start"], "2026-10-01T00:00:00Z");
                    Ok(fixture("costBreakdown"))
                }
                "usage.breakdownByModel" => {
                    assert_eq!(input["json"]["chartMetric"], "apiCalls");
                    Ok(fixture("breakdownByModel"))
                }
                "usage.prices" => {
                    assert_eq!(input, json!({"json":{"workspaceId":null}}));
                    Ok(fixture("prices"))
                }
                "billing.budget" => bail!("budget unavailable"),
                _ => panic!("unexpected endpoint"),
            }
        },
    )
    .unwrap();
    assert_eq!(endpoints.len(), 4);
    assert!(record.account_usage.is_some());
    assert!(record.quota_remaining_percent.is_none());
}

#[test]
fn native_submillisecond_time_matches_javascript_date_echo() {
    let now = period().1.replace_nanosecond(448_456_789).unwrap();
    let record = refresh_with("synthetic-cookie", now, |_, endpoint, input| {
        let name = match endpoint {
            "usage.costBreakdown" => "costBreakdown",
            "usage.breakdownByModel" => "breakdownByModel",
            "usage.prices" => return Ok(fixture("prices")),
            "billing.budget" => return Ok(fixture("budget")),
            _ => panic!("unexpected endpoint"),
        };
        let requested =
            OffsetDateTime::parse(input["json"]["end"].as_str().unwrap(), &Rfc3339).unwrap();
        assert_eq!(requested.nanosecond(), 448_000_000);
        let mut value: Value = serde_json::from_slice(&fixture(name)).unwrap();
        value["result"]["data"]["json"]["metadata"]["queryParams"]["end"] =
            json!("2026-10-03T00:00:00.448000+00:00");
        Ok(serde_json::to_vec(&value).unwrap())
    })
    .unwrap();
    assert_eq!(
        record.account_usage.unwrap().period_end,
        "2026-10-03T00:00:00.448Z"
    );
}

#[cfg(unix)]
#[test]
fn selected_cookie_requires_private_regular_file_and_no_header_injection() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cookie");
    std::fs::write(&path, "session=synthetic\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(cookie(&path).unwrap(), "session=synthetic");
    let link = dir.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(cookie(&link).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(cookie(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&path, "session=test\nInjected: header").unwrap();
    assert!(cookie(&path).is_err());
    std::fs::write(&path, "").unwrap();
    assert!(cookie(&path).is_err());
}
