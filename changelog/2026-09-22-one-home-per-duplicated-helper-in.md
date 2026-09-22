- **One home per duplicated helper in the api crate.** Every cluster the new
  duplicate-body detector reported is gone (`0 clusters`, down from 13), and the
  named families the sweep listed followed:

  * `audience` → `talaria-runs-define` (beside the `Authority` it answers with;
    research-def, runs-agent-hire and runs-plan-draft each had a copy).
  * `roll_drain_ms` → `talaria-update-layout` (already the import home for the
    two update crates; fleet-reconcile now reads it there).
  * `assistant_owner_for`, `personal_assistant_owners` → `talaria-users`;
    `has_oauth_tokens` → `talaria-mcp-oauth`; `talaria-mcp/src/registry.rs` had
    private copies of all three.
  * `percent_encode`/`percent_decode` → `talaria-body`. Four crates wrote JS's
    `encodeURIComponent` (gateway provider URLs, google-client path segments,
    session cookies, inbox cursors) and two wrote the decoder; the unreserved
    set is a JS contract, and five of them imported it from each other in a
    chain. HEX_UPPER is a table now, not `format!` per byte.
  * `now_ms`/`now_iso` → `talaria-agent-auth` (7 `u64` copies + `now_iso` ×3;
    the clock crate already owned the `i64` one and `epoch_ms_to_iso`).
  * `hex` → `talaria-body` (4 copies: LLM key minting, update signing, SigV4's
    canonical request, skill hashing).

  The detector's allow list is down to the three TypeScript clusters the UI
  waves own; each Rust entry died exactly when its work landed, which is what
  the stale-entry check is for. The plan's remaining W3 rows (`NowFn`,
  `utf16_*`, `fold_slug`, `truncate_bytes`, the `js_*` coercions, `sha256_hex`)
  are **not** in this commit: they are either type aliases or pairs whose
  signatures differ, so they need per-call-site surgery rather than a
  collapse — and none of them is detector-flagged, so nothing regrows
  unwatched. (Honest note: the sweep's "~800 lines" for this wave is really
  ~150 — most of the duplicates it named were already collapsed by W1/W2 or
  were never byte-identical.)

  Verified: `bun run api:check` (fmt + clippy `-D warnings` + `cargo test
  --workspace`), `bun run check` (13 rules, `gen-docs --check` clean).
