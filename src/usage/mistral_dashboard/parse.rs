//! The dashboard's grouped events are consumption, not execution ledger entries.
use super::super::account_usage::{
    AccountCostSource, AccountUsageModel, AccountUsageObservation, AccountUsageType,
};
use crate::quota_store::QuotaObservationRecord;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("invalid Mistral dashboard schema")
}

pub(super) fn body(bytes: &[u8]) -> Result<Value> {
    let wrapper: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid Mistral dashboard response"))?;
    wrapper
        .pointer("/result/data/json")
        .filter(|v| v.is_object())
        .cloned()
        .context("invalid Mistral dashboard response")
}

fn timestamp(value: &Value, key: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(string(value, key)?, &Rfc3339).context("invalid Mistral dashboard period")
}

fn scope(
    value: &Value,
    start: OffsetDateTime,
    end: OffsetDateTime,
    grouping: &[&str],
    requests: bool,
) -> Result<String> {
    let metadata = &value["metadata"];
    let params = &metadata["queryParams"];
    let expected = serde_json::json!(grouping);
    if metadata["hasMore"] != false
        || params["groupBy"] != expected
        || params["includeNbApiCalls"] != requests
        || params["offset"] != 0
        || params["workspaceIds"] != Value::Null
        || params.get("workspaceIds").is_none()
        || params["untracked"] != false
        || params["preferPeriodAggregate"] != false
        || timestamp(params, "start")? != start
        || timestamp(params, "end")? != end
    {
        bail!("partial or mismatched Mistral dashboard scope");
    }
    for key in [
        "apiKeyId",
        "apiZone",
        "billingDisplayName",
        "billingGroup",
        "billingMetric",
        "callSource",
        "callType",
        "filters",
        "organizationId",
        "serviceAccountId",
        "serviceTier",
        "timeBucket",
        "usageType",
        "userId",
    ] {
        if params.get(key) != Some(&Value::Null) {
            bail!("filtered Mistral dashboard scope");
        }
    }
    Ok(string(params, "customerId")?.to_owned())
}

fn groups(value: &Value) -> Result<&Vec<Value>> {
    value["groups"]
        .as_array()
        .filter(|groups| groups.len() <= 50000)
        .context("invalid Mistral dashboard groups")
}

fn usage_type(row: &Value) -> Result<Option<AccountUsageType>> {
    Ok(match string(row, "usageType")? {
        "vibe" => Some(AccountUsageType::Vibe),
        "vibe_connectors" => Some(AccountUsageType::VibeConnectors),
        _ => None,
    })
}

fn count(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .context("invalid Mistral dashboard count")
}

fn add(total: &mut Option<u64>, value: u64) -> Result<()> {
    *total = Some(
        total
            .unwrap_or_default()
            .checked_add(value)
            .context("Mistral dashboard count overflow")?,
    );
    Ok(())
}

fn model(name: String, usage_type: AccountUsageType) -> AccountUsageModel {
    AccountUsageModel {
        model: name,
        usage_type,
        requests: None,
        input_tokens: None,
        cached_input_tokens: None,
        output_tokens: None,
        cost: Some(0.0),
    }
}

fn budget(bytes: &[u8], now: OffsetDateTime) -> Result<(f64, String)> {
    let value = body(bytes)?;
    let vibe = &value["vibe_budget"];
    let used = vibe["usage_percentage"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        .context("unknown Vibe allowance")?;
    if vibe["currency"] != "USD"
        || !vibe["initial_budget"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v > 0.0)
        || timestamp(vibe, "reset_at")? <= now
    {
        bail!("unknown Vibe allowance");
    }
    Ok((used, string(vibe, "reset_at")?.to_owned()))
}

/// Match every price dimension; a display alias is never a billing metric.
pub(super) fn parse(
    cost_bytes: &[u8],
    request_bytes: &[u8],
    price_bytes: &[u8],
    budget_bytes: Option<&[u8]>,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<QuotaObservationRecord> {
    let costs = body(cost_bytes)?;
    let requests = body(request_bytes)?;
    let prices = body(price_bytes)?;
    let account = scope(
        &costs,
        start,
        end,
        &[
            "billing_metric",
            "billing_display_name",
            "billing_group",
            "usage_type",
            "api_zone",
            "service_tier",
        ],
        false,
    )?;
    if account
        != scope(
            &requests,
            start,
            end,
            &["billing_display_name", "usage_type"],
            true,
        )?
        || prices["currency"] != "USD"
    {
        bail!("mismatched Mistral dashboard account/currency");
    }
    let prices = prices["prices"]
        .as_array()
        .filter(|rows| rows.len() <= 50000)
        .context("invalid Mistral dashboard prices")?;
    let mut models = BTreeMap::new();
    for row in groups(&requests)? {
        let Some(kind) = usage_type(row)? else {
            continue;
        };
        let name = string(row, "billingDisplayName")?.to_owned();
        let key = (name.clone(), kind);
        if models.insert(key, model(name, kind)).is_some() {
            bail!("ambiguous Mistral request group");
        }
        let item = models
            .get_mut(&(string(row, "billingDisplayName")?.to_owned(), kind))
            .unwrap();
        item.requests = Some(count(&row["nbApiCalls"])?);
    }
    let mut billed_models = BTreeSet::new();
    let mut billing_groups = BTreeSet::new();
    for row in groups(&costs)? {
        let Some(kind) = usage_type(row)? else {
            continue;
        };
        let key = (string(row, "billingDisplayName")?.to_owned(), kind);
        billed_models.insert(key.clone());
        let tuple: Vec<_> = [
            "billingDisplayName",
            "usageType",
            "apiZone",
            "billingMetric",
            "billingGroup",
            "serviceTier",
        ]
        .iter()
        .map(|key| string(row, key))
        .collect::<Result<_>>()?;
        if !billing_groups.insert(tuple) {
            bail!("duplicate Mistral billing group");
        }
        let item = models
            .get_mut(&key)
            .context("incomplete Mistral request groups")?;
        let units = count(&row["value"])?;
        match (kind, string(row, "billingGroup")?) {
            (AccountUsageType::Vibe, "input") => add(&mut item.input_tokens, units)?,
            (AccountUsageType::Vibe, "cached") => add(&mut item.cached_input_tokens, units)?,
            (AccountUsageType::Vibe, "output") => add(&mut item.output_tokens, units)?,
            (AccountUsageType::VibeConnectors, "calls") => {}
            _ => bail!("unknown Mistral Vibe billing group"),
        }
        for key in ["apiZone", "billingMetric", "billingGroup", "serviceTier"] {
            string(row, key)?;
        }
        let event = if kind == AccountUsageType::Vibe {
            "api_tokens"
        } else {
            "api_connectors"
        };
        let matching: Vec<_> = prices
            .iter()
            .filter(|price| {
                ["apiZone", "billingMetric", "billingGroup", "serviceTier"]
                    .iter()
                    .all(|key| price[key] == row[key])
                    && price["eventType"] == event
            })
            .collect();
        let price = if matching.len() == 1 {
            matching[0]["price"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
        } else {
            None
        };
        item.cost = item
            .cost
            .zip(price)
            .map(|(total, price)| total + units as f64 * price);
    }
    for (key, item) in &mut models {
        if !billed_models.contains(key) {
            item.cost = None;
        }
    }
    let mut usage = AccountUsageObservation {
        account_id: account,
        workspace_id: None,
        period_start: start.format(&Rfc3339)?,
        period_end: end.format(&Rfc3339)?,
        currency: "USD".into(),
        requests: None,
        input_tokens: None,
        cached_input_tokens: None,
        output_tokens: None,
        cost: Some(0.0),
        cost_source: Some(AccountCostSource::DashboardPrices),
        models: models.into_values().collect(),
    };
    for item in &usage.models {
        for (total, value) in [
            (&mut usage.requests, item.requests),
            (&mut usage.input_tokens, item.input_tokens),
            (&mut usage.cached_input_tokens, item.cached_input_tokens),
            (&mut usage.output_tokens, item.output_tokens),
        ] {
            if let Some(value) = value {
                add(total, value)?;
            }
        }
        usage.cost = usage.cost.zip(item.cost).map(|(total, cost)| total + cost);
    }
    usage.validate()?;
    let identity_json = serde_json::to_vec(&(&usage.account_id, &usage.workspace_id))?;
    let identity = format!(
        "mistral-dashboard:{}",
        &format!("{:x}", Sha256::digest(identity_json))[..24]
    );
    let allowance = budget_bytes.and_then(|bytes| budget(bytes, end).ok());
    let checked = end.format(&Rfc3339)?;
    Ok(QuotaObservationRecord {
        backend: "mistral-dashboard".into(),
        backend_instance: Some(identity.clone()),
        model: None,
        quota_pool: Some(identity),
        quota_window: Some("vibe-code-included-monthly".into()),
        quota_used_percent: allowance.as_ref().map(|(used, _)| *used),
        quota_remaining_percent: allowance.as_ref().map(|(used, _)| 100.0 - used),
        quota_reset_at: allowance.map(|(_, reset)| reset),
        observed_at: Some(checked.clone()),
        checked_at: Some(checked),
        check_error: None,
        usage_source: Some("mistral_dashboard".into()),
        mistral_admin: None,
        account_usage: Some(usage),
    })
}
