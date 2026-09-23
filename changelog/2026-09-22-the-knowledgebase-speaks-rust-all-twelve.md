- **The knowledgebase speaks Rust — all twelve kb.* route files crossed
  whole** (spaces, docs, comments, backlinks, move, presence, search, public
  slugs) with the ACL engine they share. The 107-case byte-diff found four
  port bugs the unit tests hadn't: uuid binds where the column wanted none,
  the search hit's raw-row wire shape, the float4 rank decode (f32→f64
  promotion prints `…584` where JS's parseFloat prints `…522` — the faithful
  path is `::text` then parse), and PG's bare sentence where sqlx's Display
  wraps it.
