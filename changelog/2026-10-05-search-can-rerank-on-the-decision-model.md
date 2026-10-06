- **Search can rerank on the decision model you already configured.** The
  reranker is the precision stage after vector recall, and it was a registry of
  eight cross-encoder providers each wanting its own url, key and model. The
  ninth asks for none of them: it delegates to `talaria-decide`'s one config
  row, so an operator who has already pointed the instance at a decision model
  gets reranking out of it with no second credential to rotate.

  It works by asking **one Score per candidate over shared state** — the query
  and every candidate in one request — which is what a decision model that fans
  out is for. The levels describe situations rather than a scale, because "3 out
  of 5" is not something a model can judge and *"about the same subject, but
  answers a different question than the one asked"* is. A Score answers between
  levels, so the result is a real ordering rather than four buckets, and
  dividing by the level span puts it on the same 0..=1 axis a cross-encoder's
  score already sits on — so the alignment step needs no special case.

  **What it costs, stated rather than discovered:** a provider that cannot
  answer several questions in one round trip would make this N round trips per
  search, so the port refuses instead of quietly spending them. Reranking is
  best-effort by contract — "a provider failure falls back to vector order,
  never breaks search" — so that refusal is vector order, not an error.

  The admin panel's model control is now derived from whether a provider *has* a
  catalogue rather than from an id blocklist (`meta.id !== 'tei'`). Two
  providers have no model of their own to pick: the self-hosted sidecar serves
  whatever it was started with, and this one delegates.

  Verified: 13 tests in `talaria-retrieval-rerank` (was 11) — the new provider
  asks for no credential, the levels each describe a situation, and the
  normalization puts the bottom level at 0.0, the top at 1.0 and a
  between-levels answer between. `cargo clippy -p talaria-retrieval-rerank
  --all-targets` clean; `bun run typecheck` (svelte-check, 5432 files) clean.
  Not yet exercised against a live decision provider end to end — the port's
  own wires are covered by `talaria-decide`'s tests and a stand-in endpoint, but
  this reader's request shape has not been put through a real one.
