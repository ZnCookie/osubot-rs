use crate::dedup::RequestDedup;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use tracing::warn;

#[derive(Deserialize)]
pub struct SendAction {
    pub action: String,
    pub params: serde_json::Value,
}

pub fn extract_text_from_message(msg: &serde_json::Value) -> String {
    match msg {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|seg| {
                if seg["type"] == "text" {
                    seg["data"]["text"].as_str().map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Resolves QQ → osu! user binding from an external bot server.
///
/// Implementations query an upstream service (e.g., another osu! bot's
/// binding database) to find the osu! user associated with a QQ number.
#[async_trait]
pub trait UpstreamBindingProvider: Send + Sync {
    /// Query the upstream for a binding. Returns `Ok(Some((user_id, username)))`
    /// if found, `Ok(None)` if the QQ is not known to this upstream, or
    /// `Err(...)` on communication failure.
    async fn query_binding(&self, qq: i64) -> Result<Option<(i64, String)>, String>;
}

type BindDedup = RequestDedup<i64, Option<(i64, String)>, String>;

/// Chains multiple upstream binding providers, returning the first successful result.
///
/// Providers are tried in configuration order. Concurrent queries for the
/// same QQ number are deduplicated via [`RequestDedup`].
pub struct UpstreamChain {
    providers: Arc<Vec<Box<dyn UpstreamBindingProvider>>>,
    dedup: BindDedup,
}

impl UpstreamChain {
    pub fn new(providers: Vec<Box<dyn UpstreamBindingProvider>>) -> Self {
        Self {
            providers: Arc::new(providers),
            dedup: RequestDedup::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Returns the first successful binding from any provider, or `None` if
    /// all providers fail or the chain is empty. Failures are silent — the
    /// caller should fall back to prompting the user to bind manually.
    pub async fn try_query(&self, qq: i64) -> Option<(i64, String)> {
        if self.providers.is_empty() {
            return None;
        }
        let providers = Arc::clone(&self.providers);
        self.dedup
            .run_or_wait(qq, move || {
                let providers = Arc::clone(&providers);
                async move {
                    for provider in providers.iter() {
                        match provider.query_binding(qq).await {
                            Ok(Some(binding)) => return Ok(Some(binding)),
                            Ok(None) => continue,
                            Err(e) => {
                                tracing::warn!(
                                    ?e,
                                    "upstream provider query failed, trying next provider"
                                );
                                continue;
                            }
                        }
                    }
                    Ok(None)
                }
            })
            .await
            .unwrap_or_else(|e| {
                warn!("upstream dedup error: {e}");
                None
            })
    }
}
