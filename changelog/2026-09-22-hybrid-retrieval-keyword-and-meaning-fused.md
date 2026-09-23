- **Hybrid retrieval — keyword and meaning, fused.** Every brain now indexes
  each chunk twice: the dense embedding it always had, plus a sparse
  bag-of-terms vector (Qdrant IDF-modified, so exact identifiers like env
  vars, ticket numbers, model names, and error strings survive whole).
  Searches fuse both branches with reciprocal-rank fusion, so
  `TALARIA_EMBED_MODEL` finds the doc that names it AND "how do embeddings
  get configured" finds it too. Legacy dense-only brains keep working
  untouched until rebuilt.
