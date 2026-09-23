- **Guided reindex — the repair path for a changed embedding model.** Swapping
  `TALARIA_EMBED_MODEL` changes vector dimensions and silently breaks every
  index/search against the old collections. Talaria now probes what the
  embedding service is actually serving (model + dimension, shown in Admin →
  Retrieval) against the LIVE Qdrant collection shape — never the registry,
  which had already gone stale once — and raises a critical alert plus an
  admin banner when they diverge (or when a brain predates hybrid search).
  One "Rebuild index" button recreates each brain in the current model's
  shape and refills it from the workspace's own records; index-don't-copy
  makes the rebuild lossless. Verified live: legacy 384d dense brains
  rebuilt to hybrid, exact-identifier and paraphrase queries both rank the
  seeded doc first, and stale points from deleted sources and
  pre-officialization grounding rules washed out in the process.
