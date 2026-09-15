//! A thin Redfish client.
//!
//! Redfish is a plain REST API with HTTP Basic auth, so there is no session to
//! juggle: every request carries the credentials. That keeps the client small
//! and, more importantly, stateless — a `watch` loop that runs for hours never
//! has to renew anything.

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;

use crate::config::Config;

pub struct Client {
    http: reqwest::Client,
    base: String,
    user: String,
    password: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(!cfg.verify_tls)
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .build()
            .context("could not build the HTTP client")?;
        Ok(Self {
            http,
            base: format!("https://{}", cfg.host.trim_end_matches('/')),
            user: cfg.user.clone(),
            password: cfg.password.clone(),
        })
    }

    pub async fn get_value(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let res = self
            .http
            .get(&url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{} {}\n{}", status.as_u16(), path, describe(&body));
        }
        serde_json::from_str(&body).with_context(|| format!("{path} did not return JSON"))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let v = self.get_value(path).await?;
        serde_json::from_value(v).with_context(|| format!("{path} did not have the expected shape"))
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<()> {
        let url = format!("{}{}", self.base, path);
        let res = self
            .http
            .post(&url)
            .basic_auth(&self.user, Some(&self.password))
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{} {}\n{}", status.as_u16(), path, describe(&text));
        }
        Ok(())
    }
}

/// Redfish wraps its errors in a deep envelope. Digging the human-readable
/// message out of it is the difference between "400 Bad Request" and knowing
/// that the reset type you asked for is not supported by this machine.
fn describe(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return body.chars().take(400).collect();
    };
    let info = v
        .get("error")
        .and_then(|e| e.get("@Message.ExtendedInfo"))
        .and_then(|e| e.as_array());
    if let Some(items) = info {
        let msgs: Vec<String> = items
            .iter()
            .filter_map(|m| m.get("Message").and_then(|s| s.as_str()))
            .map(|s| s.to_string())
            .collect();
        if !msgs.is_empty() {
            return msgs.join("\n");
        }
    }
    v.get("error")
        .and_then(|e| e.get("message"))
        .and_then(|s| s.as_str())
        .unwrap_or(body)
        .to_string()
}
