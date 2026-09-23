- **The secrets family speaks Rust — the working vault crosses whole, and
  the port's first cross-runtime cipher proof lands with it.** Six routes
  under one proxy prefix: the collection (list/create/file/move/delete),
  the folders pair (create/rename/delete/share in one union body), the
  reveal (with its refusal ladder and the no-store header triad), the
  share pair for people and agents, the one-shot relay, and the git
  credential helper — the agent's door to `git.example.com`, which spends
  a workspace credential or a minted relay and falls through to the GitHub
  app token. The engine crossed with them: the vault's seal/open, the
  credential selection that makes a single-entry doc a token
  (`x-access-token`), the relay burn, and the folders' reveal-vs-spend
  asymmetry. Verified by a 127-case byte-diff against TS on the same
  sessions, with the interop proof as its centerpiece: both runtimes share
  one database, so the harness seals the same literal through each
  runtime and reveals it through both — the same value back every time,
  which is what the secretbox port was for. GitHub app JWTs gained a
  cross-language vector file (a committed fixture key, three clock
  instants, byte-equality both ways). The diff caught three port bugs:
  the listing's `distinct`/`order by` (Postgres requires the ordered
  column in the select list — every listing 500'd), the move route's
  admin thread (owner-only even for admins, as TS passes no isAdmin),
  and the union body sentence (zod unions flatten every failure — a
  non-object body included — to plain "Invalid input").
