- **A worktree stack was proxying `/api/*` to the primary stack's api — on the
  primary stack's database.** `talaria worktree` copies main's `ui/.env` and
  strips the lines that name shared state, then appends this stack's own. The
  strip-list already carried `TALARIA_API_PORT`, with a comment explaining
  exactly why: `envValue` returns the FIRST match, so leaving main's line in
  place would shadow the appended one. **`TALARIA_RUST_API_URL` was not on that
  list**, and it is the same lesson one step later.

  `talaria dev` derives the proxy's url from `TALARIA_API_PORT` — but only when
  nothing else names it, and main's `ui/.env` names it, hardcoded to
  `http://127.0.0.1:5274` by `talaria setup`. So the derivation never fired. The
  worktree's api bound its own `:54xx` correctly and nobody talked to it, while
  the app in front of it sent every `/api/*` request to whatever was listening on
  `:5274`. That is precisely the failure the appended comment warns about ("this
  app would proxy /api/* to another worktree's api, on another worktree's
  database") — and it is the hazard the whole worktree mechanism exists to
  prevent, since a second server on one database can re-seal secrets under the
  wrong key.

  Two lines: `TALARIA_RUST_API_URL` joins the stripped set, and the generated
  env writes this stack's own url rather than trusting a derivation that an
  inherited line defeats.

  **The test was correct and its fixture was never the environment it assumed.**
  `worktree.test.ts` asserted `TALARIA_API_PORT` appears exactly once and points
  at this slot — but its fixture `ui/.env` had no `TALARIA_RUST_API_URL` line at
  all, so the one variable that mattered could not drift in the test the way it
  drifts on a real machine. The fixture now carries the line `talaria setup`
  really writes, and the assertions are the same pair the port already had:
  exactly one, and this stack's.

  Found by running the stack, not by reading it: a fresh worktree came up with
  Postgres and Redis healthy and `rustApi: unreachable`, pointed at `:5274`.

  Verified: 8 tests in `cli/src/cmd/worktree.test.ts`, and the new pair is a real
  gate — reverting the one-line filter change fails it with
  `expect(received).toHaveLength(expected)` on the url line, rather than passing
  over a fixture that never had one. `bun run check` clean.
