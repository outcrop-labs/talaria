- **API keys speak Rust**: `/api/keys` and `/api/keys/{id}` — the list with
  its `canMint` gate, minting (the `tlk_` secret answered exactly once),
  revocation, and the self-imposed policy plane (#265: token/USD caps and a
  per-minute ceiling, where 0 means unlimited in the row but echoes back as
  the 0 its owner sent). The policy PUT is the port's first nullish
  `z.number()` body: the whole 400 table (safe-int guard bounds, a max
  breach that says "number" even on an int field, fractional-vs-int, type
  messages) was probed against zod 4.3.6 and pinned in Rust tests. Numbers
  now ride every Rust wire through one `js_num` helper (hoisted from the
  ledger) so an integral cap prints `1000`, never `1000.0`. Byte-diffed
  against TS for anon, member (no grant → 403), and admin callers, with the
  caps written by each runtime read back through both. One recorded
  divergence generalized into the migration doc: a non-uuid `{id}` (like
  TS's other uncaught route throws) answers the house 500 envelope, not the
  platform's plain-text sentence.
