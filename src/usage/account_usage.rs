//! Secret-free provider account consumption, separate from task ledger usage.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountUsageObservation {
    pub account_id: String,
    #[serde(deserialize_with = "required_workspace")]
    pub workspace_id: Option<String>,
    pub period_start: String,
    pub period_end: String,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_source: Option<AccountCostSource>,
    pub models: Vec<AccountUsageModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountUsageModel {
    pub model: String,
    pub usage_type: AccountUsageType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountUsageType {
    Vibe,
    VibeConnectors,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountCostSource {
    DashboardPrices,
}

fn required_workspace<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn metrics(counters: [Option<u64>; 4], cost: Option<f64>) -> Result<()> {
    if counters
        .into_iter()
        .flatten()
        .any(|count| count > 9_007_199_254_740_991)
        || cost.is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        bail!("invalid account usage metrics");
    }
    Ok(())
}

impl AccountUsageObservation {
    pub fn validate(&self) -> Result<()> {
        let start = OffsetDateTime::parse(&self.period_start, &Rfc3339)?;
        let end = OffsetDateTime::parse(&self.period_end, &Rfc3339)?;
        if end <= start
            || self.currency != "USD"
            || !text(&self.account_id, 256)
            || self
                .workspace_id
                .as_deref()
                .is_some_and(|id| !text(id, 256))
            || self.models.len() > 128
        {
            bail!("invalid account usage scope");
        }
        metrics(
            [
                self.requests,
                self.input_tokens,
                self.cached_input_tokens,
                self.output_tokens,
            ],
            self.cost,
        )?;
        if (self.cost.is_some() || self.models.iter().any(|model| model.cost.is_some()))
            && self.cost_source.is_none()
        {
            bail!("account usage cost requires its pricing source");
        }
        for model in &self.models {
            if !text(&model.model, 512) {
                bail!("invalid account usage model");
            }
            metrics(
                [
                    model.requests,
                    model.input_tokens,
                    model.cached_input_tokens,
                    model.output_tokens,
                ],
                model.cost,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_fields_unsafe_counts_and_invalid_scope_are_rejected() {
        let valid = serde_json::json!({"account_id":"synthetic-account","workspace_id":null,"period_start":"2026-10-01T00:00:00Z","period_end":"2026-10-03T00:00:00Z","currency":"USD","requests":4,"models":[]});
        for field in ["cookie", "workspace_id", "requests", "period_end", "cost"] {
            let mut value = valid.clone();
            match field {
                "cookie" => value["cookie"] = serde_json::json!("secret-must-not-enter-store"),
                "workspace_id" => {
                    value.as_object_mut().unwrap().remove(field);
                }
                "requests" => value[field] = serde_json::json!(9_007_199_254_740_992u64),
                "period_end" => value[field] = value["period_start"].clone(),
                _ => value[field] = serde_json::json!(1.0),
            }
            let parsed = serde_json::from_value::<AccountUsageObservation>(value);
            assert!(
                parsed.is_err() || parsed.unwrap().validate().is_err(),
                "{field}"
            );
        }
    }
}
