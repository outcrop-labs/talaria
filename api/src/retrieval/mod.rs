// The retrieval plane — the family and its boundaries:
//
//   sparse            keyword vectors (FNV-1a hashed terms, saturated tf)
//   qdrant            the REST client — collections, points, filtered search
//   embed             the TEI embeddings client, with its sticky-base fallback
//   collections       the RAG registry — bindings are the outer ACL gate
//   index             the write core (chunk → embed → upsert) and the search merge
//   sources           the convenience indexers and the KB doc ↔ RAG routing
//   rerank            the precision stage — best-effort by contract, never fatal
//   artifact_routing  artifact ↔ brain placement (auto / none / explicit)
//   backfill          the health probe + the backfill run's read shape
//   migrate           the reindex run's read shape + the 60s upgrade status
//
// WHY A DIRECTORY AND NOT FLAT FILES. The crate's flat modules are single
// concerns (search.rs, mcp_registry.rs); this is a ten-file family with
// internal dependencies, which is exactly what gateway/ and runs/ already
// model.
//
// THE ONE SEAM EVERY CLIENT SHARES. qdrant, embed, and rerank all speak HTTP
// and must be testable against a scripted service (house rule: no
// network-dependent tests). They share one injected fetch edge here —
// `HttpFetch`, (method, url, body, headers, timeout) → (status, text) — and
// one real implementation, `real_http()`. Sticky bases and dimension caches
// stay process-global on the real path but are injected per-test so tests
// can't fight over it.

pub mod artifact_routing;
pub mod backfill;
pub mod collections;
pub mod embed;
pub mod index;
pub mod migrate;
pub mod qdrant;
pub mod rerank;
pub mod sources;
pub mod sparse;

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
                let mut req = crate::gateway::provider::http()
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
