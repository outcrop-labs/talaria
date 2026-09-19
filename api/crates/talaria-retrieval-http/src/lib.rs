use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::Value;

/// The injected HTTP edge: (method, url, body, headers, timeout_ms) →
/// (status, body text). Strings back, not parsed JSON — two of the three
/// clients need the raw text for their error sentences, and one (qdrant's
/// upsert) quotes it verbatim.
pub type HttpFetch = Arc<
    dyn Fn(
            &str,
            &str,
            Option<&Value>,
            &[(&str, &str)],
            u64,
        ) -> BoxFuture<'static, Result<(u16, String), String>>
        + Send
        + Sync,
>;

/// The real edge: reqwest, per-call timeout (each client carries its own
/// budget — 30s for qdrant/embed writes, 15s for rerank, 5s for the
/// embed info probe — and the timeout is the caller's parameter, not the
/// client's). `content-type` is set only when a body is present — a GET
/// never sends the header.
pub fn real_http() -> HttpFetch {
    Arc::new(
        |method: &str,
         url: &str,
         body: Option<&Value>,
         headers: &[(&str, &str)],
         timeout_ms: u64| {
            let method = method.to_string();
            let url = url.to_string();
            let headers: Vec<(String, String)> = headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let payload =
                body.map(|b| serde_json::to_string(b).expect("the request body is plain data"));
            Box::pin(async move {
                let method: reqwest::Method =
                    method.parse().map_err(|e| format!("bad method: {e}"))?;
                let mut req = talaria_gateway::provider::http()
                    .request(method, &url)
                    .timeout(std::time::Duration::from_millis(timeout_ms));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Some(p) = payload {
                    req = req.header("content-type", "application/json").body(p);
                }
                let res = req.send().await.map_err(|e| e.to_string())?;
                let status = res.status().as_u16();
                let text = res.text().await.map_err(|e| e.to_string())?;
                Ok((status, text))
            })
        },
    )
}
