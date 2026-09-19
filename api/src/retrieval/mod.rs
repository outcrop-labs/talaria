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
pub use talaria_retrieval_collections as collections;
pub use talaria_retrieval_embed as embed;
pub use talaria_retrieval_index as index;
pub mod migrate;
pub use talaria_retrieval_qdrant as qdrant;
pub use talaria_retrieval_rerank as rerank;
pub use talaria_retrieval_sources as sources;
pub use talaria_retrieval_sparse as sparse;

pub use talaria_retrieval_http::{HttpFetch, real_http};
