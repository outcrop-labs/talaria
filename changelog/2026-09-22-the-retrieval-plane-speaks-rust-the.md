- **The retrieval plane speaks Rust — the reindex pair, the admin console,
  and the rag family.** rag-backfill/rag-reindex crossed with real deps (the
  artifact routing and the health probe beside them), then `admin.rag`
  (status + run projections, the reranker config, space↔brain bindings, the
  enqueue kicks), then the `/api/rag` family whole — the collection registry
  the MCP `search_knowledge` tool resolves principals against, the member's
  blanked binding matrix, and the search itself. The upgrade-status cache
  invalidates from inside the process that rebuilds it.
