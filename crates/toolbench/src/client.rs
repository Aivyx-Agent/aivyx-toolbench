//! One HTTP exchange; never fails, records what happened.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config::Endpoint;

#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    pub http_status: Option<u16>,
    pub body: Option<Value>,
    pub error: Option<String>,
    pub latency_ms: u64,
}

pub async fn post(
    client: &reqwest::Client,
    ep: &Endpoint,
    body: &Value,
    timeout: Duration,
) -> Exchange {
    let start = Instant::now();
    let result = client
        .post(ep.chat_url())
        .json(body)
        .timeout(timeout)
        .send()
        .await;
    let latency_ms = || start.elapsed().as_millis() as u64;
    let response = match result {
        Ok(r) => r,
        Err(e) => {
            let what = if e.is_timeout() {
                "timed out"
            } else {
                "request failed"
            };
            return Exchange {
                http_status: None,
                body: None,
                error: Some(format!("{what}: {e}")),
                latency_ms: latency_ms(),
            };
        }
    };
    let status = response.status();
    let text = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            return Exchange {
                http_status: Some(status.as_u16()),
                body: None,
                error: Some(format!("reading body: {e}")),
                latency_ms: latency_ms(),
            };
        }
    };
    let latency_ms = latency_ms();
    let body = serde_json::from_str::<Value>(&text).ok();
    let error = if !status.is_success() {
        Some(format!(
            "HTTP {}: {}",
            status.as_u16(),
            text.chars().take(500).collect::<String>()
        ))
    } else if body.is_none() {
        Some(format!(
            "body is not JSON: {}",
            text.chars().take(200).collect::<String>()
        ))
    } else {
        None
    };
    Exchange {
        http_status: Some(status.as_u16()),
        body,
        error,
        latency_ms,
    }
}

/// `GET {base}/models` within 5 s: is anything listening? Returns the model
/// ids it lists (empty if it lists none).
pub async fn reachable(client: &reqwest::Client, ep: &Endpoint) -> Result<Vec<String>> {
    let resp = client
        .get(ep.models_url())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("endpoint `{}` at {} is unreachable", ep.name, ep.base_url))?;
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    Ok(body["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}
