// Retrieval health + repair. The indexing pipeline is fire-and-forget by
// design (a write must never block on RAG), which means a dead Qdrant/TEI
// fails SILENTLY — the brains just stop filling. The defenses live across
// two files in TS: the health probe and the incremental sweep here
// (retrieval/backfill.ts), the durable repair runs next door in
// runs/defs/reindex.ts.
//
// WHAT HAS CROSSED is the piece the repair runs and the routes need now:
// `rag_health`. What STAYS TS is backfill.ts's read plane (`backfillStatus`)
// and the 15-minute sweep (`sweepNewActivity`/`maybeRagSweep`) — both cross
// with their callers (the admin rag route, the comms/search reads) rather
// than landing as unreachable code.
// Port of ui/src/server/retrieval/backfill.ts (the health half).

use crate::retrieval::embed::{EmbedDeps, embed_one};
use crate::retrieval::qdrant::QdrantDeps;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RagHealth {
    pub qdrant: bool,
    pub embeddings: bool,
}

/// Probe both retrieval services (cheap, ~2s worst case). A probe that
/// errors reads as DOWN — the fetch sense of `r.ok`, never an exception.
pub async fn rag_health(qd: &QdrantDeps, ed: &EmbedDeps) -> RagHealth {
    // Two probes, one budget each, in parallel — the TS Promise.all. The
    // qdrant probe rides the same HttpFetch edge the client does so a test
    // scripts it like any other call; `ok` is the fetch sense (2xx).
    let qdrant_probe = (qd.fetch)(
        "GET",
        &format!("{}/collections", (qd.base)()),
        None,
        &[],
        2_500,
    );
    let embed_probe = embed_one(ed, "health probe");
    let (qdrant, embeddings) = tokio::join!(qdrant_probe, embed_probe);
    RagHealth {
        qdrant: matches!(qdrant, Ok((200..=299, _))),
        embeddings: embeddings.map(|v| !v.is_empty()).unwrap_or(false),
    }
}
