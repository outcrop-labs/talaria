pub const BACKFILL_KIND: &str = "rag-backfill";
pub const REINDEX_KIND: &str = "rag-reindex";

pub mod artifact_routing;
pub mod backfill;
pub use talaria_retrieval_collections as collections;
pub use talaria_retrieval_embed as embed;
pub use talaria_retrieval_index as index;
pub mod migrate;
pub use talaria_retrieval_http::{HttpFetch, real_http};
pub use talaria_retrieval_qdrant as qdrant;
pub use talaria_retrieval_rerank as rerank;
pub use talaria_retrieval_sources as sources;
pub use talaria_retrieval_sparse as sparse;
